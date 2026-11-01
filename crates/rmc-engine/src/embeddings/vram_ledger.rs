//! Who holds the GPU memory: every ONNX GPU session, by owner, with what it
//! cost and what it gave back.
//!
//! # Why
//!
//! A daemon died on `hip.cpp:155 write_to_gpu: Assertion 'host or
//! is_device_ptr(result.get())' failed` with only itself on the GPU: a
//! device allocation failed inside MIGraphX while it loaded a compiled
//! program. Measured on the live daemon before that: one background
//! incremental pass (three files, a fresh session) took the card from 4.56 to
//! 5.79 GB and never gave it back. Forty sessions had been created in 1 h 40
//! min. Nothing in the process could say which of its parts owned that
//! memory, so the leak was only visible from outside, on a whole-card counter.
//!
//! # What is measured, and how
//!
//! Independent readings, because one of them alone cannot tell "this
//! process leaks" from "a neighbour grew":
//! - **process** (`process_bytes`, the one sessions are charged with) — the
//!   sum of `/sys/class/kfd/kfd/proc/<pid>/vram_<gpu>`: the KFD driver's
//!   count of everything ROCm allocated for this process, queues and
//!   context-save areas included.
//! - **process via DRM** (`process_drm_bytes`) — the sum of `drm-total-vram`
//!   (older kernels: `drm-memory-vram`) over this process's
//!   `/dev/dri/render*` descriptors in `/proc/self/fdinfo`, deduplicated by
//!   `drm-client-id`. Kept as a cross-check, not as the charge: it sees only
//!   buffer objects of the render node and measured 5 % short of the card
//!   (1.376 against 1.454 GB for one session's first run; the live daemon:
//!   4864 against KFD's 4987 MiB).
//! - **card** — the sum of `mem_info_vram_used` over every DRM card in sysfs:
//!   everything on the card, every process and the display included.
//!
//! A session is charged twice: the delta across its creation, and the delta
//! across its FIRST run — MIGraphX loads (or compiles) the program inside the
//! first `Session::run`, which is where the crash stack was, not at session
//! creation. On release the ledger records what came back. Charged minus
//! returned, per owner, is the leak; process minus the live charges is
//! memory this ledger does not explain (HIP runtime, a kernel cache, or a
//! leak of released sessions).
//!
//! Deltas are differences of whole-process (and whole-card) counters, so two
//! sessions created at the same moment would see each other's allocations.
//! Within the daemon the GPU runs are serialised by the `gpu_lock` flock, but
//! creation is not; the number is an attribution of observed deltas, not an
//! allocator hook. It is exact on a quiet process, which is what the oracle
//! test checks.
//!
//! Off Linux (or without a DRM driver) both readings are `None`: the ledger
//! still counts sessions, and says it cannot weigh them rather than reporting
//! a zero.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Log target of every ledger and broker line. Named so the daemon's default
/// filter can enable it by reference: an unnamed `embeddings::*` target sat
/// below the default `warn` and none of these lines reached the daemon log.
pub const VRAM_LOG_TARGET: &str = "embeddings::vram";

/// Which part of the program a GPU session belongs to.
///
/// Required at every construction of an [`super::EmbeddingGenerator`]: the
/// question "who created forty sessions" has to be answerable from the
/// ledger, not reconstructed from stacks afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingOwner {
    /// An index pass (`index_codebase`, the background incremental sync).
    Index,
    /// `search` and the other hybrid-search tools (cached per workspace).
    Search,
    /// The hypergraph's embedding cache.
    GraphCache,
    /// Prompt reranking inside `build_codemap`.
    CodemapRerank,
    /// The execution-provider census probe.
    ProviderCensus,
    /// Examples, benchmarks and probes outside the daemon.
    Tool,
}

/// One reading of both meters. `None` = this meter does not exist here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VramReading {
    /// KFD's per-process count.
    pub process_bytes: Option<u64>,
    /// DRM fdinfo's per-process count (cross-check).
    pub process_drm_bytes: Option<u64>,
    pub card_used_bytes: Option<u64>,
}

/// Difference of two readings, `after - before`, per meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VramDelta {
    pub process_bytes: Option<i64>,
    pub process_drm_bytes: Option<i64>,
    pub card_bytes: Option<i64>,
}

impl VramDelta {
    /// Nothing moved, on every meter. Not `Default`: a derived default would
    /// be "every meter missing", which is a different statement.
    pub const ZERO: Self = Self {
        process_bytes: Some(0),
        process_drm_bytes: Some(0),
        card_bytes: Some(0),
    };

    fn between(before: VramReading, after: VramReading) -> Self {
        fn diff(b: Option<u64>, a: Option<u64>) -> Option<i64> {
            Some(a? as i64 - b? as i64)
        }
        Self {
            process_bytes: diff(before.process_bytes, after.process_bytes),
            process_drm_bytes: diff(before.process_drm_bytes, after.process_drm_bytes),
            card_bytes: diff(before.card_used_bytes, after.card_used_bytes),
        }
    }

    fn plus(self, other: Self) -> Self {
        fn add(a: Option<i64>, b: Option<i64>) -> Option<i64> {
            Some(a? + b?)
        }
        Self {
            process_bytes: add(self.process_bytes, other.process_bytes),
            process_drm_bytes: add(self.process_drm_bytes, other.process_drm_bytes),
            card_bytes: add(self.card_bytes, other.card_bytes),
        }
    }

    fn negated(self) -> Self {
        Self {
            process_bytes: self.process_bytes.map(|v| -v),
            process_drm_bytes: self.process_drm_bytes.map(|v| -v),
            card_bytes: self.card_bytes.map(|v| -v),
        }
    }
}

/// Read every meter now.
pub fn read_vram() -> VramReading {
    VramReading {
        process_bytes: process_kfd_vram_bytes(),
        process_drm_bytes: process_drm_vram_bytes(),
        card_used_bytes: card_vram_bytes("mem_info_vram_used"),
    }
}

/// Sum of `vram_<gpu_id>` in this process's KFD directory. The directory
/// appears when the process first opens `/dev/kfd`; before that the process
/// holds nothing, which is a zero, not a missing meter. Without the KFD
/// class at all (no ROCm driver) the meter does not exist.
#[cfg(target_os = "linux")]
fn process_kfd_vram_bytes() -> Option<u64> {
    let root = std::path::Path::new("/sys/class/kfd/kfd/proc");
    if !root.is_dir() {
        return None;
    }
    let Ok(entries) = std::fs::read_dir(root.join(std::process::id().to_string())) else {
        return Some(0);
    };
    let mut sum = 0;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("vram_") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).ok()?;
        sum += text.trim().parse::<u64>().ok()?;
    }
    Some(sum)
}

#[cfg(not(target_os = "linux"))]
fn process_kfd_vram_bytes() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn process_drm_vram_bytes() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    let mut clients = std::collections::BTreeMap::<String, u64>::new();
    for entry in entries.flatten() {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if !target.to_string_lossy().starts_with("/dev/dri/render") {
            continue;
        }
        let fdinfo = std::path::Path::new("/proc/self/fdinfo").join(entry.file_name());
        let Ok(text) = std::fs::read_to_string(fdinfo) else {
            // The descriptor closed between the listing and the read.
            continue;
        };
        if let Some((client, bytes)) = parse_drm_fdinfo(&text) {
            clients.insert(client, bytes);
        }
    }
    Some(clients.values().sum())
}

#[cfg(not(target_os = "linux"))]
fn process_drm_vram_bytes() -> Option<u64> {
    None
}

/// `(drm-client-id, VRAM bytes)` of one DRM fdinfo, or `None` if the driver
/// does not report VRAM there.
///
/// `drm-total-vram` is preferred: newer kernels keep `drm-memory-vram` only
/// as a deprecated alias with the same value.
fn parse_drm_fdinfo(text: &str) -> Option<(String, u64)> {
    let mut client = None;
    let mut total = None;
    let mut legacy = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "drm-client-id" => client = Some(value.to_string()),
            "drm-total-vram" => total = parse_kib(value),
            "drm-memory-vram" => legacy = parse_kib(value),
            _ => {}
        }
    }
    Some((client?, total.or(legacy)?))
}

/// `"3779940 KiB"` → bytes. DRM fdinfo also allows `MiB` and a bare byte count.
fn parse_kib(value: &str) -> Option<u64> {
    let mut parts = value.split_whitespace();
    let number: u64 = parts.next()?.parse().ok()?;
    match parts.next() {
        None => Some(number),
        Some("KiB") => Some(number * 1024),
        Some("MiB") => Some(number * 1024 * 1024),
        Some(_) => None,
    }
}

/// Size of the card(s): `mem_info_vram_total` summed like the used meter.
pub(super) fn card_total_bytes() -> Option<u64> {
    card_vram_bytes("mem_info_vram_total")
}

/// The card a GPU session will be created on, found through KFD.
///
/// The sums above are the wrong thing to compare a session's price with: on
/// the reference machine they include an integrated GPU, whose free 480 MiB
/// would be counted as room on the discrete card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceCard {
    /// HIP device ordinal the execution provider uses.
    pub ordinal: usize,
    /// KFD `gpu_id`: the suffix of this card's `vram_<gpu_id>` in the
    /// per-process KFD directory.
    pub gpu_id: u32,
    /// PCI address, `0000:03:00.0`.
    pub pci: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

impl DeviceCard {
    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }
}

/// The card behind HIP device `ordinal`, with a fresh reading of its memory.
///
/// HIP numbers the GPU agents in KFD topology order (`nodes/<n>` with a
/// non-zero `gpu_id`, ascending `n`), after `ROCR_VISIBLE_DEVICES` and then
/// `HIP_VISIBLE_DEVICES` select among them. `None` = no KFD, a visibility
/// list that is not plain indices (UUIDs are not resolved here), or no DRM
/// card at that PCI address: the device cannot be located, which the caller
/// reports as such.
#[cfg(target_os = "linux")]
pub fn device_card(ordinal: usize) -> Option<DeviceCard> {
    let nodes_dir = std::path::Path::new("/sys/class/kfd/kfd/topology/nodes");
    let mut nodes: Vec<(u32, u32, u64)> = Vec::new(); // (node, gpu_id, location_id)
    for entry in std::fs::read_dir(nodes_dir).ok()?.flatten() {
        let Ok(node) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let gpu_id: u32 = std::fs::read_to_string(entry.path().join("gpu_id"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        if gpu_id == 0 {
            continue; // a CPU node
        }
        let properties = std::fs::read_to_string(entry.path().join("properties")).ok()?;
        nodes.push((node, gpu_id, kfd_property(&properties, "location_id")?));
    }
    nodes.sort_unstable();
    let mut agents: Vec<(u32, u64)> = nodes.into_iter().map(|(_, g, l)| (g, l)).collect();
    for var in ["ROCR_VISIBLE_DEVICES", "HIP_VISIBLE_DEVICES"] {
        if let Some(list) = std::env::var(var).ok().filter(|v| !v.trim().is_empty()) {
            agents = select_visible(&agents, &list)?;
        }
    }
    let &(gpu_id, location) = agents.get(ordinal)?;
    // `location_id` is the PCI bus/device/function, `bus << 8 | dev << 3 | fn`.
    let pci_tail = format!(
        "{:02x}:{:02x}.{:x}",
        (location >> 8) & 0xff,
        (location >> 3) & 0x1f,
        location & 0x7
    );
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device = entry.path().join("device");
        let Ok(target) = std::fs::canonicalize(&device) else {
            continue;
        };
        let pci = target.file_name()?.to_string_lossy().to_string();
        if !pci.ends_with(&pci_tail) {
            continue;
        }
        let read = |counter: &str| -> Option<u64> {
            std::fs::read_to_string(device.join(counter))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        return Some(DeviceCard {
            ordinal,
            gpu_id,
            pci,
            total_bytes: read("mem_info_vram_total")?,
            used_bytes: read("mem_info_vram_used")?,
        });
    }
    None
}

#[cfg(not(target_os = "linux"))]
pub fn device_card(_ordinal: usize) -> Option<DeviceCard> {
    None
}

/// `location_id 768` from a KFD node's `properties`.
fn kfd_property(properties: &str, key: &str) -> Option<u64> {
    properties.lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok()).flatten()
    })
}

/// Apply a `*_VISIBLE_DEVICES` list of indices; `None` if it is anything else.
fn select_visible<T: Copy>(agents: &[T], list: &str) -> Option<Vec<T>> {
    list.split(',')
        .map(|i| agents.get(i.trim().parse::<usize>().ok()?).copied())
        .collect()
}

/// This process's KFD VRAM by `gpu_id`.
#[cfg(target_os = "linux")]
pub(super) fn process_kfd_vram_by_gpu() -> std::collections::BTreeMap<u32, u64> {
    let dir = std::path::Path::new("/sys/class/kfd/kfd/proc").join(std::process::id().to_string());
    let mut by_gpu = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(gpu) = name
            .to_string_lossy()
            .strip_prefix("vram_")
            .and_then(|g| g.parse().ok())
        else {
            continue;
        };
        if let Some(bytes) = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok())
        {
            by_gpu.insert(gpu, bytes);
        }
    }
    by_gpu
}

#[cfg(not(target_os = "linux"))]
pub(super) fn process_kfd_vram_by_gpu() -> std::collections::BTreeMap<u32, u64> {
    std::collections::BTreeMap::new()
}

/// Sum of one amdgpu sysfs counter over every DRM card that has it.
#[cfg(target_os = "linux")]
fn card_vram_bytes(counter: &str) -> Option<u64> {
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    let mut sum = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `card0`, not `card0-DP-1`: connectors carry no memory counters.
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let path = entry.path().join("device").join(counter);
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(bytes) = text.trim().parse::<u64>()
        {
            *sum.get_or_insert(0) += bytes;
        }
    }
    sum
}

#[cfg(not(target_os = "linux"))]
fn card_vram_bytes(_counter: &str) -> Option<u64> {
    None
}

/// A live GPU session as the ledger holds it.
#[derive(Debug, Clone)]
struct LiveSession {
    id: u64,
    owner: EmbeddingOwner,
    profile: String,
    created_unix_s: u64,
    at_creation: VramDelta,
    /// `None` until the first run finished.
    at_first_run: Option<VramDelta>,
}

impl LiveSession {
    fn charge(&self) -> VramDelta {
        self.at_creation
            .plus(self.at_first_run.unwrap_or(VramDelta::ZERO))
    }
}

#[derive(Debug, Clone)]
struct OwnerTotals {
    created: u64,
    released: u64,
    /// Sum of charges of released sessions.
    released_charge: VramDelta,
    /// Sum of what came back when they were released (positive = freed).
    released_returned: VramDelta,
    /// Runs granted to this owner by the broker, and how long they waited.
    grants: u64,
    grant_wait_ms: u64,
}

impl OwnerTotals {
    const NONE_YET: Self = Self {
        created: 0,
        released: 0,
        released_charge: VramDelta::ZERO,
        released_returned: VramDelta::ZERO,
        grants: 0,
        grant_wait_ms: 0,
    };
}

#[derive(Debug, Default)]
struct Ledger {
    next_id: u64,
    live: Vec<LiveSession>,
    owners: std::collections::BTreeMap<EmbeddingOwner, OwnerTotals>,
    /// The largest full charge (creation + first run, KFD meter; the whole
    /// process for its only session) seen for a session of each profile:
    /// what the broker expects the next one to cost.
    max_charge_by_profile: std::collections::BTreeMap<String, u64>,
}

/// The process has one GPU and one set of meters, so one ledger.
static LEDGER: Mutex<Ledger> = Mutex::new(Ledger {
    next_id: 0,
    live: Vec::new(),
    owners: std::collections::BTreeMap::new(),
    max_charge_by_profile: std::collections::BTreeMap::new(),
});

fn ledger() -> std::sync::MutexGuard<'static, Ledger> {
    LEDGER.lock().unwrap()
}

fn now_unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn mib(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => format!("{:.0}", b as f64 / (1024.0 * 1024.0)),
        None => "n/a".to_string(),
    }
}

/// The ledger's handle on one live session. Owned next to the session and
/// dropped right after it; see [`VramTicket::release`].
#[derive(Debug)]
pub(super) struct VramTicket {
    id: u64,
    owner: EmbeddingOwner,
    first_run_done: bool,
    /// Reading taken just before the session itself was dropped.
    before_release: Option<VramReading>,
}

impl VramTicket {
    /// Record a session that has just been created; `before` was read
    /// before its construction started.
    pub(super) fn open(owner: EmbeddingOwner, profile: &str, before: VramReading) -> Self {
        let after = read_vram();
        let at_creation = VramDelta::between(before, after);
        let mut ledger = ledger();
        let id = ledger.next_id;
        ledger.next_id += 1;
        ledger.live.push(LiveSession {
            id,
            owner,
            profile: profile.to_string(),
            created_unix_s: now_unix_s(),
            at_creation,
            at_first_run: None,
        });
        ledger
            .owners
            .entry(owner)
            .or_insert(OwnerTotals::NONE_YET)
            .created += 1;
        let live = ledger.live.len();
        drop(ledger);
        tracing::info!(
            target: VRAM_LOG_TARGET,
            session = id,
            owner = ?owner,
            profile,
            live_sessions = live,
            process_before_mib = %mib(before.process_bytes),
            process_after_mib = %mib(after.process_bytes),
            card_before_mib = %mib(before.card_used_bytes),
            card_after_mib = %mib(after.card_used_bytes),
            "GPU embedding session created"
        );
        Self {
            id,
            owner,
            first_run_done: false,
            before_release: None,
        }
    }

    /// Whether the next run is the first one; read it before the run, pass
    /// the reading to [`Self::first_run_finished`] after.
    pub(super) fn first_run_pending(&self) -> bool {
        !self.first_run_done
    }

    pub(super) fn first_run_finished(&mut self, before: VramReading) {
        let after = read_vram();
        let delta = VramDelta::between(before, after);
        self.first_run_done = true;
        let mut ledger = ledger();
        let session = ledger.live.iter_mut().find(|s| s.id == self.id).unwrap();
        session.at_first_run = Some(delta);
        let charge = session.charge();
        let profile = session.profile.clone();
        // A negative charge (a neighbour freed memory inside the window) says
        // nothing about the price; it is not recorded.
        // What the card has to have free for such a session. A process's
        // only session is priced at the whole process: it brings up the HIP
        // runtime too, and part of that lands outside the creation and
        // first-run windows (measured: charge 1386, process 1436 MiB). A
        // fresh daemon's first session is exactly the case the price is for.
        let only_session = ledger.live.len() == 1;
        let price = match (charge.process_bytes, after.process_bytes) {
            (Some(charge), Some(process)) if only_session => Some(charge.max(process as i64)),
            (charge, _) => charge,
        };
        let mut grew = None;
        if let Some(bytes) = price.and_then(|b| u64::try_from(b).ok()) {
            let known = ledger
                .max_charge_by_profile
                .entry(profile.clone())
                .or_insert(0);
            if bytes > *known {
                *known = bytes;
                grew = Some(bytes);
            }
        }
        drop(ledger);
        if let Some(bytes) = grew {
            remember_price(&profile, bytes);
        }
        tracing::info!(
            target: VRAM_LOG_TARGET,
            session = self.id,
            owner = ?self.owner,
            first_run_process_bytes = ?delta.process_bytes,
            first_run_card_bytes = ?delta.card_bytes,
            charge_process_bytes = ?charge.process_bytes,
            charge_card_bytes = ?charge.card_bytes,
            "GPU embedding session: first run"
        );
    }

    /// Called right before the session is dropped. The ticket itself is
    /// dropped after the session, and only then can it see what came back.
    pub(super) fn release_starting(&mut self) {
        self.before_release = Some(read_vram());
    }
}

impl Drop for VramTicket {
    fn drop(&mut self) {
        let after = read_vram();
        // Every ticket lives inside a session whose `Drop` reads the meters
        // first; a ticket released any other way is a wiring bug.
        let returned = VramDelta::between(self.before_release.unwrap(), after).negated();
        let mut ledger = ledger();
        let index = ledger.live.iter().position(|s| s.id == self.id).unwrap();
        let session = ledger.live.swap_remove(index);
        let totals = ledger.owners.get_mut(&session.owner).unwrap();
        totals.released += 1;
        totals.released_charge = totals.released_charge.plus(session.charge());
        totals.released_returned = totals.released_returned.plus(returned);
        let live = ledger.live.len();
        drop(ledger);
        tracing::info!(
            target: VRAM_LOG_TARGET,
            session = self.id,
            owner = ?session.owner,
            live_sessions = live,
            charge_process_bytes = ?session.charge().process_bytes,
            returned_process_bytes = ?returned.process_bytes,
            returned_card_bytes = ?returned.card_bytes,
            process_after_mib = %mib(after.process_bytes),
            card_after_mib = %mib(after.card_used_bytes),
            "GPU embedding session released"
        );
    }
}

/// What a fully loaded session of `profile` costs: the largest charge
/// measured in this process, or, before the first one, the largest one an
/// earlier process wrote down (see [`remember_price`]). `None` = never
/// measured on this machine.
pub(super) fn known_session_charge(profile: &str) -> Option<u64> {
    let here = ledger().max_charge_by_profile.get(profile).copied();
    let before = remembered_price(profile);
    here.max(before)
}

/// After any run of the only live session: raise the price of `profile` to
/// what the whole process holds now.
///
/// Why after every run and not only the first: runs after the warm-up still
/// allocate, a little and a bounded number of times — measured,
/// `runs_after_warm_up_by_thread`: 1386 after the warm-up, +25 MiB on the
/// first run from another thread and once more a few runs later, then flat
/// at 1436 for good (the oracle's 50 passes). That is the process's steady
/// weight, and it is what the next process's first session has to find free
/// on the card. Two sysfs reads per run; a run is one batch.
pub(super) fn observe_after_run(profile: &str) {
    let Some(process) = process_kfd_vram_bytes() else {
        return;
    };
    let mut ledger = ledger();
    if ledger.live.len() != 1 {
        return;
    }
    let known = ledger
        .max_charge_by_profile
        .entry(profile.to_string())
        .or_insert(0);
    if process <= *known {
        return;
    }
    *known = process;
    drop(ledger);
    remember_price(profile, process);
}

/// Make this process believe a session of `profile` costs `bytes`.
#[cfg(test)]
pub(super) fn set_price_for_test(profile: &str, bytes: u64) {
    ledger()
        .max_charge_by_profile
        .insert(profile.to_string(), bytes);
}

/// Where the price of a session of `profile` is kept between processes: the
/// same cache root as the models and the MIGraphX kernels.
///
/// Why it is kept at all: the first session of a daemon is the one that must
/// be checked against the card's free memory, and without a price from an
/// earlier run there is nothing to check it against.
fn price_path(profile: &str) -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "rust-code-mcp").map(|d| {
        d.cache_dir()
            .join("vram")
            .join(format!("{profile}.session_bytes"))
    })
}

fn remembered_price(profile: &str) -> Option<u64> {
    std::fs::read_to_string(price_path(profile)?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Write the price down if it is above what is there. Zero is never written:
/// it is what a machine without the KFD meter measures, not a price.
/// Best effort: a read-only cache costs only the check of the next process's
/// first session.
fn remember_price(profile: &str, bytes: u64) {
    let Some(path) = price_path(profile) else {
        return;
    };
    if bytes == 0 || remembered_price(profile).is_some_and(|known| known >= bytes) {
        return;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, bytes.to_string()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&tmp);
        tracing::warn!(
            target: VRAM_LOG_TARGET,
            path = %path.display(),
            %error,
            "cannot write down the price of a GPU session"
        );
    }
}

/// One run granted by the broker to `owner`, after waiting `waited`.
pub(super) fn record_grant(owner: EmbeddingOwner, waited: std::time::Duration) {
    let mut ledger = ledger();
    let totals = ledger.owners.entry(owner).or_insert(OwnerTotals::NONE_YET);
    totals.grants += 1;
    totals.grant_wait_ms += waited.as_millis() as u64;
}

/// One live session in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveSessionReport {
    pub id: u64,
    pub owner: EmbeddingOwner,
    pub profile: String,
    pub age_secs: u64,
    pub at_creation: VramDelta,
    /// `None` = not run yet; the program is not on the GPU yet either.
    pub at_first_run: Option<VramDelta>,
    pub charge: VramDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerReport {
    pub owner: EmbeddingOwner,
    pub live: u64,
    pub created: u64,
    pub released: u64,
    pub live_charge: VramDelta,
    pub released_charge: VramDelta,
    pub released_returned: VramDelta,
    /// `released_charge - released_returned`: what released sessions kept.
    pub released_kept: VramDelta,
    /// Runs the broker granted to this owner, and their summed wait.
    pub grants: u64,
    pub grant_wait_ms: u64,
}

/// The `gpu` section of `runtime_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GpuLedgerReport {
    pub live_sessions: Vec<LiveSessionReport>,
    pub owners: Vec<OwnerReport>,
    pub now: VramReading,
    pub card_total_bytes: Option<u64>,
    /// Sum of live charges.
    pub live_charge: VramDelta,
    /// Process VRAM that no live session's charge explains.
    pub unaccounted_process_bytes: Option<i64>,
    /// Card VRAM held by everything that is not this process.
    pub other_than_this_process_bytes: Option<i64>,
    /// Who holds which shared session right now, and the VRAM budget.
    pub broker: super::gpu_broker::BrokerReport,
}

/// Snapshot the ledger together with a fresh reading of both meters.
pub fn gpu_ledger_report() -> GpuLedgerReport {
    let now = read_vram();
    let card_total_bytes = card_vram_bytes("mem_info_vram_total");
    let ledger = ledger();
    let now_s = now_unix_s();
    let live_sessions: Vec<LiveSessionReport> = ledger
        .live
        .iter()
        .map(|s| LiveSessionReport {
            id: s.id,
            owner: s.owner,
            profile: s.profile.clone(),
            age_secs: now_s.saturating_sub(s.created_unix_s),
            at_creation: s.at_creation,
            at_first_run: s.at_first_run,
            charge: s.charge(),
        })
        .collect();
    let live_charge = live_sessions
        .iter()
        .fold(VramDelta::ZERO, |acc, s| acc.plus(s.charge));
    let owners = ledger
        .owners
        .iter()
        .map(|(owner, totals)| {
            let live: Vec<&LiveSessionReport> =
                live_sessions.iter().filter(|s| s.owner == *owner).collect();
            OwnerReport {
                owner: *owner,
                live: live.len() as u64,
                created: totals.created,
                released: totals.released,
                live_charge: live
                    .iter()
                    .fold(VramDelta::ZERO, |acc, s| acc.plus(s.charge)),
                released_charge: totals.released_charge,
                released_returned: totals.released_returned,
                released_kept: totals
                    .released_charge
                    .plus(totals.released_returned.negated()),
                grants: totals.grants,
                grant_wait_ms: totals.grant_wait_ms,
            }
        })
        .collect();
    drop(ledger);
    GpuLedgerReport {
        broker: super::gpu_broker::broker_report(card_total_bytes),
        unaccounted_process_bytes: (|| {
            Some(now.process_bytes? as i64 - live_charge.process_bytes?)
        })(),
        other_than_this_process_bytes: (|| {
            Some(now.card_used_bytes? as i64 - now.process_bytes? as i64)
        })(),
        live_sessions,
        owners,
        now,
        card_total_bytes,
        live_charge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMDGPU_FDINFO: &str = "pos:\t0\nflags:\t02100002\ndrm-driver:\tamdgpu\n\
        drm-client-id:\t945\ndrm-pdev:\t0000:03:00.0\ndrm-total-gtt:\t8288 KiB\n\
        drm-total-vram:\t3779940 KiB\ndrm-resident-vram:\t3779940 KiB\n\
        drm-memory-vram:\t3779940 KiB\ndrm-memory-gtt: \t8272 KiB\n";

    #[test]
    fn fdinfo_vram_is_read_in_bytes_with_its_client() {
        assert_eq!(
            parse_drm_fdinfo(AMDGPU_FDINFO),
            Some(("945".to_string(), 3_779_940 * 1024))
        );
    }

    /// GTT is host memory mapped for the GPU; reading it as VRAM would
    /// charge sessions for the wrong pool.
    #[test]
    fn fdinfo_without_vram_is_not_a_zero() {
        let only_gtt = "drm-client-id:\t1\ndrm-total-gtt:\t8288 KiB\n";
        assert_eq!(parse_drm_fdinfo(only_gtt), None);
    }

    #[test]
    fn legacy_key_is_used_only_without_the_new_one() {
        let legacy = "drm-client-id:\t7\ndrm-memory-vram:\t2 MiB\n";
        assert_eq!(parse_drm_fdinfo(legacy), Some(("7".into(), 2 << 20)));
        let both = "drm-client-id:\t7\ndrm-memory-vram:\t2 MiB\ndrm-total-vram:\t3 MiB\n";
        assert_eq!(parse_drm_fdinfo(both), Some(("7".into(), 3 << 20)));
    }

    #[test]
    fn kfd_location_and_visibility_lists() {
        let properties = "cpu_cores_count 0\nsimd_count 108\nlocation_id 768\ndomain 0\n";
        assert_eq!(kfd_property(properties, "location_id"), Some(768));
        assert_eq!(kfd_property(properties, "location"), None);
        let agents = [10, 20, 30];
        assert_eq!(select_visible(&agents, "2, 0"), Some(vec![30, 10]));
        assert_eq!(select_visible(&agents, "3"), None);
        assert_eq!(select_visible(&agents, "GPU-1234"), None);
    }

    #[test]
    fn unknown_unit_is_refused() {
        assert_eq!(parse_kib("5 GiB"), None);
        assert_eq!(parse_kib("4096"), Some(4096));
    }

    #[test]
    fn delta_of_a_missing_meter_stays_missing() {
        let before = VramReading {
            process_bytes: Some(10),
            process_drm_bytes: Some(9),
            card_used_bytes: None,
        };
        let after = VramReading {
            process_bytes: Some(4),
            process_drm_bytes: None,
            card_used_bytes: Some(1),
        };
        let d = VramDelta::between(before, after);
        assert_eq!(d.process_bytes, Some(-6));
        assert_eq!(d.card_bytes, None);
    }

    /// The bookkeeping itself, without a GPU: open, first run, release.
    /// Owner `Tool`, so a daemon test running alongside cannot be confused
    /// with it; counts are compared as differences for the same reason.
    #[test]
    fn ticket_lifecycle_moves_owner_counters() {
        let counts = || {
            let r = gpu_ledger_report();
            r.owners
                .iter()
                .find(|o| o.owner == EmbeddingOwner::Tool)
                .map(|o| (o.live, o.created, o.released))
                .unwrap_or((0, 0, 0))
        };
        let (live0, created0, released0) = counts();
        let mut ticket = VramTicket::open(EmbeddingOwner::Tool, "unit", read_vram());
        assert!(ticket.first_run_pending());
        ticket.first_run_finished(read_vram());
        assert!(!ticket.first_run_pending());
        assert_eq!(counts(), (live0 + 1, created0 + 1, released0));
        ticket.release_starting();
        drop(ticket);
        assert_eq!(counts(), (live0, created0 + 1, released0 + 1));
    }
}
