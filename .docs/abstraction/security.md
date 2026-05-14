# security — Abstract Logic

## Module: security/mod.rs
**Purpose:** Filters filesystem paths against glob patterns to exclude sensitive files (env, credentials, keys, build artifacts) from indexing.

1. **Build a default filter from a hardcoded list of sensitive globs, silently dropping any that fail to compile** -> `SensitiveFileFilter::default()`, `<SensitiveFileFilter as Default>::default()`
2. **Build a filter from caller-supplied patterns, propagating compile errors** -> `SensitiveFileFilter::with_patterns()`
3. **Decide whether a path is safe to index by testing it against every stored pattern** -> `SensitiveFileFilter::should_index()`
4. **Expose or extend the active pattern set** -> `SensitiveFileFilter::excluded_patterns()`, `SensitiveFileFilter::add_pattern()`

## Module: security/secrets.rs
**Purpose:** Scans text content with named regex patterns to detect embedded secrets (API keys, tokens, private keys, etc.) and report findings.

1. **Construct a scanner preloaded with named regexes for common secret formats** -> `SecretsScanner::new()`, `<SecretsScanner as Default>::default()`
2. **Scan content line-by-line and collect every regex match into structured results** -> `SecretsScanner::scan()`
3. **Provide a boolean exclusion check derived from whether any secret was found** -> `SecretsScanner::should_exclude()`
4. **Render a human-readable summary of detected secrets, or a "none found" message** -> `SecretsScanner::scan_summary()`
