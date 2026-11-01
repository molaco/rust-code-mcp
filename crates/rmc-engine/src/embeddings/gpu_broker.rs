//! One GPU embedding session per backend per process, lent out run by run.
//!
//! # Why
//!
//! Every [`super::EmbeddingGenerator`] used to build its own ONNX session, and
//! a MIGraphX session holds ≈ 1.2 GB of VRAM for as long as it lives. The
//! daemon keeps one generator per workspace in its search cache, the indexer
//! builds one per pass, the graph cache and the codemap rerank one per call —
//! the same model, the same input shape, loaded again each time. Measured on
//! the live daemon: four search sessions plus the HIP context, 4.9 GB, with
//! one identity; an index pass on top took the card to its limit, and the
//! next allocation failed inside MIGraphX as a C++ `assert` that killed the
//! process (see `vram_ledger` and leak-repro in the plan).
//!
//! # What it does
//!
//! - The broker owns the sessions, keyed by the whole [`EmbeddingBackend`]
//!   (profile, runtime, max length — everything that makes two sessions
//!   different). A generator for a GPU backend holds no session, only the
//!   key; each run asks for a [`Grant`], which MOVES the session out of the
//!   broker for the duration of the run and back when dropped. No shared
//!   pointer, no lock held while the model runs.
//! - Waiting is ordered by [`Priority`]: an interactive query (search, the
//!   codemap rerank) before the index, the index before the graph cache. A
//!   run is one sub-batch, so a query waits at most one batch.
//! - A session is created lazily, by the first grant that finds none, and
//!   only within the VRAM budget ([`VramBudget`]: the process's current KFD
//!   reading plus the measured price of a session of this profile) and the
//!   free memory of its card ([`room_on_card`]). Creation and a warm-up run —
//!   every allocation the session will make — happen under the cross-process
//!   `gpu_lock`. Over either limit the run is refused with the numbers,
//!   instead of letting MIGraphX find out by aborting.
//! - [`release_idle`] drops sessions nobody has used for a while; the daemon's
//!   watchdog calls it every tick. The next grant creates one again.
//!
//! The cross-process `gpu_lock` stays where it was, around each run: the
//! broker orders this process's users, the flock orders processes.
//!
//! CPU and remote backends do not come here: a CPU session costs RAM, not
//! VRAM, and serialising CPU runs behind one session would only slow them.

use super::backend::EmbeddingBackend;
use super::error::EmbeddingError;
use super::fastembed_onnx::FastembedOnnxEmbedder;
use super::vram_ledger::{self, DeviceCard, EmbeddingOwner, VRAM_LOG_TARGET};
use serde::Serialize;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Order in which waiting runs get the session. Lower goes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    /// Someone is waiting on the answer: `search`, the codemap rerank.
    Interactive = 0,
    /// Index passes (the background sync among them) and offline tools.
    Index = 1,
    /// The hypergraph's embedding cache: large, and nobody is waiting on a
    /// single batch of it.
    Graph = 2,
}

const PRIORITIES: usize = 3;

impl Priority {
    fn of(owner: EmbeddingOwner) -> Self {
        match owner {
            EmbeddingOwner::Search | EmbeddingOwner::CodemapRerank => Self::Interactive,
            EmbeddingOwner::Index | EmbeddingOwner::Tool => Self::Index,
            EmbeddingOwner::GraphCache | EmbeddingOwner::ProviderCensus => Self::Graph,
        }
    }
}

enum SlotState<S> {
    /// No session: never created, released while idle, or discarded after a
    /// failed creation or a panic during a run.
    Empty,
    /// A grant is building the session; it arrives as that grant.
    Creating { for_owner: EmbeddingOwner },
    /// In the broker, nobody running it.
    Idle { session: S, since: Instant },
    /// Lent out for one run.
    Granted { to: EmbeddingOwner, since: Instant },
}

struct Slot<K, S> {
    key: K,
    state: SlotState<S>,
    /// Grants waiting for this slot, by priority.
    waiting: [u32; PRIORITIES],
    created: u64,
    grants: u64,
}

/// Sessions by key, and the condition every waiter sleeps on.
///
/// Generic over the key and the session so that the waiting and ordering
/// logic is tested without a GPU; the process has exactly one instance for
/// real sessions, [`GPU_BROKER`].
pub(super) struct Broker<K, S> {
    /// Slots are only ever appended, so an index found once stays valid.
    slots: Mutex<Vec<Slot<K, S>>>,
    changed: Condvar,
}

/// A session lent out for one run. Dropping it returns the session.
pub(super) struct Grant<'b, K: PartialEq, S> {
    broker: &'b Broker<K, S>,
    slot: usize,
    /// `Some` for the whole life of the grant; taken in `Drop`.
    session: Option<S>,
}

impl<K: PartialEq, S> Grant<'_, K, S> {
    pub(super) fn session(&self) -> &S {
        self.session.as_ref().unwrap()
    }
}

impl<K: PartialEq, S> Drop for Grant<'_, K, S> {
    fn drop(&mut self) {
        let session = self.session.take().unwrap();
        // A run that panicked may have left the session half-way through
        // something; it is not handed to the next caller. (A C++ abort inside
        // the run never gets here — the process is gone.)
        let discard = std::thread::panicking();
        // Poison is ignored on purpose: during a panic `unwrap` would abort,
        // and the state below is rewritten whole, not patched.
        let mut slots = self
            .broker
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let slot = &mut slots[self.slot];
        let discarded = if discard {
            slot.state = SlotState::Empty;
            Some(session)
        } else {
            slot.state = SlotState::Idle {
                session,
                since: Instant::now(),
            };
            None
        };
        drop(slots);
        self.broker.changed.notify_all();
        // Dropped outside the lock: releasing a GPU session reads the meters
        // and frees device memory.
        drop(discarded);
    }
}

/// Puts a slot back to `Empty` if its creator unwinds before finishing.
/// Without it every later grant of that key would wait for a creation that
/// will never arrive.
struct CreationGuard<'b, K, S> {
    broker: &'b Broker<K, S>,
    slot: usize,
    armed: bool,
}

impl<K, S> Drop for CreationGuard<'_, K, S> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut slots = self
            .broker
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slots[self.slot].state = SlotState::Empty;
        drop(slots);
        self.broker.changed.notify_all();
    }
}

impl<K: PartialEq + Clone, S> Broker<K, S> {
    pub(super) const fn new() -> Self {
        Self {
            slots: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        }
    }

    fn slot_index(slots: &mut Vec<Slot<K, S>>, key: &K) -> usize {
        if let Some(index) = slots.iter().position(|s| s.key == *key) {
            return index;
        }
        slots.push(Slot {
            key: key.clone(),
            state: SlotState::Empty,
            waiting: [0; PRIORITIES],
            created: 0,
            grants: 0,
        });
        slots.len() - 1
    }

    /// Wait for the session of `key` and take it for one run; build it with
    /// `create` if there is none.
    ///
    /// Blocks the calling thread: call it on the blocking pool, as the run
    /// itself is. Returns how long it waited along with the grant.
    pub(super) fn grant(
        &self,
        key: &K,
        owner: EmbeddingOwner,
        create: impl FnOnce() -> Result<S, EmbeddingError>,
    ) -> Result<(Grant<'_, K, S>, Duration), EmbeddingError> {
        let started = Instant::now();
        let rank = Priority::of(owner) as usize;
        let mut slots = self.slots.lock().unwrap();
        let index = Self::slot_index(&mut slots, key);
        slots[index].waiting[rank] += 1;
        loop {
            let slot = &mut slots[index];
            let outranked = slot.waiting[..rank].iter().any(|&n| n > 0);
            if !outranked {
                match std::mem::replace(&mut slot.state, SlotState::Empty) {
                    SlotState::Idle { session, .. } => {
                        slot.waiting[rank] -= 1;
                        slot.grants += 1;
                        slot.state = SlotState::Granted {
                            to: owner,
                            since: Instant::now(),
                        };
                        let grant = Grant {
                            broker: self,
                            slot: index,
                            session: Some(session),
                        };
                        return Ok((grant, started.elapsed()));
                    }
                    SlotState::Empty => {
                        slot.waiting[rank] -= 1;
                        slot.state = SlotState::Creating { for_owner: owner };
                        drop(slots);
                        return self.create_as_grant(index, owner, create, started);
                    }
                    busy => slot.state = busy,
                }
            }
            slots = self.changed.wait(slots).unwrap();
        }
    }

    /// Second half of [`Self::grant`] for a slot this caller has just marked
    /// `Creating`: build outside the lock, then hand the result over as the
    /// caller's grant.
    fn create_as_grant(
        &self,
        index: usize,
        owner: EmbeddingOwner,
        create: impl FnOnce() -> Result<S, EmbeddingError>,
        started: Instant,
    ) -> Result<(Grant<'_, K, S>, Duration), EmbeddingError> {
        let mut guard = CreationGuard {
            broker: self,
            slot: index,
            armed: true,
        };
        // An `Err` leaves the guard armed: the slot goes back to `Empty` and
        // the waiters wake up to try (and most likely fail) themselves, each
        // with its own error rather than a copy of this one.
        let session = create()?;
        guard.armed = false;
        let mut slots = self.slots.lock().unwrap();
        let slot = &mut slots[index];
        slot.created += 1;
        slot.grants += 1;
        slot.state = SlotState::Granted {
            to: owner,
            since: Instant::now(),
        };
        drop(slots);
        Ok((
            Grant {
                broker: self,
                slot: index,
                session: Some(session),
            },
            started.elapsed(),
        ))
    }

    /// Make sure the session of `key` exists, without waiting for it.
    ///
    /// Called where a generator is constructed, so that a broken GPU fails
    /// there (with the error the constructor always returned) rather than at
    /// the first run. Does nothing if the session exists, is lent out or is
    /// being built by someone else: construction happens on async threads,
    /// and must not sit behind somebody's index batch.
    pub(super) fn ensure(
        &self,
        key: &K,
        owner: EmbeddingOwner,
        create: impl FnOnce() -> Result<S, EmbeddingError>,
    ) -> Result<(), EmbeddingError> {
        let mut slots = self.slots.lock().unwrap();
        let index = Self::slot_index(&mut slots, key);
        if !matches!(slots[index].state, SlotState::Empty) {
            return Ok(());
        }
        slots[index].state = SlotState::Creating { for_owner: owner };
        drop(slots);
        let (grant, _) = self.create_as_grant(index, owner, create, Instant::now())?;
        // Not a run: the creation is counted, the grant is not.
        self.slots.lock().unwrap()[index].grants -= 1;
        drop(grant);
        Ok(())
    }

    /// Take out every session idle for at least `max_idle`; the caller drops
    /// them (outside the lock).
    pub(super) fn take_idle(&self, max_idle: Duration) -> Vec<(K, S)> {
        let mut slots = self.slots.lock().unwrap();
        let mut taken = Vec::new();
        for slot in slots.iter_mut() {
            let stale =
                matches!(&slot.state, SlotState::Idle { since, .. } if since.elapsed() >= max_idle);
            if !stale {
                continue;
            }
            let SlotState::Idle { session, .. } =
                std::mem::replace(&mut slot.state, SlotState::Empty)
            else {
                unreachable!()
            };
            taken.push((slot.key.clone(), session));
        }
        taken
    }

    fn slot_reports(&self, describe: impl Fn(&K) -> String) -> Vec<BrokerSlotReport> {
        let slots = self.slots.lock().unwrap();
        slots
            .iter()
            .map(|slot| {
                let (state, holder, state_secs) = match &slot.state {
                    SlotState::Empty => ("empty", None, None),
                    SlotState::Creating { for_owner } => ("creating", Some(*for_owner), None),
                    SlotState::Idle { since, .. } => {
                        ("idle", None, Some(since.elapsed().as_secs()))
                    }
                    SlotState::Granted { to, since } => {
                        ("granted", Some(*to), Some(since.elapsed().as_secs()))
                    }
                };
                BrokerSlotReport {
                    backend: describe(&slot.key),
                    state,
                    holder,
                    state_secs,
                    waiting_interactive: slot.waiting[Priority::Interactive as usize],
                    waiting_index: slot.waiting[Priority::Index as usize],
                    waiting_graph: slot.waiting[Priority::Graph as usize],
                    sessions_created: slot.created,
                    grants: slot.grants,
                }
            })
            .collect()
    }
}

/// The process's broker for real GPU sessions.
static GPU_BROKER: Broker<EmbeddingBackend, FastembedOnnxEmbedder> = Broker::new();

/// Proof that a GPU session passed the budget check; required by
/// `FastembedOnnxEmbedder::new_gpu`. Only [`admit`] makes one (and tests).
pub(super) struct Admission {
    /// The card the check looked at, `None` if it could not be located.
    card: Option<DeviceCard>,
}

impl Admission {
    /// For the GPU probes that measure a bare session on purpose.
    #[cfg(test)]
    pub(super) fn for_probe() -> Self {
        Self { card: None }
    }
}

/// HIP device the sessions run on: the MIGraphX execution provider is built
/// with its default `device_id`, 0 (`migraphx_execution_providers`).
const SESSION_DEVICE: usize = 0;

/// May one more GPU session of `backend` be created now?
///
/// Two checks, because they answer different questions:
/// - the budget — may THIS PROCESS hold one more session ([`VramBudget`]);
/// - the room on the card — is there memory for it at all right now, whoever
///   holds the rest ([`room_on_card`]). Without it the allocation fails inside
///   MIGraphX, and MIGraphX reports that as a C++ `assert`: the process dies.
///
/// 🚨 Only meaningful under the cross-process `gpu_lock`, held until the
/// session has made every allocation it will make (see [`create_session`]):
/// otherwise another of our processes can take the room between the check and
/// the allocation.
pub(super) fn admit(backend: &EmbeddingBackend) -> Result<Admission, EmbeddingError> {
    let profile = backend.profile.name();
    let budget = VramBudget::from_env(vram_ledger::card_total_bytes())?;
    let process = vram_ledger::read_vram().process_bytes;
    let price = vram_ledger::known_session_charge(profile);
    budget.admit(profile, process, price)?;
    let card = vram_ledger::device_card(SESSION_DEVICE);
    room_on_card(profile, card.as_ref(), price)?;
    Ok(Admission { card })
}

/// Is there `price` of free memory on the card the session will go to?
///
/// Every number here is a measurement: the card's own counters, and the
/// largest charge of such a session measured on this machine (kept between
/// processes). No margin is added on top — the price already includes the
/// first run, which is where MIGraphX allocates, and the HIP context of a
/// process's first session. Without a price (the first session of a profile
/// ever) or without a card meter there is nothing to compare; that is said in
/// the log, not refused: the session that runs is what measures the price.
fn room_on_card(
    profile: &str,
    card: Option<&DeviceCard>,
    price: Option<u64>,
) -> Result<(), EmbeddingError> {
    let mib = |b: u64| b >> 20;
    let Some(card) = card else {
        tracing::info!(
            target: VRAM_LOG_TARGET,
            profile,
            "GPU session: the device's card cannot be located; free memory not checked"
        );
        return Ok(());
    };
    let Some(price) = price else {
        tracing::info!(
            target: VRAM_LOG_TARGET,
            profile,
            card = %card.pci,
            free_mib = mib(card.free_bytes()),
            "GPU session: no price measured for this profile yet; free memory not checked"
        );
        return Ok(());
    };
    if card.free_bytes() >= price {
        return Ok(());
    }
    Err(EmbeddingError::model_init(format!(
        "GPU embedding session for `{profile}` refused: the card {} has {} MiB free \
         ({} of {} MiB used, by every process on it), a session costs {} MiB",
        card.pci,
        mib(card.free_bytes()),
        mib(card.used_bytes),
        mib(card.total_bytes),
        mib(price),
    )))
}

/// Build a GPU session for `backend` and make it allocate everything it will
/// ever allocate, all under the cross-process lock and within the checks of
/// [`admit`].
///
/// The warm-up run is what makes the check hold: VRAM is allocated at
/// creation and in the session's FIRST run (MIGraphX loads the program there;
/// measured: after it, 100 runs moved KFD 1411 → 1411 MiB). Left to the
/// first real run, that allocation would happen after the lock was released
/// and the room counted for it possibly gone.
fn create_session(
    backend: &EmbeddingBackend,
    owner: EmbeddingOwner,
) -> Result<FastembedOnnxEmbedder, EmbeddingError> {
    let _gpu = super::gpu_lock::GpuLock::acquire()?;
    let admission = admit(backend)?;
    let before = vram_ledger::process_kfd_vram_by_gpu();
    let session = FastembedOnnxEmbedder::new_gpu(backend, owner, &admission)?;
    session.warm_up()?;
    check_landed_on(&admission, &before, &vram_ledger::process_kfd_vram_by_gpu());
    Ok(session)
}

/// The free-memory check watched one card; the session's memory must have
/// grown on that card. If it grew elsewhere the device mapping in
/// [`vram_ledger::device_card`] is wrong for this machine, and the check is
/// guarding the wrong card — said loudly, since nothing else would show it.
fn check_landed_on(
    admission: &Admission,
    before: &std::collections::BTreeMap<u32, u64>,
    after: &std::collections::BTreeMap<u32, u64>,
) {
    let Some(card) = &admission.card else {
        return;
    };
    let grown = after
        .iter()
        .map(|(gpu, bytes)| {
            (
                *gpu,
                bytes.saturating_sub(before.get(gpu).copied().unwrap_or(0)),
            )
        })
        .max_by_key(|&(_, delta)| delta);
    if let Some((gpu, delta)) = grown
        && delta > 0
        && gpu != card.gpu_id
    {
        tracing::warn!(
            target: VRAM_LOG_TARGET,
            checked_gpu_id = card.gpu_id,
            checked_card = %card.pci,
            landed_gpu_id = gpu,
            landed_bytes = delta,
            "GPU session allocated on a different GPU than the one its free memory was checked on"
        );
    }
}

/// See [`Broker::ensure`].
pub(super) fn ensure_session(
    backend: &EmbeddingBackend,
    owner: EmbeddingOwner,
) -> Result<(), EmbeddingError> {
    GPU_BROKER.ensure(backend, owner, || create_session(backend, owner))
}

/// Run `f` on the shared session of `backend`, waiting for it as `owner`.
/// Blocking; call on the blocking pool.
pub(super) fn with_session<T>(
    backend: &EmbeddingBackend,
    owner: EmbeddingOwner,
    f: impl FnOnce(&FastembedOnnxEmbedder) -> Result<T, EmbeddingError>,
) -> Result<T, EmbeddingError> {
    let (grant, waited) = GPU_BROKER.grant(backend, owner, || create_session(backend, owner))?;
    vram_ledger::record_grant(owner, waited);
    if waited >= Duration::from_secs(5) {
        tracing::info!(
            target: VRAM_LOG_TARGET,
            owner = ?owner,
            profile = backend.profile.name(),
            waited_ms = waited.as_millis() as u64,
            "waited for the shared GPU embedding session"
        );
    }
    let result = f(grant.session());
    // Read while the grant is still held: no other run of this process can
    // be moving the meter.
    vram_ledger::observe_after_run(backend.profile.name());
    result
}

/// Drop every GPU session nobody has used for `max_idle`; returns how many.
/// `Duration::ZERO` drops every idle one.
pub fn release_idle_gpu_sessions(max_idle: Duration) -> usize {
    let taken = GPU_BROKER.take_idle(max_idle);
    let count = taken.len();
    for (backend, session) in taken {
        tracing::info!(
            target: VRAM_LOG_TARGET,
            profile = backend.profile.name(),
            idle_secs = max_idle.as_secs(),
            "releasing an idle GPU embedding session"
        );
        drop(session);
    }
    count
}

/// Environment knob: VRAM this process may hold, in MiB. `0` disables the
/// check. Unset: half the card (see [`VramBudget::from_env`]).
pub const VRAM_BUDGET_ENV: &str = "RMC_VRAM_BUDGET_MB";

/// How much VRAM this process may hold before it refuses to create another
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VramBudget {
    /// `RMC_VRAM_BUDGET_MB=0`.
    Disabled,
    /// Set explicitly.
    Explicit { bytes: u64 },
    /// Half of `mem_info_vram_total`, summed over every card — like the
    /// process meter, which sums every KFD node. On the reference machine the
    /// sum includes an integrated GPU's 512 MiB carve-out: 6392 MiB, not 6136.
    HalfCard { bytes: u64 },
    /// Not set, and the card's size cannot be read: nothing to derive a
    /// default from, so nothing is enforced.
    NoCardMeter,
}

impl VramBudget {
    /// The default is half the card, and the half is about binary rollouts,
    /// not about this process: a retiring daemon keeps its sessions until its
    /// last client leaves while its successor builds its own, so two daemons
    /// of one user share the card for a while. One process alone needs one
    /// session per backend (≈ 1.4 GB for `local-gpu-bge`), well inside either
    /// half; the budget exists to refuse the case where it would not.
    fn from_env(card_total: Option<u64>) -> Result<Self, EmbeddingError> {
        let raw = std::env::var(VRAM_BUDGET_ENV).ok();
        Self::resolve(raw.as_deref(), card_total)
    }

    fn resolve(raw: Option<&str>, card_total: Option<u64>) -> Result<Self, EmbeddingError> {
        match raw.map(str::trim) {
            Some(text) if !text.is_empty() => {
                let mb: u64 = text.parse().map_err(|_| {
                    EmbeddingError::model_init(format!(
                        "{VRAM_BUDGET_ENV}={text} is not a number of MiB"
                    ))
                })?;
                Ok(if mb == 0 {
                    Self::Disabled
                } else {
                    Self::Explicit { bytes: mb << 20 }
                })
            }
            _ => Ok(match card_total {
                Some(total) => Self::HalfCard { bytes: total / 2 },
                None => Self::NoCardMeter,
            }),
        }
    }

    fn bytes(self) -> Option<u64> {
        match self {
            Self::Explicit { bytes } | Self::HalfCard { bytes } => Some(bytes),
            Self::Disabled | Self::NoCardMeter => None,
        }
    }

    /// May a session of `profile` be created now?
    ///
    /// `process` is this process's VRAM (KFD), `expected` the last measured
    /// price of such a session (`None` before the first one: then only the
    /// current holding is checked — the first session of a profile is what
    /// measures the price).
    fn admit(
        self,
        profile: &str,
        process: Option<u64>,
        expected: Option<u64>,
    ) -> Result<(), EmbeddingError> {
        let Some(budget) = self.bytes() else {
            return Ok(());
        };
        let Some(process) = process else {
            // A budget somebody set but nobody can check is refused rather
            // than silently ignored; the derived default needs the meter too,
            // but a missing process meter beside a present card meter means
            // a non-KFD driver, where the default simply does not apply.
            return match self {
                Self::Explicit { .. } => Err(EmbeddingError::model_init(format!(
                    "{VRAM_BUDGET_ENV} is set, but this process's VRAM cannot be read (no KFD meter)"
                ))),
                _ => Ok(()),
            };
        };
        let expected = expected.unwrap_or(0);
        if process + expected <= budget {
            return Ok(());
        }
        let mib = |b: u64| b >> 20;
        Err(EmbeddingError::model_init(format!(
            "GPU embedding session for `{profile}` refused: this process holds {} MiB of VRAM, \
             a session costs {} MiB, the budget is {} MiB ({VRAM_BUDGET_ENV}; {self:?})",
            mib(process),
            mib(expected),
            mib(budget),
        )))
    }
}

/// One shared session in `runtime_status.gpu.broker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokerSlotReport {
    /// Profile name of the backend the session is for.
    pub backend: String,
    /// `empty` / `creating` / `idle` / `granted`.
    pub state: &'static str,
    /// Who is creating or running it.
    pub holder: Option<EmbeddingOwner>,
    /// Seconds in the current state (`idle`/`granted`).
    pub state_secs: Option<u64>,
    pub waiting_interactive: u32,
    pub waiting_index: u32,
    pub waiting_graph: u32,
    /// Sessions this slot has built so far: 1 for a daemon that never
    /// released one idle; more = idle releases or failures.
    pub sessions_created: u64,
    pub grants: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrokerReport {
    pub slots: Vec<BrokerSlotReport>,
    /// The budget as it would apply to the next creation; an unparsable
    /// knob is shown as its error.
    pub budget: Result<VramBudget, String>,
    /// The card sessions are created on, as the free-memory check sees it
    /// now. `None` = it cannot be located (and nothing is checked).
    pub device_card: Option<DeviceCard>,
    /// Price of one session by profile, as the next check would use it
    /// (this process's measurement, else the one written down by an earlier
    /// process).
    pub session_price_bytes: Vec<(String, Option<u64>)>,
}

pub(super) fn broker_report(card_total: Option<u64>) -> BrokerReport {
    let slots = GPU_BROKER.slot_reports(|backend| backend.profile.name().to_string());
    let session_price_bytes = slots
        .iter()
        .map(|s| {
            (
                s.backend.clone(),
                vram_ledger::known_session_charge(&s.backend),
            )
        })
        .collect();
    BrokerReport {
        slots,
        budget: VramBudget::from_env(card_total).map_err(|e| e.to_string()),
        device_card: vram_ledger::device_card(SESSION_DEVICE),
        session_price_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    type TestBroker = Broker<&'static str, u32>;

    /// Many concurrent grants of one key: exactly one creation, every run
    /// gets the session, and never two at once.
    #[test]
    fn one_session_for_many_concurrent_users() {
        let broker: &'static TestBroker = Box::leak(Box::new(Broker::new()));
        let creations = Arc::new(AtomicU32::new(0));
        let running = Arc::new(AtomicU32::new(0));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let creations = creations.clone();
                let running = running.clone();
                std::thread::spawn(move || {
                    let owner = if i % 2 == 0 {
                        EmbeddingOwner::Index
                    } else {
                        EmbeddingOwner::Search
                    };
                    for _ in 0..25 {
                        let (grant, _) = broker
                            .grant(&"bge", owner, || {
                                creations.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_millis(20));
                                Ok(7)
                            })
                            .unwrap();
                        assert_eq!(*grant.session(), 7);
                        assert_eq!(
                            running.fetch_add(1, Ordering::SeqCst),
                            0,
                            "two runs at once"
                        );
                        std::thread::sleep(Duration::from_micros(200));
                        running.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(creations.load(Ordering::SeqCst), 1);
        let report = broker.slot_reports(|k| k.to_string());
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].sessions_created, 1);
        assert_eq!(report[0].grants, 200);
        assert_eq!(report[0].state, "idle");
    }

    /// Different keys are different sessions.
    #[test]
    fn keys_do_not_share_a_session() {
        let broker = TestBroker::new();
        let (a, _) = broker.grant(&"a", EmbeddingOwner::Index, || Ok(1)).unwrap();
        // `a` is still lent out; `b` must not wait for it.
        let (b, _) = broker.grant(&"b", EmbeddingOwner::Index, || Ok(2)).unwrap();
        assert_eq!((*a.session(), *b.session()), (1, 2));
    }

    /// A waiting query is served before a waiting index run, whatever order
    /// they arrived in.
    #[test]
    fn interactive_waiter_goes_before_index_waiter() {
        let broker: &'static TestBroker = Box::leak(Box::new(Broker::new()));
        let order = Arc::new(Mutex::new(Vec::new()));
        let (held, _) = broker
            .grant(&"k", EmbeddingOwner::GraphCache, || Ok(0))
            .unwrap();
        let spawn = |owner: EmbeddingOwner, label: &'static str| {
            let order = order.clone();
            std::thread::spawn(move || {
                let (_g, _) = broker.grant(&"k", owner, || unreachable!()).unwrap();
                order.lock().unwrap().push(label);
            })
        };
        let index = spawn(EmbeddingOwner::Index, "index");
        wait_for_waiters(broker, 1);
        let search = spawn(EmbeddingOwner::Search, "search");
        wait_for_waiters(broker, 2);
        drop(held);
        index.join().unwrap();
        search.join().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["search", "index"]);
    }

    fn wait_for_waiters(broker: &TestBroker, n: u32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let waiting: u32 = broker.slots.lock().unwrap()[0].waiting.iter().sum();
            if waiting == n {
                return;
            }
            assert!(Instant::now() < deadline, "waiters never arrived");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A failed creation leaves no half-made slot: the next grant tries again.
    #[test]
    fn failed_creation_is_retried_by_the_next_grant() {
        let broker = TestBroker::new();
        let err = broker
            .grant(&"k", EmbeddingOwner::Index, || {
                Err(EmbeddingError::model_init("no GPU"))
            })
            .err()
            .unwrap();
        assert!(err.to_string().contains("no GPU"));
        let (g, _) = broker.grant(&"k", EmbeddingOwner::Index, || Ok(3)).unwrap();
        assert_eq!(*g.session(), 3);
    }

    /// A panic during a run discards the session; the next grant builds a
    /// fresh one instead of inheriting it, and nobody hangs.
    #[test]
    fn panic_in_a_run_discards_the_session() {
        let broker: &'static TestBroker = Box::leak(Box::new(Broker::new()));
        let crashed = std::thread::spawn(move || {
            let (_g, _) = broker.grant(&"k", EmbeddingOwner::Index, || Ok(1)).unwrap();
            panic!("run blew up");
        })
        .join();
        assert!(crashed.is_err());
        let (g, _) = broker.grant(&"k", EmbeddingOwner::Index, || Ok(2)).unwrap();
        assert_eq!(*g.session(), 2);
    }

    /// A panic inside creation does not leave the slot `Creating` forever.
    #[test]
    fn panic_in_creation_frees_the_slot() {
        let broker: &'static TestBroker = Box::leak(Box::new(Broker::new()));
        let crashed = std::thread::spawn(move || {
            let _ = broker.grant(&"k", EmbeddingOwner::Index, || panic!("load blew up"));
        })
        .join();
        assert!(crashed.is_err());
        let (g, _) = broker.grant(&"k", EmbeddingOwner::Index, || Ok(5)).unwrap();
        assert_eq!(*g.session(), 5);
    }

    #[test]
    fn idle_sessions_are_taken_only_after_the_idle_time() {
        let broker = TestBroker::new();
        broker
            .ensure(&"k", EmbeddingOwner::Index, || Ok(1))
            .unwrap();
        assert!(broker.take_idle(Duration::from_secs(3600)).is_empty());
        let (g, _) = broker
            .grant(&"k", EmbeddingOwner::Index, || unreachable!())
            .unwrap();
        // Lent out: not idle, whatever the threshold.
        assert!(broker.take_idle(Duration::ZERO).is_empty());
        drop(g);
        assert_eq!(broker.take_idle(Duration::ZERO), vec![("k", 1)]);
        let report = broker.slot_reports(|k| k.to_string());
        assert_eq!(
            (
                report[0].state,
                report[0].sessions_created,
                report[0].grants
            ),
            ("empty", 1, 1)
        );
    }

    /// `ensure` never waits for a session that is lent out.
    #[test]
    fn ensure_does_not_wait_for_a_busy_session() {
        let broker = TestBroker::new();
        let (_held, _) = broker.grant(&"k", EmbeddingOwner::Index, || Ok(1)).unwrap();
        broker
            .ensure(&"k", EmbeddingOwner::Search, || unreachable!())
            .unwrap();
    }

    #[test]
    fn budget_knob_parses_and_defaults_to_half_the_card() {
        let card = Some(12_868_124_672);
        assert_eq!(
            VramBudget::resolve(None, card).unwrap(),
            VramBudget::HalfCard {
                bytes: 6_434_062_336
            }
        );
        assert_eq!(
            VramBudget::resolve(Some(" "), None).unwrap(),
            VramBudget::NoCardMeter
        );
        assert_eq!(
            VramBudget::resolve(Some("0"), card).unwrap(),
            VramBudget::Disabled
        );
        assert_eq!(
            VramBudget::resolve(Some("4096"), card).unwrap(),
            VramBudget::Explicit { bytes: 4096 << 20 }
        );
        assert!(VramBudget::resolve(Some("4G"), card).is_err());
    }

    #[test]
    fn budget_refuses_with_the_numbers() {
        let budget = VramBudget::Explicit { bytes: 3000 << 20 };
        budget
            .admit("bge", Some(1500 << 20), Some(1400 << 20))
            .unwrap();
        let err = budget
            .admit("bge", Some(1700 << 20), Some(1400 << 20))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("holds 1700 MiB") && err.contains("costs 1400 MiB"),
            "{err}"
        );
        assert!(err.contains("budget is 3000 MiB"), "{err}");
        // First session of a profile: only the current holding is checked.
        budget.admit("bge", Some(2900 << 20), None).unwrap();
        assert!(budget.admit("bge", Some(3100 << 20), None).is_err());
    }

    /// Oracle for gpu-broker on the real card: 50 index passes and 50
    /// searches, concurrently, each through a FRESH `EmbeddingGenerator` (as
    /// the indexer builds one per pass and the search cache one per
    /// workspace). Before the broker that was up to 100 sessions; now the
    /// ledger must show exactly one created, and the process's VRAM after the
    /// last run must equal the reading after the first few, ±64 MiB.
    ///
    /// The live daemon shares the card; one session is ≈ 1.4 GB.
    ///
    /// `cargo test --release -p rmc-engine --features embeddings-migraphx one_gpu_session_for -- --ignored --nocapture`
    #[cfg(feature = "embeddings-migraphx")]
    #[test]
    #[ignore = "needs an AMD GPU, ORT with MIGraphX and a downloaded model"]
    fn one_gpu_session_for_fifty_index_passes_and_fifty_searches() {
        use crate::embeddings::{EmbeddingGenerator, gpu_ledger_report};
        const ROUNDS: usize = 50;
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let created_before: u64 = gpu_ledger_report().owners.iter().map(|o| o.created).sum();
        let readings = rt.block_on(async {
            let index = {
                let backend = backend.clone();
                tokio::spawn(async move {
                    let mut readings = Vec::new();
                    for pass in 0..ROUNDS {
                        let generator = EmbeddingGenerator::with_backend(
                            backend.clone(),
                            EmbeddingOwner::Index,
                        )
                        .unwrap();
                        let texts = (0..32)
                            .map(|i| format!("fn pass_{pass}_item_{i}() -> u64 {{ {i} }}"))
                            .collect();
                        generator.embed_documents(texts).await.unwrap();
                        drop(generator);
                        readings.push(vram_ledger::read_vram().process_bytes.unwrap());
                    }
                    readings
                })
            };
            let search = tokio::spawn(async move {
                for round in 0..ROUNDS {
                    let generator =
                        EmbeddingGenerator::with_backend(backend.clone(), EmbeddingOwner::Search)
                            .unwrap();
                    generator
                        .embed_queries(vec![format!("where is round {round} handled")])
                        .await
                        .unwrap();
                }
            });
            search.await.unwrap();
            index.await.unwrap()
        });
        let report = gpu_ledger_report();
        eprintln!("{:#?}", report.broker);
        let created: u64 = report.owners.iter().map(|o| o.created).sum::<u64>() - created_before;
        assert_eq!(created, 1, "sessions created: {created}");
        let slot = &report.broker.slots[0];
        assert_eq!(slot.sessions_created, 1);
        assert!(slot.grants >= 2 * ROUNDS as u64, "grants {}", slot.grants);
        eprintln!(
            "process VRAM after passes 1..=5, MiB: {:?}; price {:?} MiB",
            readings[..5].iter().map(|b| b >> 20).collect::<Vec<_>>(),
            vram_ledger::known_session_charge("local-gpu-bge").map(|b| b >> 20),
        );
        let early = readings[4];
        let last = *readings.last().unwrap();
        eprintln!(
            "process VRAM after pass 5: {} MiB, after pass {ROUNDS}: {} MiB",
            early >> 20,
            last >> 20
        );
        assert!(
            (last as i64 - early as i64).abs() <= 64 << 20,
            "VRAM moved {} MiB between pass 5 and pass {ROUNDS}",
            (last as i64 - early as i64) >> 20
        );
    }

    fn card(total_mib: u64, used_mib: u64) -> DeviceCard {
        DeviceCard {
            ordinal: 0,
            gpu_id: 18572,
            pci: "0000:03:00.0".into(),
            total_bytes: total_mib << 20,
            used_bytes: used_mib << 20,
        }
    }

    #[test]
    fn room_on_card_refuses_with_the_numbers() {
        let price = Some(1454 << 20);
        room_on_card("bge", Some(&card(12272, 10818)), price).unwrap();
        let err = room_on_card("bge", Some(&card(12272, 10819)), price)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("has 1453 MiB free") && err.contains("costs 1454 MiB"),
            "{err}"
        );
        assert!(err.contains("0000:03:00.0"), "{err}");
        // Nothing to compare: not refused.
        room_on_card("bge", Some(&card(12272, 12272)), None).unwrap();
        room_on_card("bge", None, price).unwrap();
    }

    /// The no-abort check on the real card, without allocating anything: the
    /// price of `local-gpu-bge` is set above what the card has free, and the
    /// broker must refuse to create the session — with the card's numbers —
    /// while this process's VRAM stays where it was. Mutation that must fail
    /// it: drop the `room_on_card` call from `admit`.
    ///
    /// Takes the real `gpu_lock` for a moment (as every creation does). The
    /// process budget is switched off, or it would refuse first:
    /// `RMC_VRAM_BUDGET_MB=0 cargo test -p rmc-engine room_on_the_real_card -- --ignored`
    #[test]
    #[ignore = "reads the real card through KFD and takes the real GPU lock"]
    fn room_on_the_real_card_is_checked_before_any_allocation() {
        assert_eq!(
            VramBudget::from_env(None).unwrap(),
            VramBudget::Disabled,
            "run with RMC_VRAM_BUDGET_MB=0"
        );
        let card = vram_ledger::device_card(SESSION_DEVICE).expect("no KFD GPU at ordinal 0");
        eprintln!("{card:?}");
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        vram_ledger::set_price_for_test(backend.profile.name(), card.free_bytes() + (1 << 30));
        let before = vram_ledger::read_vram().process_bytes;
        let err = ensure_session(&backend, EmbeddingOwner::Tool)
            .err()
            .expect("created a session the card has no room for")
            .to_string();
        assert!(err.contains("MiB free") && err.contains(&card.pci), "{err}");
        assert_eq!(vram_ledger::read_vram().process_bytes, before);
        let report = broker_report(None);
        assert_eq!(report.slots[0].state, "empty");
        assert_eq!(report.slots[0].sessions_created, 0);
    }

    /// Does a run allocate after the warm-up? Runs the shared session from
    /// fresh OS threads, one after another, and reads KFD after each.
    ///
    /// `cargo test --release -p rmc-engine --features embeddings-migraphx runs_after_warm_up -- --ignored --nocapture`
    #[cfg(feature = "embeddings-migraphx")]
    #[test]
    #[ignore = "needs an AMD GPU, ORT with MIGraphX and a downloaded model"]
    fn runs_after_warm_up_by_thread() {
        let backend = EmbeddingBackend::from_profile_name("local-gpu-bge").unwrap();
        ensure_session(&backend, EmbeddingOwner::Tool).unwrap();
        let mut readings = vec![vram_ledger::read_vram().process_bytes.unwrap() >> 20];
        for round in 0..8 {
            let backend = backend.clone();
            std::thread::spawn(move || {
                with_session(&backend, EmbeddingOwner::Tool, |s| {
                    s.embed_documents(&[&format!("fn round_{round}() {{}}")])
                })
                .unwrap();
            })
            .join()
            .unwrap();
            readings.push(vram_ledger::read_vram().process_bytes.unwrap() >> 20);
        }
        // Same thread twice more.
        for _ in 0..2 {
            with_session(&backend, EmbeddingOwner::Tool, |s| {
                s.embed_documents(&["x"])
            })
            .unwrap();
            readings.push(vram_ledger::read_vram().process_bytes.unwrap() >> 20);
        }
        let price = vram_ledger::known_session_charge("local-gpu-bge").unwrap() >> 20;
        eprintln!(
            "price {price} MiB; after warm-up, then after each run (8 fresh threads, 2 on this one): {readings:?}"
        );
        // The price follows the process up to its steady weight.
        assert!(
            price >= *readings.iter().max().unwrap(),
            "price {price} below {readings:?}"
        );
    }

    #[test]
    fn budget_without_a_process_meter() {
        assert!(
            VramBudget::Explicit { bytes: 1 }
                .admit("x", None, None)
                .is_err()
        );
        VramBudget::HalfCard { bytes: 1 }
            .admit("x", None, Some(5))
            .unwrap();
        VramBudget::Disabled
            .admit("x", Some(u64::MAX / 2), Some(u64::MAX / 2))
            .unwrap();
    }
}
