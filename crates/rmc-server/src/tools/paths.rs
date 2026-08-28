//! One gate for every path parameter that arrives from a client.
//!
//! The daemon serves *every* working directory from a single process (its key
//! stopped including the cwd), so it no longer shares a cwd with the session
//! that asked. A relative path therefore resolves against whatever directory
//! happened to spawn the daemon — which answers about the wrong project
//! *silently*, with a plausible-looking result rather than an error. That is
//! the most expensive failure class here, so relative paths are refused at the
//! entry points instead of being resolved against a directory nobody chose.

use std::path::{Path, PathBuf};

use rmcp::ErrorData as McpError;

/// Accept an absolute path parameter, refuse anything else.
///
/// `param` names the field in the refusal so the caller can see *which* of a
/// tool's several path arguments was wrong. Takes `AsRef<Path>` because the
/// entry points hold the value as a `&str` in some tools and as a `&Path` in
/// others, and one gate is worth more than a tidy signature.
pub(crate) fn require_absolute(param: &str, raw: impl AsRef<Path>) -> Result<PathBuf, McpError> {
    let path = raw.as_ref();
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let shown = path.display();
    // `~` is worth naming: shells expand it, JSON does not, so a tilde path
    // arrives here looking absolute to a human and relative to the filesystem.
    let hint = if path.to_string_lossy().starts_with('~') {
        " (`~` is not expanded — pass the expanded home directory)"
    } else {
        ""
    };
    Err(McpError::invalid_params(
        format!(
            "`{param}` must be an absolute path, got `{shown}`{hint}. \
             One daemon serves every working directory, so it cannot resolve \
             a relative path on your behalf."
        ),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_path_passes_through_unchanged() {
        let got = require_absolute("directory", "/home/sc/t/bur/rust_app").unwrap();
        assert_eq!(got, PathBuf::from("/home/sc/t/bur/rust_app"));
    }

    #[test]
    fn a_relative_path_is_refused() {
        for raw in [".", "..", "crates/rmc-server", "./x", ""] {
            let err = require_absolute("directory", raw)
                .expect_err("a relative path must not be accepted");
            assert!(
                err.message.contains("must be an absolute path"),
                "unhelpful refusal for {raw:?}: {}",
                err.message
            );
        }
    }

    /// The refusal has to name the offending parameter: several tools take more
    /// than one path, and "some path was relative" would send the caller hunting.
    #[test]
    fn the_refusal_names_the_parameter_and_the_value() {
        let err = require_absolute("file_path", "src/main.rs").unwrap_err();
        assert!(err.message.contains("file_path"), "{}", err.message);
        assert!(err.message.contains("src/main.rs"), "{}", err.message);
    }

    #[test]
    fn a_tilde_path_is_refused_with_the_reason_it_looks_absolute() {
        let err = require_absolute("directory", "~/t/bur/rust_app").unwrap_err();
        assert!(err.message.contains("not expanded"), "{}", err.message);
    }
}
