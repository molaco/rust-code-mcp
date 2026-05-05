# security — Detailed Logic

## Module: security/mod.rs

### `SensitiveFileFilter::default() -> Self`
**Call graph:** Pattern::new -> Iterator::filter_map -> Iterator::collect
**Steps:**
1. Define a hardcoded list of glob pattern strings covering env files, credentials, SSH/AWS dirs, key files, build artifacts, git, and common secret name fragments.
2. Iterate the strings, compile each via `glob::Pattern::new`, and silently drop any that fail to compile using `filter_map`.
3. Collect the compiled patterns into a `Vec<Pattern>` and return a `SensitiveFileFilter` containing them.

### `SensitiveFileFilter::with_patterns(patterns: Vec<String>) -> Result<Self, glob::PatternError>`
**Call graph:** Pattern::new -> Iterator::map -> Iterator::collect
**Steps:**
1. Map each input string through `Pattern::new`, collecting into a `Result<Vec<Pattern>, _>` so the first compile error short-circuits.
2. Propagate any `PatternError` via `?` on the collected result.
3. Wrap the compiled vector in a new `SensitiveFileFilter` and return it as `Ok`.

### `SensitiveFileFilter::should_index(&self, path: &Path) -> bool`
**Call graph:** Path::to_string_lossy -> Pattern::matches -> tracing::debug!
**Steps:**
1. Convert the path to a lossy string for glob matching.
2. Iterate the stored patterns and test each against the path string with `Pattern::matches`.
3. On the first match, log a debug message identifying the excluded path and return `false`.
4. If no pattern matches, return `true` to permit indexing.

### `SensitiveFileFilter::excluded_patterns(&self) -> Vec<String>`
**Call graph:** Pattern::as_str -> str::to_string -> Iterator::map -> Iterator::collect
**Steps:**
1. Iterate the stored compiled patterns.
2. For each, call `as_str()` to recover the original glob and convert it to an owned `String`.
3. Collect the strings into a `Vec<String>` and return.

### `SensitiveFileFilter::add_pattern(&mut self, pattern: &str) -> Result<(), glob::PatternError>`
**Call graph:** Pattern::new -> Vec::push
**Steps:**
1. Compile the supplied pattern string via `Pattern::new`, propagating errors with `?`.
2. Push the compiled `Pattern` into the filter's `excluded_patterns` vector.
3. Return `Ok(())` to signal successful addition.

### `impl Default for SensitiveFileFilter :: default() -> Self`
**Call graph:** SensitiveFileFilter::default
**Steps:**
1. Delegate to the inherent `SensitiveFileFilter::default()` constructor (note: this shadows the trait method and can recurse if the inherent method is ever removed).
2. Return the resulting instance with the standard exclusion patterns.

## Module: security/secrets.rs

### `SecretsScanner::new() -> Self`
**Call graph:** Regex::new -> Result::unwrap
**Steps:**
1. Build a `Vec<(String, Regex)>` of named pattern definitions covering AWS access keys, PEM private keys, generic API keys, generic secrets, GitHub tokens, Slack tokens, Google API keys, Stripe live keys, and JWT tokens.
2. Compile each regex literal at construction time, unwrapping any failure (these literals are static so unwrap is safe).
3. Return a `SecretsScanner` containing the compiled patterns.

### `SecretsScanner::scan(&self, content: &str) -> Vec<SecretMatch>`
**Call graph:** str::lines -> Iterator::enumerate -> Regex::is_match -> Vec::push -> String::clone
**Steps:**
1. Initialize an empty `Vec<SecretMatch>` accumulator.
2. For each `(name, pattern)` pair, iterate every line of `content` with its zero-based index.
3. If `pattern.is_match(line)` succeeds, push a `SecretMatch` recording the cloned pattern name and the 1-based line number.
4. After iterating all patterns and lines, return the accumulated matches.

### `SecretsScanner::should_exclude(&self, content: &str) -> bool`
**Call graph:** SecretsScanner::scan -> Vec::is_empty
**Steps:**
1. Run `scan` to collect any matches in the content.
2. Return `true` if the resulting vector is non-empty (i.e., negate `is_empty`).

### `SecretsScanner::scan_summary(&self, content: &str) -> String`
**Call graph:** SecretsScanner::scan -> Vec::is_empty -> str::to_string -> format! -> String::push_str
**Steps:**
1. Call `scan` to obtain the list of matches.
2. If empty, return the literal `"No secrets detected"`.
3. Otherwise, format a header containing the total count of potential secrets.
4. Iterate each match: append a bullet line including the pattern name and (if present) the line number, otherwise just the name.
5. Return the assembled summary string.

### `impl Default for SecretsScanner :: default() -> Self`
**Call graph:** SecretsScanner::new
**Steps:**
1. Delegate to `SecretsScanner::new()` to build the scanner with the default pattern set.
2. Return the constructed scanner.
