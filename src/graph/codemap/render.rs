//! Mermaid + outline renderers for `Codemap`.
//!
//! Split out of `mod.rs` in PR 13. The two entry points are
//! `render_mermaid` (produces a `flowchart LR` diagram grouped by parent
//! module) and `render_outline` (produces a hierarchy-indented text outline).
//! Small private helpers — `short_node_id`, `sanitize_mermaid_id`,
//! `escape_label` — back the mermaid renderer.

use std::collections::HashMap;

use crate::graph::codemap::model::{Codemap, CodemapEdge, CodemapNode, EdgeKind};
use crate::graph::ids::NodeId;
use crate::graph::queries::ModuleTreeNode;

// ---------------------------------------------------------------------------
// Phase 6 — Mermaid + outline renderers.
// ---------------------------------------------------------------------------

/// Render a `Codemap` as a Mermaid `flowchart LR` graph.
///
/// **Snippets are intentionally not rendered here.** Mermaid node labels are
/// short identifiers (the trailing `::` segment of each qualified name).
/// Embedding 5-line code snippets inside `["..."]` labels would bloat the
/// diagram beyond readability and trigger Mermaid's quoted-label escaping
/// edge cases. The JSON `Codemap` payload still carries `CodemapNode.snippet`
/// when `include_snippets=true`; the outline renderer prints it. Consumers
/// that want code alongside the diagram can read it from there.
///
/// Layout choices:
/// - Nodes are grouped into per-module `subgraph` blocks (flat, not nested),
///   keyed on each node's parent qualified-name path (the substring before
///   the last `::` segment). Nodes without a parent (top-level crate items)
///   go into an `_orphans` group keyed on the bare crate prefix.
/// - Node IDs are `n_<first 8 hex chars of NodeId bytes>` — deterministic,
///   short, Mermaid-safe.
/// - `EdgeKind::Calls` uses solid arrows (`-->`), `EdgeKind::Uses` uses
///   dotted arrows (`-.->`). `Imports` and `Contains` are not produced by
///   the current algorithm and are skipped if encountered.
/// - Edge labels include `(×N)` when weight > 1.
/// - Seeds carry the `:::seed` class; the `classDef seed` is declared once
///   at the bottom.
///
/// Identifiers are sanitized: `:`, `<`, `>`, spaces, and other Mermaid-
/// hostile characters are mapped to `_`. Display text (inside `["..."]`)
/// is left as-is except for escaping `"`.
pub(crate) fn render_mermaid(cm: &Codemap) -> String {
    let mut out = String::new();
    out.push_str("flowchart LR\n");

    // Build a NodeId -> &CodemapNode lookup so edges can resolve display
    // names without re-walking the nodes Vec each time.
    let nodes_by_id: HashMap<NodeId, &CodemapNode> =
        cm.nodes.iter().map(|n| (n.id, n)).collect();

    // Group nodes by parent module qualified name.
    let mut groups: HashMap<String, Vec<&CodemapNode>> = HashMap::new();
    for node in &cm.nodes {
        let parent = match node.qualified_name.rsplit_once("::") {
            Some((parent, _)) => parent.to_string(),
            None => "_orphans".to_string(),
        };
        groups.entry(parent).or_default().push(node);
    }

    // Deterministic group ordering by parent qualified name.
    let mut group_keys: Vec<String> = groups.keys().cloned().collect();
    group_keys.sort();

    for parent_qn in &group_keys {
        let group_id = sanitize_mermaid_id(parent_qn);
        let display = if parent_qn == "_orphans" {
            "orphans".to_string()
        } else {
            format!("mod {parent_qn}")
        };
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("  subgraph m_{group_id} [\"{}\"]\n", escape_label(&display)),
        );
        // Deterministic node ordering inside the subgraph by qualified name.
        let mut group_nodes: Vec<&CodemapNode> = groups[parent_qn].iter().copied().collect();
        group_nodes.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
        for node in group_nodes {
            let short = short_node_id(node.id);
            let display_name = node
                .qualified_name
                .rsplit_once("::")
                .map(|(_, tail)| tail.to_string())
                .unwrap_or_else(|| node.qualified_name.clone());
            let class_suffix = if node.is_seed { ":::seed" } else { "" };
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "    n_{short}[\"{}\"]{}\n",
                    escape_label(&display_name),
                    class_suffix
                ),
            );
        }
        out.push_str("  end\n");
    }

    // Edges — sorted by (from_qn, to_qn) for stability; the algorithm
    // already produces them sorted, but a defensive resort costs nothing.
    let mut edges_sorted: Vec<&CodemapEdge> = cm.edges.iter().collect();
    edges_sorted.sort_by(|a, b| {
        let aq = nodes_by_id
            .get(&a.from)
            .map(|n| n.qualified_name.as_str())
            .unwrap_or("");
        let bq = nodes_by_id
            .get(&b.from)
            .map(|n| n.qualified_name.as_str())
            .unwrap_or("");
        aq.cmp(bq).then_with(|| {
            let aq2 = nodes_by_id
                .get(&a.to)
                .map(|n| n.qualified_name.as_str())
                .unwrap_or("");
            let bq2 = nodes_by_id
                .get(&b.to)
                .map(|n| n.qualified_name.as_str())
                .unwrap_or("");
            aq2.cmp(bq2)
        })
    });

    for edge in edges_sorted {
        let (arrow, label_kind) = match edge.kind {
            EdgeKind::Calls => ("-->", "calls"),
            EdgeKind::Uses => ("-.->", "uses"),
            // Not produced by the current algorithm; if a future version
            // emits them, skip rather than mis-render.
            EdgeKind::Imports | EdgeKind::Contains => continue,
        };
        let from = short_node_id(edge.from);
        let to = short_node_id(edge.to);
        let label = if edge.weight > 1 {
            format!("{label_kind} (×{})", edge.weight)
        } else {
            label_kind.to_string()
        };
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("  n_{from} {arrow}|{}| n_{to}\n", escape_label(&label)),
        );
    }

    out.push_str("  classDef seed fill:#fde68a,stroke:#92400e\n");
    out
}

/// Render a `Codemap` as a flat indented outline.
///
/// Format: one line per retained node, sorted by qualified name, indented
/// by `::`-segment depth (two spaces per level). Seeds are prefixed with
/// `* ` instead of two spaces at their indent level; non-seeds use plain
/// space indent. Each line:
///
/// ```text
/// <indent><qualified_name>  [<item_kind>]  <file>:<line>
/// ```
///
/// `item_kind` falls back to the higher-level `NodeKind` string when the
/// `Option<ItemKind>` is None. The trailing `<file>:<line>` is omitted
/// entirely when neither a file nor a span is recorded. When the line
/// number is available (resolved during `build_codemap` via
/// `OpenedSnapshot::line_to_byte`) the form is `<file>:<line>`. If the
/// node has a span but no line (snapshot-less render, file unreadable,
/// etc.) the form falls back to `<file>@<byte_offset>`.
///
/// When `CodemapNode.snippet` is `Some`, each snippet line is appended
/// under the item, prefixed with `        | ` (8 spaces + `| `) to make
/// the snippet visually distinct from the structural outline.
pub(crate) fn render_outline(cm: &Codemap) -> String {
    // Build a qualified-name -> &CodemapNode lookup so the recursive walk
    // can decide which `ModuleTreeNode`s correspond to retained items.
    let retained_by_qn: HashMap<&str, &CodemapNode> = cm
        .nodes
        .iter()
        .map(|n| (n.qualified_name.as_str(), n))
        .collect();

    // Recurse the hierarchy tree; indent is derived from traversal depth so
    // items at the same logical hierarchy level always align (pass-2 #A2).
    // Modules that contain no retained descendants are already filtered out
    // by `project_hierarchy`, so a plain pre-order walk is sufficient.
    fn emit(
        node: &ModuleTreeNode,
        depth: usize,
        out: &mut String,
        retained_by_qn: &HashMap<&str, &CodemapNode>,
    ) {
        if let Some(cn) = retained_by_qn.get(node.qualified_name.as_str()).copied() {
            let kind_label = cn
                .item_kind
                .map(|k| format!("{k:?}"))
                .unwrap_or_else(|| format!("{:?}", cn.kind));

            // Indent: 2 spaces per depth level. Seed marker replaces the
            // final two spaces with "* " (or prepends "* " at depth 0).
            let indent = "  ".repeat(depth);
            let prefix = if cn.is_seed {
                if depth == 0 {
                    "* ".to_string()
                } else {
                    format!("{}* ", &indent[..indent.len() - 2])
                }
            } else {
                indent
            };

            let location = match (&cn.file, cn.span, cn.line) {
                (Some(file), _, Some(line)) => format!("  {file}:{line}"),
                (Some(file), Some((start_byte, _)), None) => format!("  {file}@{start_byte}"),
                (Some(file), None, None) => format!("  {file}"),
                _ => String::new(),
            };

            let _ = std::fmt::Write::write_fmt(
                out,
                format_args!(
                    "{prefix}{}  [{kind_label}]{location}\n",
                    cn.qualified_name
                ),
            );

            if let Some(snippet) = &cn.snippet {
                for line in snippet.lines() {
                    let _ = std::fmt::Write::write_fmt(
                        out,
                        format_args!("        | {line}\n"),
                    );
                }
            }
        }
        for child in &node.children {
            emit(child, depth + 1, out, retained_by_qn);
        }
    }

    let mut out = String::new();
    emit(&cm.hierarchy, 0, &mut out, &retained_by_qn);
    out
}

/// First 8 hex chars of a NodeId — short enough for Mermaid identifiers,
/// long enough to avoid collisions in a max_nodes=500 codemap.
fn short_node_id(nid: NodeId) -> String {
    let bytes = nid.as_bytes();
    let mut s = String::with_capacity(8);
    for b in &bytes[..4] {
        let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{b:02x}"));
    }
    s
}

/// Sanitize a qualified-name fragment for use as a Mermaid identifier.
/// Replaces every char that's not `[A-Za-z0-9_]` with `_`.
fn sanitize_mermaid_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

/// Escape characters in a Mermaid label string that would break the
/// `["..."]` form. Only `"` matters in v1.
fn escape_label(s: &str) -> String {
    s.replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graph::codemap::model::CodemapOptions;
    use crate::graph::codemap::test_support::{hand_built_codemap, shared_fixture};

    #[test]
    fn render_mermaid_against_hand_built() {
        let cm = hand_built_codemap();
        let m = render_mermaid(&cm);

        assert!(
            m.starts_with("flowchart LR\n"),
            "mermaid must start with 'flowchart LR\\n', got:\n{m}"
        );
        // Both nodes live under `demo_crate`, so a single subgraph
        // header is emitted with the module label.
        assert!(
            m.contains("\"mod demo_crate\""),
            "expected 'mod demo_crate' subgraph header, got:\n{m}"
        );
        // The seed (caller) node carries the `:::seed` class suffix.
        assert!(
            m.contains("[\"caller\"]:::seed"),
            "expected seed-classed caller node, got:\n{m}"
        );
        // The non-seed callee node renders without a class suffix.
        assert!(
            m.contains("[\"callee\"]\n"),
            "expected plain callee node, got:\n{m}"
        );
        // Exactly one Calls edge -> `-->` arrow with `calls` label.
        assert!(
            m.contains("-->|calls|"),
            "expected '-->|calls|' edge, got:\n{m}"
        );
        // Closing classDef block is always emitted.
        assert!(
            m.contains("classDef seed fill:#fde68a"),
            "expected classDef seed block, got:\n{m}"
        );
    }

    #[test]
    fn render_outline_against_hand_built() {
        let cm = hand_built_codemap();
        let o = render_outline(&cm);

        // Sorted by qualified name: callee (non-seed) appears first.
        let mut lines = o.lines();
        let callee_line = lines.next().expect("at least one outline line");
        let caller_line = lines.next().expect("at least two outline lines");

        // Non-seed line: two-space indent (depth=1, one `::`) plus
        // the qualified name, kind, and file:line tail.
        assert!(
            callee_line.contains("demo_crate::callee  [Function]  src/lib.rs:1"),
            "expected callee outline line, got: {callee_line}"
        );
        // Seed line: "* " replaces the last two indent spaces.
        assert!(
            caller_line.starts_with("* demo_crate::caller"),
            "expected seed marker on caller line, got: {caller_line}"
        );
        assert!(
            caller_line.contains("[Function]") && caller_line.contains("src/lib.rs:1"),
            "expected kind+location on caller line, got: {caller_line}"
        );
    }

    #[tokio::test]
    async fn render_mermaid_smoke() {
        let fixture = shared_fixture();
        let names = vec!["synthetic_codemap_crate::caller".to_string()];
        let opts = CodemapOptions::default();
        let cm = crate::graph::codemap::build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
            .await
            .expect("build_codemap succeeds for mermaid smoke test");

        let m = render_mermaid(&cm);
        assert!(m.starts_with("flowchart LR\n"));
        // The seed node should be marked with the ":::seed" class.
        assert!(m.contains(":::seed"));
        // The classDef block is always emitted at the end.
        assert!(m.contains("classDef seed"));
        // The caller's parent module is the crate root, so the subgraph
        // header carries "mod synthetic_codemap_crate".
        assert!(m.contains("\"mod synthetic_codemap_crate\""));
    }

    #[tokio::test]
    async fn render_outline_smoke() {
        let fixture = shared_fixture();
        let names = vec!["synthetic_codemap_crate::caller".to_string()];
        let opts = CodemapOptions::default();
        let cm = crate::graph::codemap::build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
            .await
            .expect("build_codemap succeeds for outline smoke test");

        let o = render_outline(&cm);
        // The seed line carries a "* " marker before the qualified name.
        assert!(
            o.lines().any(|l| l.contains("* synthetic_codemap_crate::caller")),
            "expected seed marker in outline output, got:\n{o}"
        );
    }
}
