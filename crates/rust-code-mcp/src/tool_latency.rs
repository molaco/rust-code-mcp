//! `--tool-latency <LOG>...`: did a reading tool wait for somebody else's work?
//!
//! # What it answers
//!
//! The daemon logs one `mem:` line per tool call, sync, vector-store
//! maintenance pass, index write and rust-analyzer load (`rmc_engine::mem_trace`):
//! the line's timestamp is the END of the operation and `took` its length, so
//! every line is an interval. A reading tool (`search`, `find_definition`,
//! `health_check`, graph queries, …) that ran while other work ran is a
//! suspect; whether it actually WAITED is read against the same tool on the
//! same workspace when nothing else ran ("clean" calls):
//!
//! - `own` — a rust-analyzer tool loading ITS OWN project pays for the load;
//!   that is the call's cost, not a wait. Loads of any other project are not
//!   (since the per-project slots one project's load must hold no other).
//!   Likewise a `search` that builds the missing index of its directory
//!   inside the call (an index build that began before the call is somebody
//!   else's, and waiting for it counts).
//! - `base` — the longest clean call of the same tool on the same workspace
//!   (the same tool on any workspace when that one has none; 0 when the tool
//!   never ran clean).
//! - `excess = took - own - base`. A call is **blocked** when the excess is
//!   more than [`SLACK_SECS`] and other work overlapped it for more than
//!   [`SLACK_SECS`]: it took longer than that tool ever takes alone, and the
//!   difference sat beside somebody else's work.
//!
//! "Blocked" is evidence, not proof: a call that is long for its own reasons
//! (references of a symbol used everywhere) and happened to overlap a sync
//! is listed too. The report names the overlapping work with the most
//! overlap and how long before the call's end it finished — a waiter wakes
//! when the holder lets go, so a gap near zero is the signature of a wait.
//!
//! # Lines it reads
//!
//! - `mem: tool <name>[ <dir>] took …`
//! - `mem: background sync of <dir> took …`
//! - `mem: vector store maintenance[ of <dir>] took …`
//! - `mem: index changes[ of <dir>] (…) took …`
//! - `mem: analysis load <kind> of <dir> took …`
//! - `mem: index build of <dir> took …`
//!
//! Older binaries wrote maintenance and index lines without the directory and
//! had neither load nor build line. A dir-less line is given the directory of
//! the sync or `index_codebase` call of the same process that encloses it; a
//! load is the interval from `Loading <kind> IDE for project: <dir>` to the
//! next `IDE loaded successfully` of the same process, a build from
//! `Initializing unified indexer for <dir>` to the next `Indexed N files`.
//! That pairing is exact only while such work did not overlap in a process;
//! the report counts the closes that had more than one open.
//!
//! Intervals of different processes (daemons sharing a log over a binary
//! swap) never overlap here: the locks in question are per process.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The slack of the criterion, seconds: what a reader may lose beside other
/// work before it counts as blocked (the plan's `max(own work) + 2 s`).
pub const SLACK_SECS: f64 = 2.0;

/// `took` is printed with one decimal; an interval's start is that much less
/// exact than its end. Used when deciding which sync encloses a dir-less line.
const ROUNDING_SECS: f64 = 0.15;

/// Tools that change the daemon's state. Every other tool reads.
const WRITER_TOOLS: &[&str] = &[
    "index_codebase",
    "clear_cache",
    "clear_runtime",
    "build_hypergraph",
    "build_codemap",
    "semantic_overlaps",
];

/// Tools that run on a rust-analyzer project: a load of their own workspace
/// during the call is the call's cost.
const ANALYSIS_TOOLS: &[&str] = &["find_definition", "find_references", "rename_symbol"];

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Tool(String),
    Sync,
    Maintenance,
    IndexChanges,
    AnalysisLoad,
    /// A `search` building the missing index of its directory first.
    IndexBuild,
}

impl Kind {
    fn label(&self) -> String {
        match self {
            Kind::Tool(name) => format!("tool {name}"),
            Kind::Sync => "background sync".to_string(),
            Kind::Maintenance => "vector store maintenance".to_string(),
            Kind::IndexChanges => "index changes".to_string(),
            Kind::AnalysisLoad => "analysis load".to_string(),
            Kind::IndexBuild => "index build".to_string(),
        }
    }

    fn reader(&self) -> Option<&str> {
        match self {
            Kind::Tool(name) if !WRITER_TOOLS.contains(&name.as_str()) => Some(name),
            _ => None,
        }
    }
}

/// One operation of one daemon process, `[start, end]` in seconds since the
/// Unix epoch (UTC).
#[derive(Debug, Clone)]
pub struct Span {
    pub pid: u32,
    pub kind: Kind,
    pub workspace: Option<String>,
    pub start: f64,
    pub end: f64,
}

impl Span {
    fn took(&self) -> f64 {
        self.end - self.start
    }

    fn overlap(&self, other: &Span) -> Option<(f64, f64)> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (end > start).then_some((start, end))
    }
}

/// The spans of a set of log files, with what the reading had to guess.
#[derive(Debug, Default)]
pub struct Parsed {
    pub spans: Vec<Span>,
    /// Loads and index builds rebuilt from their opening and closing text
    /// lines ([`TEXT_PAIRS`]).
    pub rebuilt_from_text: usize,
    /// …of which the close had more than one open (paired with the latest;
    /// exact only while such work did not overlap in a process).
    pub paired_by_guess: usize,
}

/// Seconds since the Unix epoch of `2026-10-31T15:34:46.421517Z` (the
/// fraction and the `Z` optional). `None` for anything else.
pub fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.strip_suffix('Z').unwrap_or(text);
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: f64 = time.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from 1970-01-01 of a proleptic Gregorian date (H. Hinnant's
    // `days_from_civil`).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hour * 3_600 + minute * 60) as f64 + second)
}

/// `MM-DD HH:MM:SS.s` of an epoch time, UTC — the report's clock (a daemon
/// log spans days).
fn clock(at: f64) -> String {
    let days = at.div_euclid(86_400.0) as i64;
    let in_day = at.rem_euclid(86_400.0);
    let hours = (in_day / 3_600.0) as u32;
    let minutes = ((in_day % 3_600.0) / 60.0) as u32;
    let seconds = in_day % 60.0;
    // H. Hinnant's `civil_from_days`, month and day only.
    let z = days + 719_468;
    let doe = z - z.div_euclid(146_097) * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:04.1}")
}

/// The timestamp, the pid (0 when the line has none) and the rest.
fn split_line(line: &str) -> Option<(f64, u32, &str)> {
    let (stamp, rest) = line.split_once(' ')?;
    let at = parse_timestamp(stamp)?;
    let rest = rest.trim_start();
    let (pid, rest) = match rest.strip_prefix('[') {
        Some(tail) => {
            let (pid, rest) = tail.split_once(']')?;
            (pid.parse().ok()?, rest)
        }
        None => (0, rest),
    };
    Some((at, pid, rest))
}

/// What a `mem:` line measured: `<what> took <secs>s, …`.
fn parse_mem(what_and_rest: &str) -> Option<(Kind, Option<String>, f64)> {
    let (what, rest) = what_and_rest.rsplit_once(" took ")?;
    let secs: f64 = rest.split_once("s,")?.0.parse().ok()?;
    let (kind, workspace) = if let Some(tool) = what.strip_prefix("tool ") {
        match tool.split_once(' ') {
            Some((name, dir)) => (Kind::Tool(name.to_string()), Some(dir.to_string())),
            None => (Kind::Tool(tool.to_string()), None),
        }
    } else if let Some(dir) = what.strip_prefix("background sync of ") {
        (Kind::Sync, Some(dir.to_string()))
    } else if let Some(tail) = what.strip_prefix("vector store maintenance") {
        (
            Kind::Maintenance,
            tail.strip_prefix(" of ").map(str::to_string),
        )
    } else if let Some(tail) = what.strip_prefix("index changes") {
        let dir = tail
            .strip_prefix(" of ")
            .and_then(|tail| tail.rsplit_once(" (").map(|(dir, _)| dir.to_string()));
        (Kind::IndexChanges, dir)
    } else if let Some(tail) = what.strip_prefix("analysis load ") {
        let (_, dir) = tail.split_once(" of ")?;
        (Kind::AnalysisLoad, Some(dir.to_string()))
    } else if let Some(dir) = what.strip_prefix("index build of ") {
        (Kind::IndexBuild, Some(dir.to_string()))
    } else {
        // `indexer init for …` and anything newer: not work a reader waits for.
        return None;
    };
    Some((kind, workspace, secs))
}

/// Work an older binary logged as an opening and a closing text line instead
/// of a `mem:` line.
struct TextPair {
    kind: Kind,
    /// The directory, when `rest` (the line after the pid) opens one.
    opens: fn(&str) -> Option<&str>,
    closes: fn(&str) -> bool,
}

const TEXT_PAIRS: &[TextPair] = &[
    TextPair {
        kind: Kind::AnalysisLoad,
        opens: |rest| rest.split_once(" IDE for project: ").map(|(_, dir)| dir),
        closes: |rest| rest.ends_with("IDE loaded successfully"),
    },
    TextPair {
        kind: Kind::IndexBuild,
        opens: |rest| {
            rest.split_once(
                "rmc_server::tools::endpoints::query: Initializing unified indexer for ",
            )
            .map(|(_, dir)| dir)
        },
        closes: |rest| rest.contains("rmc_server::tools::endpoints::query: Indexed "),
    },
];

/// Read the spans out of log text (several files: call once per file and
/// [`Parsed::absorb`] — or feed the concatenation, the pairing is per pid).
pub fn parse_log(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    // Per (pair, pid): the open lines (time, dir). A process that writes the
    // `mem:` line of a kind itself has its text pairs of that kind ignored.
    let mut open: BTreeMap<(usize, u32), Vec<(f64, String)>> = BTreeMap::new();
    let mut from_text: Vec<(Span, bool)> = Vec::new();
    for line in text.lines() {
        let Some((at, pid, rest)) = split_line(line) else {
            continue;
        };
        if let Some((_, what)) = rest.split_once(" mem: ") {
            if !rest.contains("rmc_mem:") {
                continue;
            }
            if let Some((kind, workspace, secs)) = parse_mem(what) {
                parsed.spans.push(Span {
                    pid,
                    kind,
                    workspace,
                    start: at - secs,
                    end: at,
                });
            }
        } else {
            for (index, pair) in TEXT_PAIRS.iter().enumerate() {
                if let Some(dir) = (pair.opens)(rest) {
                    open.entry((index, pid))
                        .or_default()
                        .push((at, dir.trim().to_string()));
                } else if (pair.closes)(rest) {
                    let open = open.entry((index, pid)).or_default();
                    // The latest open one; any earlier ones never closed
                    // (failed) — or, where such work overlapped, a guess.
                    if let Some((start, dir)) = open.pop() {
                        let guessed = !open.is_empty();
                        open.clear();
                        let span = Span {
                            pid,
                            kind: pair.kind.clone(),
                            workspace: Some(dir),
                            start,
                            end: at,
                        };
                        from_text.push((span, guessed));
                    }
                }
            }
        }
    }
    let with_mem_lines: Vec<(Kind, u32)> = parsed
        .spans
        .iter()
        .filter(|span| TEXT_PAIRS.iter().any(|pair| pair.kind == span.kind))
        .map(|span| (span.kind.clone(), span.pid))
        .collect();
    for (span, guessed) in from_text {
        if with_mem_lines.contains(&(span.kind.clone(), span.pid)) {
            continue;
        }
        parsed.rebuilt_from_text += 1;
        parsed.paired_by_guess += usize::from(guessed);
        parsed.spans.push(span);
    }
    attribute_dirless(&mut parsed.spans);
    parsed.spans.sort_by(|a, b| a.end.total_cmp(&b.end));
    parsed
}

impl Parsed {
    pub fn absorb(&mut self, other: Parsed) {
        self.spans.extend(other.spans);
        self.rebuilt_from_text += other.rebuilt_from_text;
        self.paired_by_guess += other.paired_by_guess;
        self.spans.sort_by(|a, b| a.end.total_cmp(&b.end));
    }
}

/// Give a dir-less maintenance or index line the directory of the innermost
/// sync or `index_codebase` call of its process that encloses it.
fn attribute_dirless(spans: &mut [Span]) {
    let owners: Vec<Span> = spans
        .iter()
        .filter(|span| {
            span.workspace.is_some()
                && (span.kind == Kind::Sync || span.kind == Kind::Tool("index_codebase".into()))
        })
        .cloned()
        .collect();
    for span in spans.iter_mut() {
        if span.workspace.is_some() || !matches!(span.kind, Kind::Maintenance | Kind::IndexChanges)
        {
            continue;
        }
        span.workspace = owners
            .iter()
            .filter(|owner| {
                owner.pid == span.pid
                    && owner.start <= span.start + ROUNDING_SECS
                    && span.end <= owner.end + ROUNDING_SECS
            })
            .min_by(|a, b| a.took().total_cmp(&b.took()))
            .and_then(|owner| owner.workspace.clone());
    }
}

/// Length of the union of intervals.
fn union_len(mut parts: Vec<(f64, f64)>) -> f64 {
    parts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut current: Option<(f64, f64)> = None;
    for (start, end) in parts {
        current = match current {
            Some((cs, ce)) if start <= ce => Some((cs, ce.max(end))),
            Some((cs, ce)) => {
                total += ce - cs;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((cs, ce)) = current {
        total += ce - cs;
    }
    total
}

/// One reading call, judged.
#[derive(Debug, Clone)]
pub struct Judged {
    pub call: Span,
    /// Its own project's rust-analyzer load during the call, seconds.
    pub own: f64,
    /// Longest clean call of the same tool (see the module docs).
    pub base: f64,
    /// `base` came from the tool on other workspaces, or is 0 (never clean).
    pub base_borrowed: bool,
    /// Seconds other work overlapped the call (union).
    pub overlapped: f64,
    /// What the call most likely waited for (see [`culprit`]); `None` for a
    /// clean call.
    pub culprit: Option<Span>,
    /// The overlapping span with the most overlap (see [`longest_beside`]);
    /// printed when it is not the culprit.
    pub longest: Option<Span>,
    pub blocked: bool,
}

impl Judged {
    pub fn excess(&self) -> f64 {
        self.call.took() - self.own - self.base
    }
}

/// Judge every reading call ending in `[since, until]`. Baselines come from
/// every call in `parsed`, inside the window or not.
pub fn judge(parsed: &Parsed, since: Option<f64>, until: Option<f64>) -> Vec<Judged> {
    struct Raw {
        call: Span,
        wake: f64,
        own: f64,
        others: Vec<Span>,
        overlapped: f64,
    }
    let mut raws = Vec::new();
    for call in &parsed.spans {
        let Some(name) = call.kind.reader() else {
            continue;
        };
        let analysis = ANALYSIS_TOOLS.contains(&name);
        let mut own_parts = Vec::new();
        let mut other_parts = Vec::new();
        let mut others = Vec::new();
        for span in &parsed.spans {
            if span.pid != call.pid || std::ptr::eq(span, call) {
                continue;
            }
            let Some(part) = call.overlap(span) else {
                continue;
            };
            let same_workspace = span.workspace == call.workspace;
            let own_load = analysis && span.kind == Kind::AnalysisLoad && same_workspace;
            // Begun inside the call: this search's build. Begun before, it
            // is another call's, and this one waited for it.
            let own_build = span.kind == Kind::IndexBuild
                && same_workspace
                && span.start >= call.start - ROUNDING_SECS;
            if own_load || own_build {
                own_parts.push(part);
            } else {
                other_parts.push(part);
                others.push(span.clone());
            }
        }
        // A call that loads its own project stopped waiting when the load
        // began; one without a load, at its end.
        let wake = own_parts
            .iter()
            .map(|(start, _)| *start)
            .reduce(f64::min)
            .unwrap_or(call.end);
        raws.push(Raw {
            call: call.clone(),
            wake,
            own: union_len(own_parts),
            others,
            overlapped: union_len(other_parts),
        });
    }

    // Clean = nothing else ran and no load of its own: the tool's own work.
    let mut clean_by_workspace: BTreeMap<(String, Option<String>), f64> = BTreeMap::new();
    let mut clean_by_tool: BTreeMap<String, f64> = BTreeMap::new();
    for raw in raws
        .iter()
        .filter(|raw| raw.others.is_empty() && raw.own == 0.0)
    {
        let name = raw.call.kind.reader().unwrap().to_string();
        let took = raw.call.took();
        let slot = clean_by_workspace
            .entry((name.clone(), raw.call.workspace.clone()))
            .or_insert(0.0);
        *slot = slot.max(took);
        let slot = clean_by_tool.entry(name).or_insert(0.0);
        *slot = slot.max(took);
    }

    raws.into_iter()
        .filter(|raw| {
            since.is_none_or(|since| raw.call.end >= since)
                && until.is_none_or(|until| raw.call.end <= until)
        })
        .map(|raw| {
            let name = raw.call.kind.reader().unwrap().to_string();
            let (base, base_borrowed) =
                match clean_by_workspace.get(&(name.clone(), raw.call.workspace.clone())) {
                    Some(&base) => (base, false),
                    None => (clean_by_tool.get(&name).copied().unwrap_or(0.0), true),
                };
            let culprit = culprit(&raw.call, &raw.others, raw.wake);
            let longest = longest_beside(&raw.call, &raw.others);
            let excess = raw.call.took() - raw.own - base;
            Judged {
                longest,
                blocked: excess > SLACK_SECS && raw.overlapped > SLACK_SECS,
                call: raw.call,
                own: raw.own,
                base,
                base_borrowed,
                overlapped: raw.overlapped,
                culprit,
            }
        })
        .collect()
}

/// Work before other readers: a reader waits for work, and readers beside a
/// waiting one are usually waiting too.
fn work_first<'a>(spans: impl Iterator<Item = &'a Span>) -> Vec<&'a Span> {
    let all: Vec<&Span> = spans.collect();
    let work: Vec<&Span> = all
        .iter()
        .copied()
        .filter(|s| s.kind.reader().is_none())
        .collect();
    if work.is_empty() { all } else { work }
}

/// The span `call` most likely waited for. A waiter wakes when the holder
/// lets go, so: the span (work first) that ended last before the call woke —
/// before its own load began, or before its end when it had none — among
/// those beside the call for more than [`SLACK_SECS`] (a 0.1 s sync of a
/// neighbour ending just then held nobody). When none ended inside the call,
/// the one with the most overlap, and of the spans within [`SLACK_SECS`] of
/// that the shortest (the inner one, e.g. the maintenance pass rather than
/// the sync that ran it).
fn culprit(call: &Span, others: &[Span], wake: f64) -> Option<Span> {
    let ended_before_wake = work_first(others.iter().filter(|s| {
        s.end <= wake + ROUNDING_SECS
            && call
                .overlap(s)
                .is_some_and(|(start, end)| end - start > SLACK_SECS)
    }));
    if let Some(latest) = ended_before_wake
        .into_iter()
        .max_by(|a, b| a.end.total_cmp(&b.end))
    {
        return Some(latest.clone());
    }
    longest_beside(call, others)
}

/// The overlapping span (work first) with the most overlap; ties within
/// [`SLACK_SECS`] go to the shortest span.
fn longest_beside(call: &Span, others: &[Span]) -> Option<Span> {
    let len = |span: &Span| call.overlap(span).map_or(0.0, |(s, e)| e - s);
    let pool = work_first(others.iter());
    let most = pool.iter().map(|span| len(span)).reduce(f64::max)?;
    pool.into_iter()
        .filter(|span| len(span) >= most - SLACK_SECS)
        .min_by(|a, b| a.took().total_cmp(&b.took()))
        .cloned()
}

fn short(workspace: &Option<String>) -> String {
    workspace.as_deref().map_or_else(
        || "-".to_string(),
        |dir| dir.rsplit('/').next().unwrap_or(dir).to_string(),
    )
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// The report, and the number of blocked calls in it.
pub fn report(parsed: &Parsed, judged: &[Judged]) -> (String, usize) {
    use std::fmt::Write;
    let mut out = String::new();
    let work = parsed
        .spans
        .iter()
        .filter(|span| span.kind.reader().is_none())
        .count();
    let first = judged.iter().map(|j| j.call.end).reduce(f64::min);
    let last = judged.iter().map(|j| j.call.end).reduce(f64::max);
    writeln!(
        out,
        "tool latency: {} reading call(s) judged, {} work span(s) read; window {} .. {} UTC",
        judged.len(),
        work,
        first.map_or("-".into(), clock),
        last.map_or("-".into(), clock),
    )
    .unwrap();
    writeln!(
        out,
        "loads and index builds rebuilt from text lines: {} ({} paired by guess)",
        parsed.rebuilt_from_text, parsed.paired_by_guess
    )
    .unwrap();

    let mut by_tool: BTreeMap<&str, Vec<&Judged>> = BTreeMap::new();
    for j in judged {
        by_tool
            .entry(j.call.kind.reader().unwrap())
            .or_default()
            .push(j);
    }
    writeln!(
        out,
        "\n{:<22} {:>6} {:>7} {:>7}   {:>6} {:>7} {:>7}   {:>4} {:>7}",
        "tool", "clean", "p50 s", "max s", "beside", "p50 s", "max s", "own", "blocked"
    )
    .unwrap();
    for (tool, calls) in &by_tool {
        let durations = |pick: &dyn Fn(&&Judged) -> bool| {
            let mut v: Vec<f64> = calls
                .iter()
                .filter(|j| pick(j))
                .map(|j| j.call.took())
                .collect();
            v.sort_by(f64::total_cmp);
            v
        };
        let clean = durations(&|j| j.culprit.is_none() && j.own == 0.0);
        let beside = durations(&|j| j.culprit.is_some());
        let stats = |v: &[f64]| {
            if v.is_empty() {
                ("-".to_string(), "-".to_string())
            } else {
                (
                    format!("{:.1}", quantile(v, 0.5)),
                    format!("{:.1}", v[v.len() - 1]),
                )
            }
        };
        let (clean_p50, clean_max) = stats(&clean);
        let (beside_p50, beside_max) = stats(&beside);
        writeln!(
            out,
            "{:<22} {:>6} {:>7} {:>7}   {:>6} {:>7} {:>7}   {:>4} {:>7}",
            tool,
            clean.len(),
            clean_p50,
            clean_max,
            beside.len(),
            beside_p50,
            beside_max,
            calls.iter().filter(|j| j.own > 0.0).count(),
            calls.iter().filter(|j| j.blocked).count(),
        )
        .unwrap();
    }

    let blocked: Vec<&Judged> = judged.iter().filter(|j| j.blocked).collect();
    writeln!(
        out,
        "\nblocked: took more than own load + the clean max + {SLACK_SECS} s, beside other work for more than {SLACK_SECS} s"
    )
    .unwrap();
    let describe = |call: &Span, span: &Span| {
        let when = if span.end <= call.end {
            format!("ended {:.1} s before its end", call.end - span.end)
        } else {
            "ran past its end".to_string()
        };
        format!(
            "{} {} {:.1} s, {when}",
            span.kind.label(),
            short(&span.workspace),
            span.took()
        )
    };
    for j in &blocked {
        let culprit = j.culprit.as_ref().unwrap();
        write!(
            out,
            "  {} pid {} {} {}: took {:.1} s = own {:.1} + base {:.1}{} + excess {:.1}; beside work {:.1} s\n      waited for: {}",
            clock(j.call.end),
            j.call.pid,
            j.call.kind.label(),
            short(&j.call.workspace),
            j.call.took(),
            j.own,
            j.base,
            if j.base_borrowed { "*" } else { "" },
            j.excess(),
            j.overlapped,
            describe(&j.call, culprit),
        )
        .unwrap();
        match &j.longest {
            Some(longest)
                if longest.end != culprit.end
                    || longest.kind != culprit.kind
                    || longest.workspace != culprit.workspace =>
            {
                writeln!(out, "; longest beside: {}", describe(&j.call, longest)).unwrap()
            }
            _ => writeln!(out).unwrap(),
        }
    }
    writeln!(
        out,
        "(* base from the tool on other workspaces, or 0: never ran clean here)\nverdict: {} blocked call(s)",
        blocked.len()
    )
    .unwrap();
    (out, blocked.len())
}

/// The CLI entry: read `logs`, judge, print. Returns the number of blocked
/// calls (the process exits 1 when it is not zero).
pub fn run(
    logs: &[PathBuf],
    since: Option<f64>,
    until: Option<f64>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut parsed = Parsed::default();
    for log in logs {
        let text = std::fs::read_to_string(log)
            .map_err(|e| format!("cannot read {}: {e}", log.display()))?;
        parsed.absorb(parse_log(&text));
    }
    let judged = judge(&parsed, since, until);
    let (text, blocked) = report(&parsed, &judged);
    print!("{text}");
    Ok(blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(at: &str, pid: u32, what: &str, took: f64) -> String {
        format!(
            "2026-10-31T{at}Z [{pid}]  INFO rmc_mem: mem: {what} took {took:.1}s, RSS 1 -> 2 MB (+1), peak 3 MB\n"
        )
    }

    fn text(at: &str, pid: u32, target: &str, message: &str) -> String {
        format!("2026-10-31T{at}Z [{pid}]  INFO {target}: {message}\n")
    }

    fn blocked_tools(judged: &[Judged]) -> Vec<(String, f64)> {
        judged
            .iter()
            .filter(|j| j.blocked)
            .map(|j| (j.call.kind.label(), (j.call.took() * 10.0).round() / 10.0))
            .collect()
    }

    #[test]
    fn timestamps_count_from_the_epoch() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_timestamp("2000-03-01T00:00:00Z"), Some(951_868_800.0));
        let at = parse_timestamp("2026-10-31T15:34:46.5Z").unwrap();
        assert_eq!(clock(at), "10-31 15:34:46.5");
        assert_eq!(clock(951_868_800.0 - 0.5), "02-29 23:59:59.5");
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_timestamp("INFO"), None);
    }

    /// The measured case of 2026-10-31 (17:27–17:35 EET), shape kept, paths
    /// replaced: a `search` that waited 413.9 s for maintenance which ran
    /// under the workspace lock, inside its sync. Clean searches before give
    /// the baseline. This is the oracle's own mutant: run on the log of the
    /// binary that blocked, it must name the call.
    #[test]
    fn the_search_that_waited_for_maintenance_is_blocked() {
        let mut log = String::new();
        log += &mem("15:16:26.720", 7, "tool search /w/big", 0.2);
        log += &mem("15:16:29.168", 7, "tool search /w/big", 0.4);
        log += &mem(
            "15:27:39.728",
            7,
            "index changes (2 added, 6 modified, 0 deleted)",
            10.7,
        );
        log += &mem("15:30:32.632", 7, "tool search /w/small", 0.3);
        log += &mem("15:34:44.901", 7, "vector store maintenance", 425.2);
        log += &mem("15:34:45.327", 7, "background sync of /w/big", 438.7);
        // The neighbour's next sync, ending last before the search: too short
        // to have held anybody, it must not be named.
        log += &mem("15:34:45.900", 7, "background sync of /w/small", 0.1);
        log += &mem("15:34:46.421", 7, "tool search /w/big", 413.9);
        let parsed = parse_log(&log);
        let maintenance = parsed
            .spans
            .iter()
            .find(|span| span.kind == Kind::Maintenance)
            .unwrap();
        assert_eq!(maintenance.workspace.as_deref(), Some("/w/big"));

        let judged = judge(&parsed, None, None);
        assert_eq!(
            blocked_tools(&judged),
            vec![("tool search".to_string(), 413.9)]
        );
        let search = judged.iter().find(|j| j.blocked).unwrap();
        // Epoch seconds in f64: microsecond-exact, not decimal-exact.
        assert!((search.base - 0.4).abs() < 1e-3, "{search:?}");
        // It woke when the sync let go of the lock, 1.1 s before its end;
        // the maintenance pass inside the sync is the longest beside it.
        assert_eq!(search.culprit.as_ref().unwrap().kind, Kind::Sync);
        assert_eq!(search.longest.as_ref().unwrap().kind, Kind::Maintenance);
        let (report, blocked) = report(&parsed, &judged);
        assert_eq!(blocked, 1, "{report}");
        assert!(
            report.contains("waited for: background sync big 438.7 s, ended 1.1 s before its end"),
            "{report}"
        );
        assert!(
            report.contains("longest beside: vector store maintenance big 425.2 s"),
            "{report}"
        );
    }

    /// A rust-analyzer tool that loads its own project pays for the load; a
    /// load of ANOTHER project in the same call is a wait. The second case is
    /// the measured `find_definition` of the small project (176.7 s) that
    /// stood behind the big one's load, from the text pair of an old binary.
    #[test]
    fn an_own_load_is_cost_and_another_projects_load_is_a_wait() {
        let mut log = String::new();
        log += &mem("15:15:13.322", 7, "tool find_definition /w/small", 0.5);
        log += &text(
            "15:30:39.878",
            7,
            "rmc_server::semantic",
            "Loading Full IDE for project: /w/small",
        );
        log += &text(
            "15:31:11.128",
            7,
            "rmc_server::semantic",
            "IDE loaded successfully",
        );
        log += &mem("15:31:11.936", 7, "tool find_definition /w/small", 32.1);
        log += &text(
            "15:31:13.220",
            7,
            "rmc_server::semantic",
            "Loading Full IDE for project: /w/big",
        );
        log += &text(
            "15:33:41.984",
            7,
            "rmc_server::semantic",
            "IDE loaded successfully",
        );
        log += &mem("15:33:45.101", 7, "tool find_definition /w/big", 156.4);
        log += &text(
            "15:33:45.119",
            7,
            "rmc_server::semantic",
            "Loading Full IDE for project: /w/small",
        );
        log += &text(
            "15:34:24.004",
            7,
            "rmc_server::semantic",
            "IDE loaded successfully",
        );
        log += &mem("15:34:24.581", 7, "tool find_definition /w/small", 176.7);
        let parsed = parse_log(&log);
        assert_eq!((parsed.rebuilt_from_text, parsed.paired_by_guess), (3, 0));

        let judged = judge(&parsed, None, None);
        let first_small = judged
            .iter()
            .find(|j| (j.call.took() - 32.1).abs() < 0.05)
            .unwrap();
        assert!(first_small.own > 31.0, "{first_small:?}");
        assert!(!first_small.blocked, "{first_small:?}");
        // The big project's call overlaps the small one's waiting call: both
        // are listed, the small one with the big load as the culprit.
        let second_small = judged
            .iter()
            .find(|j| (j.call.took() - 176.7).abs() < 0.05)
            .unwrap();
        assert!(second_small.blocked, "{second_small:?}");
        let culprit = second_small.culprit.as_ref().unwrap();
        assert_eq!(
            (culprit.kind.clone(), culprit.workspace.as_deref()),
            (Kind::AnalysisLoad, Some("/w/big"))
        );
    }

    /// The new lines carry their directory and the load has its own line;
    /// a process that writes load lines has its text pair ignored.
    #[test]
    fn new_lines_name_their_workspace() {
        let mut log = String::new();
        log += &text(
            "10:00:00.000",
            9,
            "rmc_server::semantic",
            "Loading Full IDE for project: /w/a",
        );
        log += &mem("10:00:30.000", 9, "analysis load Full of /w/a", 30.0);
        log += &text(
            "10:00:30.001",
            9,
            "rmc_server::semantic",
            "IDE loaded successfully",
        );
        log += &mem("10:01:00.000", 9, "vector store maintenance of /w/b", 5.0);
        log += &mem(
            "10:01:01.000",
            9,
            "index changes of /w/b (1 added, 0 modified, 0 deleted)",
            1.0,
        );
        log += &mem("10:02:00.000", 9, "index build of /w/c", 50.0);
        let parsed = parse_log(&log);
        assert_eq!(parsed.rebuilt_from_text, 0);
        let workspaces: Vec<(Kind, Option<&str>)> = parsed
            .spans
            .iter()
            .map(|span| (span.kind.clone(), span.workspace.as_deref()))
            .collect();
        assert_eq!(
            workspaces,
            vec![
                (Kind::AnalysisLoad, Some("/w/a")),
                (Kind::Maintenance, Some("/w/b")),
                (Kind::IndexChanges, Some("/w/b")),
                (Kind::IndexBuild, Some("/w/c")),
            ]
        );
    }

    /// A `search` that builds its directory's missing index inside the call
    /// (measured 2026-10-31: 788 files, 600.4 s, a sync beside it 13.8 s) is
    /// long by its own work; a second `search` of that directory, which
    /// began after the build and waited for it, is blocked. Old text-pair form.
    #[test]
    fn a_search_building_its_index_pays_and_the_one_waiting_for_it_is_blocked() {
        let query = "rmc_server::tools::endpoints::query";
        let mut log = String::new();
        log += &mem("18:00:01.000", 7, "tool search /w/sub", 0.5);
        log += &mem("18:00:10.000", 7, "tool search /w/big", 0.5);
        log += &text(
            "18:10:43.905",
            7,
            query,
            "Initializing unified indexer for /w/sub",
        );
        log += &mem("18:15:00.000", 7, "background sync of /w/big", 13.8);
        log += &text(
            "18:20:44.059",
            7,
            query,
            "Indexed 781 files (32221 chunks), 0 unchanged, 7 skipped",
        );
        log += &mem("18:20:44.305", 7, "tool search /w/sub", 600.4);
        log += &mem("18:20:44.900", 7, "tool search /w/sub", 300.0);
        let parsed = parse_log(&log);
        assert_eq!((parsed.rebuilt_from_text, parsed.paired_by_guess), (1, 0));
        let judged = judge(&parsed, None, None);
        let builder = judged
            .iter()
            .find(|j| (j.call.took() - 600.4).abs() < 0.05)
            .unwrap();
        assert!(builder.own > 600.0 && !builder.blocked, "{builder:?}");
        let waiter = judged
            .iter()
            .find(|j| (j.call.took() - 300.0).abs() < 0.05)
            .unwrap();
        assert!(waiter.blocked && waiter.own == 0.0, "{waiter:?}");
        assert_eq!(waiter.culprit.as_ref().unwrap().kind, Kind::IndexBuild);
    }

    /// Readers beside each other within the slack are not blocked, and a
    /// reader is never compared across processes.
    #[test]
    fn short_overlaps_and_other_processes_do_not_count() {
        let mut log = String::new();
        log += &mem("10:00:01.000", 1, "tool search /w/a", 1.0);
        log += &mem("10:00:10.000", 1, "tool search /w/a", 2.5);
        log += &mem("10:00:10.000", 1, "tool health_check /w/a", 1.5);
        log += &mem("10:05:00.000", 2, "background sync of /w/a", 200.0);
        log += &mem("10:05:00.000", 1, "tool search /w/a", 60.0);
        let judged = judge(&parse_log(&log), None, None);
        // The 60 s search is long, but nothing of ITS process overlapped it.
        assert!(blocked_tools(&judged).is_empty(), "{judged:#?}");
        let since = parse_timestamp("2026-10-31T10:04:00Z");
        assert_eq!(judge(&parse_log(&log), since, None).len(), 1);
    }
}
