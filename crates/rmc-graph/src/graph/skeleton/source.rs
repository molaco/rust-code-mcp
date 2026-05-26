use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use ra_ap_syntax::{AstNode, SourceFile, SyntaxNode, TextRange, TextSize, ast};

use crate::graph::model::{ItemKind, Node};

use super::model::{SkeletonDiagnostic, SkeletonItem, SkeletonOptions};

pub(super) struct SourceCache {
    workspace_root: PathBuf,
    files: HashMap<String, Result<CachedSource, ()>>,
}

struct CachedSource {
    text: String,
    parsed: SourceFile,
}

#[derive(Clone, Copy)]
struct Replacement {
    range: TextRange,
    text: &'static str,
}

impl SourceCache {
    pub(super) fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            files: HashMap::new(),
        }
    }

    pub(super) fn render_item(
        &mut self,
        item: &SkeletonItem,
        opts: &SkeletonOptions,
        diagnostics: &mut Vec<SkeletonDiagnostic>,
    ) -> String {
        let Some(file) = item.node.file.as_deref() else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "item `{}` has no source file; rendered fallback declaration",
                    item.node.qualified_name,
                ),
            });
            return fallback_declaration(item);
        };
        let Some(source) = self.source(file, diagnostics) else {
            return fallback_declaration(item);
        };
        let Some(syntax) = find_item_syntax(&source.parsed, &item.node) else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "could not find source syntax for `{}` in `{}`; rendered fallback declaration",
                    item.node.qualified_name, file,
                ),
            });
            return fallback_declaration(item);
        };
        render_source_item_text(
            &source.text,
            &syntax,
            &item.node.attributes,
            opts,
        )
    }

    fn source(
        &mut self,
        file: &str,
        diagnostics: &mut Vec<SkeletonDiagnostic>,
    ) -> Option<&CachedSource> {
        if !self.files.contains_key(file) {
            let path = self.workspace_root.join(file);
            let entry = match fs::read_to_string(&path) {
                Ok(text) => {
                    let parse = SourceFile::parse(
                        &text,
                        ra_ap_syntax::Edition::Edition2024,
                    );
                    let parsed_errors = parse.errors().len();
                    let parsed = parse.tree();
                    if parsed_errors > 0 {
                        diagnostics.push(SkeletonDiagnostic {
                            message: format!(
                                "source `{file}` parsed with {parsed_errors} syntax errors",
                            ),
                        });
                    }
                    Ok(CachedSource { text, parsed })
                }
                Err(err) => {
                    diagnostics.push(SkeletonDiagnostic {
                        message: format!(
                            "could not read source `{}` at `{}`: {err}",
                            file,
                            path.display(),
                        ),
                    });
                    Err(())
                }
            };
            self.files.insert(file.to_string(), entry);
        }
        self.files.get(file).and_then(|entry| entry.as_ref().ok())
    }
}

pub(super) fn render_source_item_text(
    source: &str,
    syntax: &SyntaxNode,
    attrs: &[String],
    opts: &SkeletonOptions,
) -> String {
    let range = syntax.text_range();
    let mut rendered = slice_range(source, range).to_string();
    let replacements = non_overlapping_replacements(collect_body_replacements(syntax));
    let base = u32::from(range.start());
    for replacement in replacements {
        let start = (u32::from(replacement.range.start()) - base) as usize;
        let end = (u32::from(replacement.range.end()) - base) as usize;
        rendered.replace_range(start..end, replacement.text);
    }

    let declaration = strip_leading_attrs_docs(&rendered).trim().to_string();
    let mut out = String::new();
    for attr in attrs.iter().filter(|attr| keep_attr(attr, opts)) {
        out.push_str(attr);
        out.push('\n');
    }
    out.push_str(&declaration);
    out
}

pub(super) fn find_item_syntax(parsed: &SourceFile, node: &Node) -> Option<SyntaxNode> {
    let item_kind = node.item_kind?;
    let (start, end) = node.span?;
    let wanted = TextRange::new(TextSize::from(start), TextSize::from(end));
    let mut best_covering: Option<SyntaxNode> = None;
    let mut best_inside: Option<SyntaxNode> = None;

    for syntax in parsed.syntax().descendants() {
        if !is_expected_item_syntax(item_kind, &syntax) {
            continue;
        }
        let range = syntax.text_range();
        if range == wanted {
            return Some(syntax);
        }
        if range.contains_range(wanted) {
            if best_covering
                .as_ref()
                .map(|best| range_len(range) < range_len(best.text_range()))
                .unwrap_or(true)
            {
                best_covering = Some(syntax);
            }
        } else if wanted.contains_range(range)
            && best_inside
                .as_ref()
                .map(|best| range_len(range) > range_len(best.text_range()))
                .unwrap_or(true)
        {
            best_inside = Some(syntax);
        }
    }

    best_covering.or(best_inside)
}

fn is_expected_item_syntax(item_kind: ItemKind, syntax: &SyntaxNode) -> bool {
    match item_kind {
        ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction => {
            ast::Fn::cast(syntax.clone()).is_some()
        }
        ItemKind::Struct => ast::Struct::cast(syntax.clone()).is_some(),
        ItemKind::Enum => ast::Enum::cast(syntax.clone()).is_some(),
        ItemKind::Union => ast::Union::cast(syntax.clone()).is_some(),
        ItemKind::Trait => ast::Trait::cast(syntax.clone()).is_some(),
        ItemKind::TypeAlias | ItemKind::AssocType => {
            ast::TypeAlias::cast(syntax.clone()).is_some()
        }
        ItemKind::Const | ItemKind::AssocConst => ast::Const::cast(syntax.clone()).is_some(),
        ItemKind::Static => ast::Static::cast(syntax.clone()).is_some(),
        ItemKind::EnumVariant => ast::Variant::cast(syntax.clone()).is_some(),
    }
}

fn collect_body_replacements(syntax: &SyntaxNode) -> Vec<Replacement> {
    let mut replacements = Vec::new();
    for node in syntax.descendants() {
        if let Some(function) = ast::Fn::cast(node.clone()) {
            if let Some(body) = function.body() {
                replacements.push(Replacement {
                    range: body.syntax().text_range(),
                    text: "{ /* ... */ }",
                });
            }
            continue;
        }
        if let Some(const_) = ast::Const::cast(node.clone()) {
            if let Some(body) = const_.body() {
                replacements.push(Replacement {
                    range: body.syntax().text_range(),
                    text: "todo!()",
                });
            }
            continue;
        }
        if let Some(static_) = ast::Static::cast(node.clone()) {
            if let Some(body) = static_.body() {
                replacements.push(Replacement {
                    range: body.syntax().text_range(),
                    text: "todo!()",
                });
            }
        }
    }
    replacements
}

fn non_overlapping_replacements(mut replacements: Vec<Replacement>) -> Vec<Replacement> {
    replacements.sort_by(|a, b| {
        range_len(b.range)
            .cmp(&range_len(a.range))
            .then_with(|| a.range.start().cmp(&b.range.start()))
    });
    let mut kept: Vec<Replacement> = Vec::new();
    for replacement in replacements {
        if kept
            .iter()
            .all(|kept| !ranges_overlap(kept.range, replacement.range))
        {
            kept.push(replacement);
        }
    }
    kept.sort_by(|a, b| b.range.start().cmp(&a.range.start()));
    kept
}

fn ranges_overlap(a: TextRange, b: TextRange) -> bool {
    a.start() < b.end() && b.start() < a.end()
}

fn range_len(range: TextRange) -> u32 {
    u32::from(range.end()) - u32::from(range.start())
}

fn slice_range(source: &str, range: TextRange) -> &str {
    let start = u32::from(range.start()) as usize;
    let end = u32::from(range.end()) as usize;
    &source[start..end]
}

fn keep_attr(attr: &str, opts: &SkeletonOptions) -> bool {
    if is_doc_attr(attr) {
        opts.include_docs
    } else {
        opts.include_attrs
    }
}

fn is_doc_attr(attr: &str) -> bool {
    let trimmed = attr.trim_start();
    trimmed.starts_with("///")
        || trimmed.starts_with("//!")
        || trimmed.starts_with("/**")
        || trimmed.starts_with("/*!")
}

fn strip_leading_attrs_docs(text: &str) -> &str {
    let mut idx = 0;
    loop {
        idx += leading_whitespace_len(&text[idx..]);
        let rest = &text[idx..];
        if rest.starts_with("///") || rest.starts_with("//!") {
            idx += rest.find('\n').map(|pos| pos + 1).unwrap_or(rest.len());
            continue;
        }
        if rest.starts_with("/**") || rest.starts_with("/*!") {
            let Some(end) = rest.find("*/") else {
                return "";
            };
            idx += end + 2;
            continue;
        }
        if rest.starts_with("#[") {
            let Some(end) = outer_attr_len(rest) else {
                break;
            };
            idx += end;
            continue;
        }
        break;
    }
    &text[idx..]
}

fn leading_whitespace_len(text: &str) -> usize {
    text.char_indices()
        .find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))
        .unwrap_or(text.len())
}

fn outer_attr_len(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(idx + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn fallback_declaration(item: &SkeletonItem) -> String {
    let vis = visibility_prefix(item.visibility.as_deref());
    let name = item.node.display_name.as_str();
    match item.node.item_kind {
        Some(ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction) => {
            format!("{vis}fn {name}() {{ /* ... */ }}")
        }
        Some(ItemKind::Struct) => format!("{vis}struct {name};"),
        Some(ItemKind::Enum) => format!("{vis}enum {name} {{}}"),
        Some(ItemKind::Union) => format!("{vis}union {name} {{ _skeleton: () }}"),
        Some(ItemKind::Trait) => format!("{vis}trait {name} {{}}"),
        Some(ItemKind::TypeAlias | ItemKind::AssocType) => {
            format!("{vis}type {name} = ();")
        }
        Some(ItemKind::Const | ItemKind::AssocConst) => {
            format!("{vis}const {name}: () = ();")
        }
        Some(ItemKind::Static) => format!("{vis}static {name}: () = ();"),
        Some(ItemKind::EnumVariant) | None => {
            format!("// item `{}` could not be rendered", item.node.qualified_name)
        }
    }
}

fn visibility_prefix(visibility: Option<&str>) -> String {
    match visibility {
        Some("pub(self)") | None => String::new(),
        Some(vis) => format!("{vis} "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_syntax(src: &str, kind: ItemKind) -> SyntaxNode {
        let parsed = SourceFile::parse(src, ra_ap_syntax::Edition::Edition2024).tree();
        parsed
            .syntax()
            .descendants()
            .find(|syntax| is_expected_item_syntax(kind, syntax))
            .expect("expected item syntax")
    }

    fn render(src: &str, kind: ItemKind, attrs: &[&str], opts: SkeletonOptions) -> String {
        let syntax = first_syntax(src, kind);
        let attrs: Vec<String> = attrs.iter().map(|attr| attr.to_string()).collect();
        render_source_item_text(src, &syntax, &attrs, &opts)
    }

    #[test]
    fn strips_function_body_and_reemits_filtered_attrs() {
        let src = "#[inline]\n/// docs\npub fn f() -> i32 { 1 + 2 }\n";
        let rendered = render(
            src,
            ItemKind::Function,
            &["#[inline]", "/// docs"],
            SkeletonOptions {
                include_docs: true,
                include_attrs: false,
                ..Default::default()
            },
        );
        assert!(rendered.starts_with("/// docs\npub fn f()"));
        assert!(!rendered.contains("#[inline]"));
        assert!(rendered.contains("{ /* ... */ }"));
        assert!(!rendered.contains("1 + 2"));
    }

    #[test]
    fn trait_methods_keep_semicolon_or_strip_default_body() {
        let src = "pub trait T {\n    fn a(&self);\n    fn b(&self) -> usize { 1 }\n}\n";
        let rendered = render(src, ItemKind::Trait, &[], SkeletonOptions::default());
        assert!(rendered.contains("fn a(&self);"));
        assert!(rendered.contains("fn b(&self) -> usize { /* ... */ }"));
        assert!(!rendered.contains("{ 1 }"));
    }

    #[test]
    fn const_and_static_initializers_become_placeholder_exprs() {
        let const_src = "pub const X: usize = 1 + 2;\n";
        let static_src = "pub static Y: usize = 3;\n";
        let rendered_const = render(
            const_src,
            ItemKind::Const,
            &[],
            SkeletonOptions::default(),
        );
        let rendered_static = render(
            static_src,
            ItemKind::Static,
            &[],
            SkeletonOptions::default(),
        );
        assert_eq!(rendered_const, "pub const X: usize = todo!();");
        assert_eq!(rendered_static, "pub static Y: usize = todo!();");
    }
}
