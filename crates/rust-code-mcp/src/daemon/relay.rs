//! The client's pipe to the daemon, one JSON-RPC line at a time, and the reason
//! it is not a byte copy any more: a session must outlive the daemon serving it.
//!
//! # Why
//!
//! The byte-level proxy this replaces remembered nothing, so the daemon a
//! session first reached was the only one it could ever use. Two things follow,
//! and both were measured on the machine this runs on:
//!
//! - **A rebuilt binary never reached running sessions.** The socket key covers
//!   the binary, so a new build starts a new daemon for NEW sessions, while
//!   every session already attached keeps the old one alive — on 2026-10-21 two
//!   daemons of a deleted binary held ≈ 30 GB between them (one of them with
//!   twelve clients since 05:00), next to the current daemon.
//! - **A lost daemon took the session's tools with it.** A crash (two GPU
//!   aborts in one night), the OOM killer, or a retired daemon's deadline closed
//!   the socket, and the session had no MCP server until it was restarted.
//!
//! # What the relay remembers
//!
//! Exactly what a new daemon needs to continue the conversation: the host's
//! `initialize` request and `notifications/initialized`, and every request
//! still waiting for its answer. On a new connection it replays the handshake
//! (swallowing the answer the host already has) and then the requests the host
//! is still waiting for — or, for the ones it must not repeat, answers them with
//! an error naming why. Lines pass through byte for byte; parsing only reads them.
//!
//! # When it moves
//!
//! - The connection is lost (EOF or an error on the socket): at once.
//! - The daemon was superseded — its socket file is gone or is someone else's
//!   now (a retired daemon unlinks it), or the binary at the launch path is not
//!   the one the socket key was made from: on the next tick, and only while no
//!   request is in flight. Moving between requests costs the host nothing;
//!   moving under a request would repeat it.

use super::BoxError;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{Instant, MissedTickBehavior};

/// Tools whose call changes state on the server: a write to the index, a cache
/// wipe, a runtime clear, a persisted graph. Repeating one on a new daemon after
/// the old one vanished mid-call would do its work a second time without the
/// host having asked for that, so such a call is answered with an error that
/// says what happened instead. Every other tool reads, and is repeated once.
///
/// `state_changing_tools_exist` checks every name here against the server's
/// tool list, so a rename cannot silently make a mutating tool retryable.
pub(super) const STATE_CHANGING_TOOLS: [&str; 4] = [
    "index_codebase",
    "clear_cache",
    "clear_runtime",
    "build_hypergraph",
];

/// How many consecutive connections may die without delivering a single line
/// before the relay gives up. A daemon that crashes while starting (a broken
/// build, a GPU that refuses the model) would otherwise be respawned forever.
const QUICK_LOSSES_BEFORE_GIVING_UP: u32 = 3;

/// JSON-RPC error code for the answers the relay writes itself: the
/// implementation-defined server-error range.
const LOST_DAEMON_ERROR: i64 = -32000;

/// Where the relay gets its daemon from.
pub(super) trait Dialer {
    /// A connection to the daemon that should serve this session now, starting
    /// one when nobody listens. `None` — none could be reached or started.
    async fn dial(&mut self) -> Option<UnixStream>;

    /// Whether the daemon behind the current connection has been superseded and
    /// the session should move once nothing is in flight.
    fn superseded(&mut self) -> bool;
}

/// One JSON-RPC line, as far as the relay cares.
#[derive(Debug, Clone, PartialEq)]
enum Frame {
    /// Carries a method and an id: someone waits for an answer.
    Request {
        id: Value,
        method: String,
        /// `params.name` of a `tools/call`.
        tool: Option<String>,
    },
    /// A method without an id.
    Notification {
        method: String,
        params: Option<Value>,
    },
    /// An id without a method: the answer to a request.
    Response { id: Value },
    /// Anything else — not JSON, a batch array, an error answer with a null id.
    /// Forwarded untouched and not tracked. Batches are gone from MCP since
    /// 2025-06-18 and no host this serves sends them.
    Opaque,
}

fn classify(line: &[u8]) -> Frame {
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(line) else {
        return Frame::Opaque;
    };
    let id = map.remove("id").filter(|id| !id.is_null());
    let method = match map.remove("method") {
        Some(Value::String(method)) => Some(method),
        _ => None,
    };
    let params = map.remove("params");
    match (method, id) {
        (Some(method), Some(id)) => {
            let tool = (method == "tools/call")
                .then(|| params.as_ref()?.get("name")?.as_str().map(str::to_owned))
                .flatten();
            Frame::Request { id, method, tool }
        }
        (Some(method), None) => Frame::Notification { method, params },
        (None, Some(id)) => Frame::Response { id },
        (None, None) => Frame::Opaque,
    }
}

/// A request the host is still waiting for.
#[derive(Debug)]
struct Pending {
    id: Value,
    line: Vec<u8>,
    /// `Some(tool)` when the request must not be repeated on a new daemon.
    state_changing: Option<String>,
    /// How many daemons this request has been sent to.
    sent: u32,
}

#[derive(Debug)]
struct Handshake {
    id: Value,
    line: Vec<u8>,
    answered: bool,
}

/// What to write after a reconnect: lines for the new daemon, and the answers
/// the relay gives the host itself for requests it will not repeat.
#[derive(Debug, Default)]
struct Replay {
    to_daemon: Vec<Vec<u8>>,
    to_host: Vec<Vec<u8>>,
}

/// The conversation as far as a new daemon needs it. Pure: no I/O, so the
/// replay decisions are tested without sockets.
#[derive(Debug, Default)]
struct Conversation {
    handshake: Option<Handshake>,
    initialized: Option<Vec<u8>>,
    pending: Vec<Pending>,
    /// Id of a replayed `initialize` whose answer the host already has.
    swallow: Option<Value>,
    /// Whether the host has sent anything at all. Before that the session can
    /// still be served in-process if no daemon can be reached.
    consumed: bool,
}

impl Conversation {
    fn from_host(&mut self, line: &[u8]) {
        self.consumed = true;
        match classify(line) {
            Frame::Request { id, method, tool } => {
                if method == "initialize" {
                    self.handshake = Some(Handshake {
                        id: id.clone(),
                        line: line.to_vec(),
                        answered: false,
                    });
                }
                let state_changing =
                    tool.filter(|tool| STATE_CHANGING_TOOLS.contains(&tool.as_str()));
                self.pending.push(Pending {
                    id,
                    line: line.to_vec(),
                    state_changing,
                    sent: 1,
                });
            }
            Frame::Notification { method, params } => match method.as_str() {
                "notifications/initialized" => self.initialized = Some(line.to_vec()),
                // A cancelled request gets no answer, so without this it would
                // stay "in flight" forever and the session would never move.
                "notifications/cancelled" => {
                    if let Some(id) = params.as_ref().and_then(|p| p.get("requestId")) {
                        self.pending.retain(|pending| &pending.id != id);
                    }
                }
                _ => {}
            },
            Frame::Response { .. } | Frame::Opaque => {}
        }
    }

    /// Record a daemon line; `false` — the host must not see it.
    fn from_daemon(&mut self, line: &[u8]) -> bool {
        let Frame::Response { id } = classify(line) else {
            return true;
        };
        if let Some(handshake) = &mut self.handshake {
            if handshake.id == id {
                handshake.answered = true;
            }
        }
        if self.swallow.as_ref() == Some(&id) {
            self.swallow = None;
            if line_is_error(line) {
                tracing::warn!(
                    "the new daemon refused the replayed handshake: {}",
                    String::from_utf8_lossy(line).trim_end()
                );
            }
            return false;
        }
        self.pending.retain(|pending| pending.id != id);
        true
    }

    /// The lines that carry this conversation over to a new daemon.
    ///
    /// The handshake first, because rmcp reads `initialize` and then
    /// `notifications/initialized` before anything else; its answer is
    /// swallowed when the host already has one. Then every request still in
    /// flight, in the order the host sent them — except those the relay answers
    /// itself: a state-changing tool, and a request that has already been on a
    /// daemon that was lost under it (the request may be what kills them).
    fn replay(&mut self) -> Replay {
        let mut replay = Replay::default();
        let handshake_id = self
            .handshake
            .as_ref()
            .map(|handshake| handshake.id.clone());
        if let Some(handshake) = &self.handshake {
            replay.to_daemon.push(handshake.line.clone());
            self.swallow = handshake.answered.then(|| handshake.id.clone());
            if let Some(initialized) = &self.initialized {
                replay.to_daemon.push(initialized.clone());
            }
        }
        self.pending.retain_mut(|pending| {
            if handshake_id.as_ref() == Some(&pending.id) {
                return true;
            }
            let refusal = match (&pending.state_changing, pending.sent) {
                (Some(tool), _) => Some(format!(
                    "the rust-code-mcp daemon was lost while serving this call, and `{tool}` \
                     changes state, so it was not repeated on the new daemon; check whether it \
                     took effect and call it again if needed"
                )),
                (None, sent) if sent >= 2 => Some(
                    "the rust-code-mcp daemon was lost twice while serving this call; it is not \
                     repeated a third time, because the call itself may be what brings the \
                     daemon down"
                        .to_string(),
                ),
                (None, _) => None,
            };
            match refusal {
                Some(message) => {
                    replay.to_host.push(error_line(&pending.id, &message));
                    false
                }
                None => {
                    pending.sent += 1;
                    replay.to_daemon.push(pending.line.clone());
                    true
                }
            }
        });
        replay
    }

    /// Answers for every request still in flight when no daemon is left.
    fn abandon(&mut self, reason: &str) -> Vec<Vec<u8>> {
        self.pending
            .drain(..)
            .map(|pending| error_line(&pending.id, reason))
            .collect()
    }
}

fn line_is_error(line: &[u8]) -> bool {
    serde_json::from_slice::<Value>(line).is_ok_and(|value| value.get("error").is_some())
}

fn error_line(id: &Value, message: &str) -> Vec<u8> {
    let mut line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": LOST_DAEMON_ERROR, "message": message },
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    line
}

/// One connection to a daemon. The read buffer lives here rather than in the
/// read future: `read_until` is cancel-safe only when the buffer survives the
/// `select!` that dropped it.
struct Link {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    buf: Vec<u8>,
    /// Whether this connection delivered anything; see
    /// [`QUICK_LOSSES_BEFORE_GIVING_UP`].
    delivered: bool,
    /// Whether the host's EOF has been passed on to this daemon.
    half_closed: bool,
}

impl Link {
    fn new(stream: UnixStream) -> Self {
        let (reader, writer) = stream.into_split();
        Self {
            reader: BufReader::new(reader),
            writer,
            buf: Vec::new(),
            delivered: false,
            half_closed: false,
        }
    }

    async fn send_all(&mut self, lines: &[Vec<u8>]) -> std::io::Result<()> {
        for line in lines {
            self.writer.write_all(line).await?;
        }
        Ok(())
    }
}

async fn to_host<O: AsyncWrite + Unpin>(host: &mut O, lines: &[Vec<u8>]) -> std::io::Result<()> {
    for line in lines {
        host.write_all(line).await?;
    }
    host.flush().await
}

/// Pump one session between the host (`input`/`output`) and a daemon, moving
/// to another daemon whenever the current one is lost or superseded.
///
/// Returns [`super::run_client`]'s verdict: `true` when the session ran to the
/// end, `false` when nothing was exchanged and no daemon could be reached, so
/// the caller may still serve it in-process.
///
/// Stdin EOF is passed on only once nothing is owed. A one-shot host writes its
/// requests and closes stdin at once, and rmcp ends a service the moment its
/// input ends, dropping every answer still being computed — the in-process
/// server loses them that way (`RMC_DAEMON=0` with a piped
/// `initialize` + `tools/call` answers the `initialize` only). So the write
/// half is shut down after the last answer, the daemon closes, and the session
/// ends at that close — or `drain` after the half-close, if the daemon never
/// closes.
pub(super) async fn relay<I, O, D>(
    input: I,
    mut output: O,
    first: UnixStream,
    dialer: &mut D,
    tick: Duration,
    drain: Duration,
) -> Result<bool, BoxError>
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
    D: Dialer,
{
    let mut input = BufReader::new(input);
    let mut host_buf: Vec<u8> = Vec::new();
    let mut conversation = Conversation::default();
    let mut link = Link::new(first);
    let mut input_open = true;
    let mut drain_until: Option<Instant> = None;
    let mut quick_losses = 0u32;
    let mut ticker = tokio::time::interval_at(Instant::now() + tick, tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if !input_open && !link.half_closed && conversation.pending.is_empty() {
            link.half_closed = true;
            drain_until = Some(Instant::now() + drain);
            // A failure here is a lost link, and the read side reports it.
            if let Err(e) = link.writer.shutdown().await {
                tracing::debug!("half-closing the daemon connection failed: {e}");
            }
        }

        // What happened this turn, decided inside the `select!` and acted on
        // after it, when no branch future borrows `link` any more.
        enum Turn {
            Continue,
            Lost(String),
            MoveTo(Link),
        }
        let turn = tokio::select! {
            read = input.read_until(b'\n', &mut host_buf), if input_open => match read {
                Ok(0) => {
                    input_open = false;
                    Turn::Continue
                }
                Ok(_) => {
                    let line = std::mem::take(&mut host_buf);
                    conversation.from_host(&line);
                    match link.writer.write_all(&line).await {
                        Ok(()) => Turn::Continue,
                        Err(e) => Turn::Lost(format!("writing to the daemon failed: {e}")),
                    }
                }
                // The host's side is broken; no daemon can fix that.
                Err(e) => {
                    if conversation.consumed {
                        return Err(format!("reading the session's stdin failed: {e}").into());
                    }
                    tracing::warn!("stdin failed before anything was exchanged: {e}");
                    return Ok(false);
                }
            },
            read = link.reader.read_until(b'\n', &mut link.buf) => match read {
                Ok(0) => Turn::Lost("the daemon closed the connection".to_string()),
                // No newline means EOF arrived mid-line: the daemon died while
                // writing, and half an answer is no answer.
                Ok(_) if link.buf.last() != Some(&b'\n') => {
                    link.buf.clear();
                    Turn::Lost("the daemon closed the connection mid-message".to_string())
                }
                Ok(_) => {
                    let line = std::mem::take(&mut link.buf);
                    link.delivered = true;
                    if conversation.from_daemon(&line) {
                        to_host(&mut output, &[line]).await?;
                    }
                    Turn::Continue
                }
                Err(e) => Turn::Lost(format!("the daemon connection failed: {e}")),
            },
            _ = ticker.tick(), if input_open => {
                if conversation.pending.is_empty() && dialer.superseded() {
                    match dialer.dial().await {
                        Some(stream) => {
                            let mut next = Link::new(stream);
                            let replay = conversation.replay();
                            match next.send_all(&replay.to_daemon).await {
                                Ok(()) => Turn::MoveTo(next),
                                Err(e) => {
                                    // Stay where we are: the old daemon still
                                    // answers, the next tick tries again.
                                    conversation.swallow = None;
                                    tracing::warn!("the new daemon dropped the handshake: {e}; staying");
                                    Turn::Continue
                                }
                            }
                        }
                        None => Turn::Continue,
                    }
                } else {
                    Turn::Continue
                }
            }
            _ = tokio::time::sleep_until(drain_until.unwrap_or_else(Instant::now)), if drain_until.is_some() => {
                return Err(format!(
                    "daemon held the connection open for longer than {drain:?} after the \
                     session's last answer and stdin's EOF"
                )
                .into());
            }
        };

        match turn {
            Turn::Continue => {}
            Turn::MoveTo(next) => {
                tracing::info!("the daemon was superseded; this session moved to the current one");
                // Dropping the old link closes it: the old daemon sees this
                // client leave, and exits once the last one has.
                link = next;
                quick_losses = 0;
            }
            Turn::Lost(reason) => {
                if !input_open && conversation.pending.is_empty() {
                    // The ordinary end: every answer arrived, then the close.
                    return Ok(true);
                }
                if !conversation.consumed {
                    tracing::warn!(
                        "daemon session ended with nothing exchanged: {reason}; serving in-process"
                    );
                    return Ok(false);
                }
                quick_losses = if link.delivered { 1 } else { quick_losses + 1 };
                if quick_losses > QUICK_LOSSES_BEFORE_GIVING_UP {
                    let answers = conversation.abandon(&format!(
                        "the rust-code-mcp daemon was lost {QUICK_LOSSES_BEFORE_GIVING_UP} times in a \
                         row without answering anything ({reason}); giving up"
                    ));
                    let _ = to_host(&mut output, &answers).await;
                    return Err(format!("daemon lost repeatedly: {reason}").into());
                }
                tracing::warn!(
                    "{reason}; reconnecting with {} request(s) in flight",
                    conversation.pending.len()
                );
                let Some(stream) = dialer.dial().await else {
                    let answers = conversation.abandon(&format!(
                        "the rust-code-mcp daemon was lost ({reason}) and no successor could be started"
                    ));
                    let _ = to_host(&mut output, &answers).await;
                    return Err(
                        format!("daemon lost and no successor could be reached: {reason}").into(),
                    );
                };
                let replay = conversation.replay();
                to_host(&mut output, &replay.to_host).await?;
                link = Link::new(stream);
                drain_until = None;
                // A failed write shows up on the read side as a lost link, which
                // brings us back here with the attempt counted.
                if let Err(e) = link.send_all(&replay.to_daemon).await {
                    tracing::warn!("replaying the session to the new daemon failed: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(value: Value) -> Vec<u8> {
        let mut line = value.to_string().into_bytes();
        line.push(b'\n');
        line
    }

    fn request(id: u64, method: &str, params: Value) -> Vec<u8> {
        line(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
    }

    fn call(id: u64, tool: &str) -> Vec<u8> {
        request(id, "tools/call", json!({ "name": tool, "arguments": {} }))
    }

    fn answer(id: u64) -> Vec<u8> {
        line(json!({ "jsonrpc": "2.0", "id": id, "result": {} }))
    }

    fn initialized() -> Vec<u8> {
        line(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
    }

    fn ids_of(lines: &[Vec<u8>]) -> Vec<Value> {
        lines
            .iter()
            .map(|line| serde_json::from_slice::<Value>(line).unwrap()["id"].clone())
            .collect()
    }

    /// A conversation past its handshake with requests 2 and 3 in flight.
    fn mid_session(second: &[u8], third: &[u8]) -> Conversation {
        let mut conversation = Conversation::default();
        conversation.from_host(&request(1, "initialize", json!({})));
        assert!(conversation.from_daemon(&answer(1)));
        conversation.from_host(&initialized());
        conversation.from_host(second);
        conversation.from_host(third);
        conversation
    }

    #[test]
    fn frames_are_told_apart_by_method_and_id() {
        assert!(matches!(
            classify(&call(7, "search")),
            Frame::Request { tool: Some(tool), .. } if tool == "search"
        ));
        assert!(matches!(
            classify(&initialized()),
            Frame::Notification { .. }
        ));
        assert_eq!(classify(&answer(7)), Frame::Response { id: json!(7) });
        assert_eq!(classify(b"not json\n"), Frame::Opaque);
        assert_eq!(classify(b"[{\"id\":1}]\n"), Frame::Opaque);
        // A parse-error answer carries `id: null` and answers nobody we track.
        assert_eq!(
            classify(br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"x"}}"#),
            Frame::Opaque
        );
    }

    /// The replay a new daemon needs: handshake, then the reads still owed, in
    /// the order the host sent them; the handshake's answer is swallowed.
    #[test]
    fn a_lost_daemon_gets_the_handshake_and_the_reads_in_flight() {
        let mut conversation = mid_session(&call(2, "search"), &call(3, "find_references"));
        let replay = conversation.replay();

        assert_eq!(
            replay.to_daemon,
            vec![
                request(1, "initialize", json!({})),
                initialized(),
                call(2, "search"),
                call(3, "find_references"),
            ]
        );
        assert!(replay.to_host.is_empty());
        assert!(
            !conversation.from_daemon(&answer(1)),
            "the host already has its initialize answer; a second one is a protocol error"
        );
        assert!(conversation.from_daemon(&answer(2)));
        assert!(conversation.from_daemon(&answer(3)));
        assert!(conversation.pending.is_empty());
    }

    /// Mutation that must fail it: drop the state-changing check, and the
    /// index write is replayed instead of answered.
    #[test]
    fn a_state_changing_call_is_answered_not_repeated() {
        let mut conversation = mid_session(&call(2, "index_codebase"), &call(3, "search"));
        let replay = conversation.replay();

        assert_eq!(ids_of(&replay.to_host), vec![json!(2)]);
        let error = String::from_utf8(replay.to_host[0].clone()).unwrap();
        assert!(
            error.contains("index_codebase") && error.contains("not repeated"),
            "{error}"
        );
        assert_eq!(ids_of(&replay.to_daemon[2..]), vec![json!(3)]);
    }

    /// A read is repeated once; lost a second time, it is answered instead —
    /// otherwise a call that crashes every daemon would crash them forever.
    #[test]
    fn a_read_is_repeated_once_only() {
        let mut conversation = mid_session(&call(2, "search"), &call(3, "search"));
        let first = conversation.replay();
        assert_eq!(ids_of(&first.to_daemon[2..]), vec![json!(2), json!(3)]);
        assert!(!conversation.from_daemon(&answer(1)));
        assert!(conversation.from_daemon(&answer(3)));

        let second = conversation.replay();
        assert_eq!(ids_of(&second.to_host), vec![json!(2)]);
        assert_eq!(
            second.to_daemon.len(),
            2,
            "only the handshake is left to send"
        );
    }

    /// A session lost before its handshake was answered replays `initialize` as
    /// an ordinary request: the host is still waiting for that answer.
    #[test]
    fn an_unanswered_handshake_is_not_swallowed() {
        let mut conversation = Conversation::default();
        conversation.from_host(&request(1, "initialize", json!({})));
        let replay = conversation.replay();

        assert_eq!(replay.to_daemon, vec![request(1, "initialize", json!({}))]);
        assert!(
            conversation.from_daemon(&answer(1)),
            "the host must get this answer"
        );
    }

    /// Without this a cancelled call stays "in flight" forever and the session
    /// never leaves a superseded daemon.
    #[test]
    fn a_cancelled_request_is_no_longer_in_flight() {
        let mut conversation = mid_session(&call(2, "search"), &call(3, "search"));
        conversation.from_host(&line(json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": 2, "reason": "user" }
        })));
        assert_eq!(
            conversation
                .pending
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>(),
            vec![json!(3)]
        );
    }

    /// Every name in the list is a real tool: a rename would otherwise make a
    /// state-changing tool retryable without anyone noticing.
    #[test]
    fn state_changing_tools_exist() {
        let tools = rmc_server::tools::SearchTool::new().tool_names();
        for name in STATE_CHANGING_TOOLS {
            assert!(
                tools.iter().any(|tool| tool == name),
                "{name} is not a tool any more"
            );
        }
    }

    /// A fake daemon: answers every request with `{"result":{"pid":<pid>}}`
    /// after the handshake, and closes after `answers` requests when given a
    /// limit — the crash.
    fn fake_daemon(stream: UnixStream, pid: u64, die_after: Option<usize>) {
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut answered = 0usize;
            while let Ok(Some(line)) = lines.next_line().await {
                let value: Value = serde_json::from_str(&line).unwrap();
                let (Some(id), Some(method)) = (value.get("id"), value.get("method")) else {
                    continue;
                };
                if method != "initialize" {
                    if die_after == Some(answered) {
                        return;
                    }
                    answered += 1;
                }
                let reply = json!({ "jsonrpc": "2.0", "id": id, "result": { "pid": pid } });
                let mut bytes = reply.to_string().into_bytes();
                bytes.push(b'\n');
                if writer.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        });
    }

    /// Hands out fake daemons with pids 1, 2, …; `die_after[n]` is daemon n's limit.
    struct FakeDialer {
        next_pid: u64,
        die_after: Vec<Option<usize>>,
        superseded: bool,
    }

    impl FakeDialer {
        fn connect(&mut self) -> UnixStream {
            let (ours, theirs) = UnixStream::pair().unwrap();
            let limit = self
                .die_after
                .get(self.next_pid as usize - 1)
                .copied()
                .flatten();
            fake_daemon(theirs, self.next_pid, limit);
            self.next_pid += 1;
            ours
        }
    }

    impl Dialer for FakeDialer {
        async fn dial(&mut self) -> Option<UnixStream> {
            self.superseded = false;
            Some(self.connect())
        }

        fn superseded(&mut self) -> bool {
            self.superseded
        }
    }

    /// Run a relay over in-memory pipes; returns the host's side.
    fn start(
        dialer: FakeDialer,
    ) -> (
        tokio::io::DuplexStream,
        tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
        tokio::task::JoinHandle<Result<bool, String>>,
    ) {
        let (host_in, relay_in) = tokio::io::duplex(1 << 16);
        let (relay_out, host_out) = tokio::io::duplex(1 << 16);
        let handle = tokio::spawn(async move {
            let mut dialer = dialer;
            let first = dialer.connect();
            relay(
                relay_in,
                relay_out,
                first,
                &mut dialer,
                Duration::from_millis(20),
                Duration::from_secs(5),
            )
            .await
            .map_err(|e| e.to_string())
        });
        (host_in, BufReader::new(host_out).lines(), handle)
    }

    async fn next_answer(
        lines: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    ) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("an answer in time")
            .unwrap()
            .expect("the relay closed the host's stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// The daemon dies under a read; the host gets the answer from the next
    /// daemon and sees exactly one `initialize` answer.
    ///
    /// Mutation that must fail it: return on a lost link instead of dialing.
    #[tokio::test]
    async fn a_session_outlives_its_daemon() {
        let dialer = FakeDialer {
            next_pid: 1,
            die_after: vec![Some(1)],
            superseded: false,
        };
        let (mut host, mut answers, handle) = start(dialer);
        host.write_all(&request(1, "initialize", json!({})))
            .await
            .unwrap();
        assert_eq!(next_answer(&mut answers).await["id"], json!(1));
        host.write_all(&initialized()).await.unwrap();
        host.write_all(&call(2, "search")).await.unwrap();
        assert_eq!(next_answer(&mut answers).await["result"]["pid"], json!(1));

        // Daemon 1 closes instead of answering this one.
        host.write_all(&call(3, "search")).await.unwrap();
        let answer = next_answer(&mut answers).await;
        assert_eq!(answer["id"], json!(3));
        assert_eq!(answer["result"]["pid"], json!(2), "{answer}");

        drop(host);
        assert_eq!(handle.await.unwrap(), Ok(true));
    }

    /// A daemon that behaves like rmcp at EOF: the service ends the moment its
    /// input does, and an answer still being computed (100 ms here) is dropped.
    fn eof_dropping_daemon(stream: UnixStream) {
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(text)) = lines.next_line().await {
                let value: Value = serde_json::from_str(&text).unwrap();
                let Some(id) = value.get("id").cloned() else {
                    continue;
                };
                if value["method"] != "initialize" {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        next = lines.next_line() => {
                            assert!(matches!(next, Ok(None)), "one request at a time here");
                            return;
                        }
                    }
                }
                let reply = line(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                writer.write_all(&reply).await.unwrap();
            }
        });
    }

    /// A one-shot host — requests, then EOF at once — gets every answer.
    ///
    /// Mutation that must fail it: half-close the daemon connection at stdin
    /// EOF rather than after the last answer, and the call's answer is lost.
    #[tokio::test]
    async fn a_one_shot_session_gets_every_answer() {
        let (host_in, relay_in) = tokio::io::duplex(1 << 16);
        let (relay_out, host_out) = tokio::io::duplex(1 << 16);
        let (ours, theirs) = UnixStream::pair().unwrap();
        eof_dropping_daemon(theirs);

        let mut host = host_in;
        host.write_all(&request(1, "initialize", json!({})))
            .await
            .unwrap();
        host.write_all(&initialized()).await.unwrap();
        host.write_all(&call(2, "search")).await.unwrap();
        drop(host);

        let mut dialer = FakeDialer {
            next_pid: 1,
            die_after: vec![],
            superseded: false,
        };
        let verdict = relay(
            relay_in,
            relay_out,
            ours,
            &mut dialer,
            Duration::from_secs(60),
            Duration::from_secs(5),
        )
        .await
        .map_err(|e| e.to_string());

        let mut ids = Vec::new();
        let mut answers = BufReader::new(host_out).lines();
        while let Ok(Some(line)) = answers.next_line().await {
            ids.push(serde_json::from_str::<Value>(&line).unwrap()["id"].clone());
        }
        assert_eq!(ids, vec![json!(1), json!(2)]);
        assert_eq!(verdict, Ok(true));
        // The answer would also arrive by losing the daemon and replaying the
        // call on another one; the point is that the first daemon delivers it.
        assert_eq!(dialer.next_pid, 1, "the daemon was lost and replaced");
    }

    /// A superseded daemon is left between requests, not under one.
    ///
    /// Mutation that must fail it: ignore `superseded()`, and the second call
    /// is still answered by daemon 1.
    #[tokio::test]
    async fn a_superseded_daemon_is_left_between_requests() {
        let dialer = FakeDialer {
            next_pid: 1,
            die_after: vec![],
            superseded: true,
        };
        let (mut host, mut answers, handle) = start(dialer);
        host.write_all(&request(1, "initialize", json!({})))
            .await
            .unwrap();
        // Whichever daemon answers it: the first tick may already have moved.
        assert_eq!(next_answer(&mut answers).await["id"], json!(1));
        host.write_all(&initialized()).await.unwrap();

        tokio::time::sleep(Duration::from_millis(200)).await;
        host.write_all(&call(2, "search")).await.unwrap();
        let answer = next_answer(&mut answers).await;
        assert_eq!(
            answer["id"],
            json!(2),
            "the swallowed initialize must not reach the host"
        );
        assert_eq!(answer["result"]["pid"], json!(2));

        drop(host);
        assert_eq!(handle.await.unwrap(), Ok(true));
    }
}
