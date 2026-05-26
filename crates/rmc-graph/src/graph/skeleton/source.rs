use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use ra_ap_syntax::{
    AstNode, SourceFile, SyntaxNode, TextRange, TextSize,
    ast::{self, HasName},
};

use crate::graph::model::{
    FunctionSignature, ItemKind, Node, SelfKind, StaticMetadata,
};
use crate::graph::snapshot::OpenedSnapshot;

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
        snap: &OpenedSnapshot,
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
            return fallback_declaration(item, &fallback_metadata(snap, item, diagnostics));
        };
        let Some(source) = self.source(file, diagnostics) else {
            return fallback_declaration(item, &fallback_metadata(snap, item, diagnostics));
        };
        let Some(syntax) = find_item_syntax(&source.parsed, &item.node) else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "could not find source syntax for `{}` in `{}`; rendered fallback declaration",
                    item.node.qualified_name, file,
                ),
            });
            return fallback_declaration(item, &fallback_metadata(snap, item, diagnostics));
        };
        render_source_item_text(
            &source.text,
            &syntax,
            &item.node.attributes,
            opts,
        )
    }

    #[cfg(test)]
    fn render_item_with_metadata(
        &mut self,
        item: &SkeletonItem,
        opts: &SkeletonOptions,
        diagnostics: &mut Vec<SkeletonDiagnostic>,
        fallback: &FallbackMetadata,
    ) -> String {
        let Some(file) = item.node.file.as_deref() else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "item `{}` has no source file; rendered fallback declaration",
                    item.node.qualified_name,
                ),
            });
            return fallback_declaration(item, fallback);
        };
        let Some(source) = self.source(file, diagnostics) else {
            return fallback_declaration(item, fallback);
        };
        let Some(syntax) = find_item_syntax(&source.parsed, &item.node) else {
            diagnostics.push(SkeletonDiagnostic {
                message: format!(
                    "could not find source syntax for `{}` in `{}`; rendered fallback declaration",
                    item.node.qualified_name, file,
                ),
            });
            return fallback_declaration(item, fallback);
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
                    if parsed_errors > 0 {
                        diagnostics.push(SkeletonDiagnostic {
                            message: format!(
                                "source `{file}` parsed with {parsed_errors} syntax errors; rendered fallback declarations",
                            ),
                        });
                        Err(())
                    } else {
                        let parsed = parse.tree();
                        Ok(CachedSource { text, parsed })
                    }
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
        if !matches_node_name(item_kind, &syntax, node) {
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

fn matches_node_name(item_kind: ItemKind, syntax: &SyntaxNode, node: &Node) -> bool {
    declaration_name(item_kind, syntax)
        .map(|name| name == node.display_name.as_str())
        .unwrap_or(true)
}

fn declaration_name(item_kind: ItemKind, syntax: &SyntaxNode) -> Option<String> {
    match item_kind {
        ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction => {
            ast::Fn::cast(syntax.clone())?.name()
        }
        ItemKind::Struct => ast::Struct::cast(syntax.clone())?.name(),
        ItemKind::Enum => ast::Enum::cast(syntax.clone())?.name(),
        ItemKind::Union => ast::Union::cast(syntax.clone())?.name(),
        ItemKind::Trait => ast::Trait::cast(syntax.clone())?.name(),
        ItemKind::TypeAlias | ItemKind::AssocType => {
            ast::TypeAlias::cast(syntax.clone())?.name()
        }
        ItemKind::Const | ItemKind::AssocConst => ast::Const::cast(syntax.clone())?.name(),
        ItemKind::Static => ast::Static::cast(syntax.clone())?.name(),
        ItemKind::EnumVariant => ast::Variant::cast(syntax.clone())?.name(),
    }
    .map(|name| name.text().to_string())
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

#[derive(Default)]
struct FallbackMetadata {
    signature: Option<FunctionSignature>,
    static_metadata: Option<StaticMetadata>,
}

fn fallback_metadata(
    snap: &OpenedSnapshot,
    item: &SkeletonItem,
    diagnostics: &mut Vec<SkeletonDiagnostic>,
) -> FallbackMetadata {
    match item.node.item_kind {
        Some(ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction) => {
            match snap.function_signature(item.id) {
                Ok(signature) => FallbackMetadata {
                    signature,
                    static_metadata: None,
                },
                Err(err) => {
                    diagnostics.push(SkeletonDiagnostic {
                        message: format!(
                            "could not load persisted signature for `{}`: {err}",
                            item.node.qualified_name,
                        ),
                    });
                    FallbackMetadata::default()
                }
            }
        }
        Some(ItemKind::Static) => match snap.static_metadata(item.id) {
            Ok(static_metadata) => FallbackMetadata {
                signature: None,
                static_metadata,
            },
            Err(err) => {
                diagnostics.push(SkeletonDiagnostic {
                    message: format!(
                        "could not load persisted static metadata for `{}`: {err}",
                        item.node.qualified_name,
                    ),
                });
                FallbackMetadata::default()
            }
        },
        _ => FallbackMetadata::default(),
    }
}

fn fallback_declaration(item: &SkeletonItem, metadata: &FallbackMetadata) -> String {
    let vis = visibility_prefix(item.visibility.as_deref());
    let name = item.node.display_name.as_str();
    match item.node.item_kind {
        Some(ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction) => {
            if let Some(signature) = &metadata.signature {
                fallback_function_declaration(&vis, name, signature)
            } else {
                format!("{vis}fn {name}() {{ /* ... */ }}")
            }
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
        Some(ItemKind::Static) => {
            if let Some(metadata) = &metadata.static_metadata {
                fallback_static_declaration(&vis, name, metadata)
            } else {
                format!("{vis}static {name}: () = ();")
            }
        }
        Some(ItemKind::EnumVariant) | None => {
            format!("// item `{}` could not be rendered", item.node.qualified_name)
        }
    }
}

fn fallback_function_declaration(
    vis: &str,
    name: &str,
    signature: &FunctionSignature,
) -> String {
    let async_prefix = if signature.is_async { "async " } else { "" };
    let generics = render_generics(&signature.generics);
    let params = render_function_params(signature);
    let return_type = render_return_type(&signature.return_type);
    format!(
        "{vis}{async_prefix}fn {name}{generics}({params}){return_type} {{ /* ... */ }}"
    )
}

fn render_generics(generics: &[crate::graph::model::GenericBound]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let rendered = generics
        .iter()
        .map(|generic| {
            if generic.bounds.is_empty() {
                generic.name.clone()
            } else {
                format!("{}: {}", generic.name, generic.bounds.join(" + "))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{rendered}>")
}

fn render_function_params(signature: &FunctionSignature) -> String {
    let mut params = Vec::new();
    if let Some(self_param) = signature.self_param {
        params.push(match self_param {
            SelfKind::Owned => "self".to_string(),
            SelfKind::Ref => "&self".to_string(),
            SelfKind::RefMut => "&mut self".to_string(),
        });
    }
    params.extend(
        signature
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| render_param(idx, param)),
    );
    params.join(", ")
}

fn render_param(idx: usize, param: &crate::graph::model::Param) -> String {
    let name = if param.name.is_empty() {
        format!("_arg{idx}")
    } else {
        param.name.clone()
    };
    let ty = if param.ty.is_empty() {
        "()"
    } else {
        param.ty.as_str()
    };
    format!("{name}: {ty}")
}

fn render_return_type(return_type: &str) -> String {
    let trimmed = return_type.trim();
    if trimmed.is_empty() || trimmed == "()" {
        String::new()
    } else {
        format!(" -> {trimmed}")
    }
}

fn fallback_static_declaration(
    vis: &str,
    name: &str,
    metadata: &StaticMetadata,
) -> String {
    let mutability = if metadata.is_mut { "mut " } else { "" };
    let ty = if metadata.type_string.is_empty() {
        "()"
    } else {
        metadata.type_string.as_str()
    };
    format!("{vis}static {mutability}{name}: {ty} = todo!();")
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
    use crate::graph::ids::NodeId;
    use crate::graph::model::{GenericBound, Node, NodeKind, Param};

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

    fn item_syntax_range(src: &str, kind: ItemKind, name: &str) -> (u32, u32) {
        let parsed = SourceFile::parse(src, ra_ap_syntax::Edition::Edition2024).tree();
        let syntax = parsed
            .syntax()
            .descendants()
            .find(|syntax| {
                is_expected_item_syntax(kind, syntax)
                    && declaration_name(kind, syntax).as_deref() == Some(name)
            })
            .expect("expected named item syntax");
        let range = syntax.text_range();
        (u32::from(range.start()), u32::from(range.end()))
    }

    fn item_with_source(file: Option<&str>, span: Option<(u32, u32)>) -> SkeletonItem {
        SkeletonItem {
            id: NodeId::from_components(&["skeleton-test", "f"]),
            node: Node {
                id: NodeId::from_components(&["skeleton-test", "f"]),
                kind: NodeKind::Item,
                display_name: "f".to_string(),
                qualified_name: "test_crate::f".to_string(),
                crate_id: None,
                parent_id: None,
                item_kind: Some(ItemKind::Function),
                file: file.map(str::to_string),
                span,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            },
            parent: None,
            visibility: Some("pub".to_string()),
        }
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

    #[test]
    fn missing_source_file_reports_diagnostic_and_falls_back() {
        let td = tempfile::tempdir().expect("tempdir");
        let mut cache = SourceCache::new(td.path());
        let mut diagnostics = Vec::new();
        let rendered = cache.render_item_with_metadata(
            &item_with_source(Some("src/lib.rs"), Some((0, 10))),
            &SkeletonOptions::default(),
            &mut diagnostics,
            &FallbackMetadata::default(),
        );
        assert_eq!(rendered, "pub fn f() { /* ... */ }");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("could not read source")),
            "expected missing-source diagnostic, got {diagnostics:?}",
        );
    }

    #[test]
    fn missing_span_reports_diagnostic_and_falls_back() {
        let td = tempfile::tempdir().expect("tempdir");
        let src_dir = td.path().join("src");
        std::fs::create_dir_all(&src_dir).expect("create src dir");
        std::fs::write(src_dir.join("lib.rs"), "pub fn f() { 1 }\n")
            .expect("write source");

        let mut cache = SourceCache::new(td.path());
        let mut diagnostics = Vec::new();
        let rendered = cache.render_item_with_metadata(
            &item_with_source(Some("src/lib.rs"), None),
            &SkeletonOptions::default(),
            &mut diagnostics,
            &FallbackMetadata::default(),
        );
        assert_eq!(rendered, "pub fn f() { /* ... */ }");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("could not find source syntax")),
            "expected missing-span fallback diagnostic, got {diagnostics:?}",
        );
    }

    #[test]
    fn stale_span_name_mismatch_reports_diagnostic_and_falls_back() {
        let td = tempfile::tempdir().expect("tempdir");
        let src_dir = td.path().join("src");
        std::fs::create_dir_all(&src_dir).expect("create src dir");
        let src = "pub fn g() -> i32 { 1 }\n\npub fn f() -> i32 { 2 }\n";
        std::fs::write(src_dir.join("lib.rs"), src).expect("write source");
        let (start, end) = item_syntax_range(src, ItemKind::Function, "g");

        let mut cache = SourceCache::new(td.path());
        let mut diagnostics = Vec::new();
        let rendered = cache.render_item_with_metadata(
            &item_with_source(Some("src/lib.rs"), Some((start, end + 1))),
            &SkeletonOptions::default(),
            &mut diagnostics,
            &FallbackMetadata::default(),
        );
        assert_eq!(rendered, "pub fn f() { /* ... */ }");
        assert!(!rendered.contains("pub fn g"));
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("could not find source syntax")),
            "expected stale-span fallback diagnostic, got {diagnostics:?}",
        );
    }

    #[test]
    fn parse_error_source_reports_diagnostic_and_falls_back() {
        let td = tempfile::tempdir().expect("tempdir");
        let src_dir = td.path().join("src");
        std::fs::create_dir_all(&src_dir).expect("create src dir");
        let src = "pub fn f() -> i32 { 1\n";
        std::fs::write(src_dir.join("lib.rs"), src).expect("write source");

        let mut cache = SourceCache::new(td.path());
        let mut diagnostics = Vec::new();
        let rendered = cache.render_item_with_metadata(
            &item_with_source(Some("src/lib.rs"), Some((0, src.len() as u32))),
            &SkeletonOptions::default(),
            &mut diagnostics,
            &FallbackMetadata::default(),
        );
        assert_eq!(rendered, "pub fn f() { /* ... */ }");
        assert!(!rendered.contains("-> i32"));
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("parsed with")
                    && diagnostic.message.contains("syntax errors")
            }),
            "expected parse-error fallback diagnostic, got {diagnostics:?}",
        );
    }

    #[test]
    fn fallback_function_uses_signature_shape() {
        let item = item_with_source(None, None);
        let metadata = FallbackMetadata {
            signature: Some(FunctionSignature {
                is_async: true,
                self_param: None,
                params: vec![
                    Param {
                        name: "path".to_string(),
                        ty: "&Path".to_string(),
                        by_ref: true,
                        mutability: false,
                    },
                    Param {
                        name: "value".to_string(),
                        ty: "T".to_string(),
                        by_ref: false,
                        mutability: false,
                    },
                ],
                return_type: "Result<T>".to_string(),
                generics: vec![GenericBound {
                    name: "T".to_string(),
                    bounds: vec!["Send".to_string(), "Sync".to_string()],
                }],
            }),
            static_metadata: None,
        };
        let rendered = fallback_declaration(&item, &metadata);
        assert_eq!(
            rendered,
            "pub async fn f<T: Send + Sync>(path: &Path, value: T) -> Result<T> { /* ... */ }",
        );
        let parsed = SourceFile::parse(&rendered, ra_ap_syntax::Edition::Edition2024);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    }

    #[test]
    fn fallback_method_uses_self_receiver() {
        let mut item = item_with_source(None, None);
        item.node.item_kind = Some(ItemKind::Method);
        item.node.display_name = "update".to_string();
        let metadata = FallbackMetadata {
            signature: Some(FunctionSignature {
                is_async: false,
                self_param: Some(SelfKind::RefMut),
                params: vec![Param {
                    name: "next".to_string(),
                    ty: "State".to_string(),
                    by_ref: false,
                    mutability: false,
                }],
                return_type: "bool".to_string(),
                generics: Vec::new(),
            }),
            static_metadata: None,
        };
        let rendered = fallback_declaration(&item, &metadata);
        assert_eq!(
            rendered,
            "pub fn update(&mut self, next: State) -> bool { /* ... */ }",
        );
        let parsed = SourceFile::parse(
            &format!("struct Host;\nimpl Host {{ {rendered} }}"),
            ra_ap_syntax::Edition::Edition2024,
        );
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    }

    #[test]
    fn fallback_static_uses_static_metadata() {
        let mut item = item_with_source(None, None);
        item.node.item_kind = Some(ItemKind::Static);
        item.node.display_name = "CACHE".to_string();
        let metadata = FallbackMetadata {
            signature: None,
            static_metadata: Some(StaticMetadata {
                type_string: "OnceLock<String>".to_string(),
                is_mut: true,
            }),
        };
        let rendered = fallback_declaration(&item, &metadata);
        assert_eq!(rendered, "pub static mut CACHE: OnceLock<String> = todo!();");
        let parsed = SourceFile::parse(&rendered, ra_ap_syntax::Edition::Edition2024);
        assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    }
}
