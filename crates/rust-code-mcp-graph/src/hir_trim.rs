//! Post-process HirDisplay output to strip std-library default type
//! parameters that add noise without signal.
//!
//! Targeted patterns:
//!   - `, Global>` at the end of a generic — `std::alloc::Global`, the
//!     default allocator. `Vec<T, Global>` -> `Vec<T>`.
//!   - `, RandomState, Global>` — `HashMap` with the default
//!     state and allocator. `HashMap<K, V, RandomState, Global>` ->
//!     `HashMap<K, V>`.
//!   - `, BuildHasherDefault<...>, Global>` — `HashMap` with a custom
//!     hasher (e.g. `FxHasher`). The hasher is signal in some contexts
//!     but the trailing `Global` allocator is still pure noise; we trim
//!     both for now (cleaner output for the 99% case).
//!   - `LazyLock<X, fn() -> X>` -> `LazyLock<X>` — strip the init-fn
//!     pointer, which HirDisplay always renders even though the source
//!     elided it.
//!
//! **Risk:** if a user-defined type happens to be named exactly `Global`
//! or `RandomState`, the trim would mangle its rendering. We accept the
//! risk for now — the benefit (cleaner output for the 99% case)
//! outweighs the downside, and a `tracing::trace!` fires when the
//! trimmer produces unbalanced angle brackets so issues are debuggable.

/// Strip std-library default type parameters from a HirDisplay-rendered
/// type string. Idempotent. Linear in the input length (a few passes
/// over short strings — these are at most a few hundred chars).
pub(crate) fn trim_hir_display(s: &str) -> String {
    let mut out = s.to_string();

    // 1. Strip `, Global>` repeatedly so nested `Vec<Vec<T, Global>, Global>`
    //    trims down to `Vec<Vec<T>>`. Bounded by the input length.
    while let Some(idx) = out.find(", Global>") {
        out.replace_range(idx..idx + ", Global>".len(), ">");
    }

    // 2. After step 1, a `HashMap<K, V, RandomState, Global>` has become
    //    `HashMap<K, V, RandomState>`. Strip the trailing `, RandomState>`.
    while let Some(idx) = out.find(", RandomState>") {
        out.replace_range(idx..idx + ", RandomState>".len(), ">");
    }

    // 3. Same for `BuildHasherDefault<...>` — strip ", BuildHasherDefault<X>>"
    //    where X is any inner type. Use a small state machine to find the
    //    matching `>` since X may itself contain `<...>`.
    out = strip_build_hasher_default(&out);

    // 4. `LazyLock<X, fn() -> X>` -> `LazyLock<X>` (only when X == X).
    out = strip_lazy_lock_init_fn(&out);

    // Defensive: warn if we produced unbalanced angle brackets (a sign
    // that one of the trims went off the rails on a pathological input).
    let opens = out.bytes().filter(|&b| b == b'<').count();
    let closes = out.bytes().filter(|&b| b == b'>').count();
    if opens != closes {
        tracing::trace!(
            "trim_hir_display: unbalanced angle brackets after trim — input=`{s}` output=`{out}` opens={opens} closes={closes}"
        );
    }

    out
}

/// Find every `, BuildHasherDefault<...>>` and replace with `>`. The inner
/// `<...>` may itself nest, so we walk angle-bracket depth.
fn strip_build_hasher_default(s: &str) -> String {
    let needle = ", BuildHasherDefault<";
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    while let Some(rel) = s[cursor..].find(needle) {
        let start = cursor + rel;
        out.push_str(&s[cursor..start]);
        // Walk from after `BuildHasherDefault<` to the matching `>`.
        let inner_start = start + needle.len();
        let mut depth = 1usize;
        let mut idx = inner_start;
        let bytes = s.as_bytes();
        while idx < bytes.len() && depth > 0 {
            // Skip `->` (the return-type arrow in `fn() -> T`).
            if idx + 1 < bytes.len() && bytes[idx] == b'-' && bytes[idx + 1] == b'>' {
                idx += 2;
                continue;
            }
            match bytes[idx] {
                b'<' => depth += 1,
                b'>' => depth -= 1,
                _ => {}
            }
            idx += 1;
        }
        // `idx` is now one past the matching `>`. We require a literal `>`
        // immediately after (the outer generic's close bracket); if not
        // present, abort the trim and emit verbatim.
        if depth != 0 || idx >= bytes.len() || bytes[idx] != b'>' {
            // Not the pattern we expected — emit `, BuildHasherDefault<...`
            // verbatim and continue past it.
            out.push_str(&s[start..idx]);
            cursor = idx;
            continue;
        }
        // Replace `, BuildHasherDefault<...>` with empty string; the
        // outer `>` at idx is preserved.
        cursor = idx;
    }
    out.push_str(&s[cursor..]);
    out
}

/// Find every `LazyLock<X, fn() -> X>` and replace with `LazyLock<X>` when
/// the two `X`s are textually identical. Walks angle-bracket depth to
/// locate the comma at depth 1.
fn strip_lazy_lock_init_fn(s: &str) -> String {
    let needle = "LazyLock<";
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    while let Some(rel) = s[cursor..].find(needle) {
        let abs = cursor + rel;
        out.push_str(&s[cursor..abs]);
        out.push_str(needle);
        let inner_start = abs + needle.len();
        // Walk to the matching `>` at depth 0, tracking the comma position
        // at depth 1 (the top-level comma separating the two type args).
        let bytes = s.as_bytes();
        let mut depth = 1usize;
        let mut idx = inner_start;
        let mut top_comma: Option<usize> = None;
        while idx < bytes.len() && depth > 0 {
            // Skip `->` (the return-type arrow in `fn() -> T`); the `>`
            // there is part of an arrow, not a generic close.
            if idx + 1 < bytes.len() && bytes[idx] == b'-' && bytes[idx + 1] == b'>' {
                idx += 2;
                continue;
            }
            match bytes[idx] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                b',' if depth == 1 && top_comma.is_none() => {
                    top_comma = Some(idx);
                }
                _ => {}
            }
            idx += 1;
        }
        if depth != 0 || idx >= bytes.len() {
            // Malformed — emit the rest verbatim.
            out.push_str(&s[inner_start..]);
            return out;
        }
        let close = idx;
        match top_comma {
            Some(comma) => {
                let lhs = s[inner_start..comma].trim();
                let rhs = s[comma + 1..close].trim();
                // Only strip if rhs looks like `fn() -> <lhs>`.
                let expected = format!("fn() -> {lhs}");
                if rhs == expected {
                    out.push_str(lhs);
                    out.push('>');
                } else {
                    // Leave the LazyLock<...> contents alone.
                    out.push_str(&s[inner_start..=close]);
                }
            }
            None => {
                // Single-arg LazyLock — already canonical.
                out.push_str(&s[inner_start..=close]);
            }
        }
        cursor = close + 1;
    }
    out.push_str(&s[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_strips_global_allocator() {
        assert_eq!(trim_hir_display("Vec<u32, Global>"), "Vec<u32>");
        assert_eq!(
            trim_hir_display("Vec<Vec<u32, Global>, Global>"),
            "Vec<Vec<u32>>"
        );
    }

    #[test]
    fn trim_strips_hashmap_default_state() {
        assert_eq!(
            trim_hir_display("HashMap<String, u32, RandomState, Global>"),
            "HashMap<String, u32>"
        );
    }

    #[test]
    fn trim_strips_hashmap_with_fxhasher() {
        assert_eq!(
            trim_hir_display(
                "HashMap<String, u32, BuildHasherDefault<FxHasher>, Global>"
            ),
            "HashMap<String, u32>"
        );
    }

    #[test]
    fn trim_strips_lazy_lock_init_fn() {
        assert_eq!(
            trim_hir_display("LazyLock<Mutex<Foo>, fn() -> Mutex<Foo>>"),
            "LazyLock<Mutex<Foo>>"
        );
    }

    #[test]
    fn trim_keeps_lazy_lock_when_init_fn_differs() {
        // If the init fn returns something that doesn't textually match the
        // value type, leave it alone (defensive).
        assert_eq!(
            trim_hir_display("LazyLock<Mutex<Foo>, fn() -> Mutex<Bar>>"),
            "LazyLock<Mutex<Foo>, fn() -> Mutex<Bar>>"
        );
    }

    #[test]
    fn trim_leaves_unrelated_types_alone() {
        assert_eq!(trim_hir_display("&Path"), "&Path");
        assert_eq!(trim_hir_display("Option<u32>"), "Option<u32>");
        assert_eq!(trim_hir_display("Result<T, E>"), "Result<T, E>");
    }

    #[test]
    fn trim_is_idempotent() {
        let once = trim_hir_display("Vec<HashMap<K, V, RandomState, Global>, Global>");
        let twice = trim_hir_display(&once);
        assert_eq!(once, twice);
        assert_eq!(once, "Vec<HashMap<K, V>>");
    }
}
