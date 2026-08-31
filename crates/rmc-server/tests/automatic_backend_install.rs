//! Its own test binary: `install_automatic_backend` writes a process-wide
//! `OnceLock`, so a non-default profile in the lib test binary would change the
//! default under every other test there.

use rmc_server::mcp::{
    automatic_embedding_profile_name, install_automatic_backend, validate_automatic_profile,
    DEFAULT_AUTOMATIC_EMBEDDING_PROFILE,
};

/// One test, because separate tests in one binary run in parallel threads and
/// the two installs need a known order.
#[test]
fn install_decides_the_default_and_the_second_install_is_refused() {
    // Not the fallback profile, so this fails if the install does nothing.
    let installed = validate_automatic_profile("openrouter-qwen3-8b").expect("built-in profile");
    install_automatic_backend(installed).expect("nothing has read the default yet");
    assert_eq!(automatic_embedding_profile_name(), "openrouter-qwen3-8b");

    let second = validate_automatic_profile(DEFAULT_AUTOMATIC_EMBEDDING_PROFILE)
        .expect("built-in profile");
    let returned = install_automatic_backend(second).expect_err("the default is set for the run");

    assert_eq!(returned.profile.name(), DEFAULT_AUTOMATIC_EMBEDDING_PROFILE);
    assert_eq!(automatic_embedding_profile_name(), "openrouter-qwen3-8b");
}
