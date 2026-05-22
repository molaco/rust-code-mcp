//! Security module for preventing sensitive data from being indexed

pub mod secrets;

use glob::Pattern;
use std::path::Path;

/// Filter for sensitive files that should not be indexed
pub struct SensitiveFileFilter {
    excluded_patterns: Vec<Pattern>,
}

impl SensitiveFileFilter {
    /// Create a new filter with default exclusion patterns
    pub fn default() -> Self {
        let patterns = vec![
            // Environment files
            ".env",
            ".env.*",
            "**/.env.*",
            "*.env",
            // Credentials and secrets
            "**/secrets/**",
            "**/credentials/**",
            "**/.aws/**",
            "**/.ssh/**",
            "**/private_key*",
            "**/*.key",
            "**/*.pem",
            "**/*.p12",
            "**/*.pfx",
            // Configuration with potential secrets
            "**/config/database.yml",
            "**/config/secrets.yml",
            // Git and version control
            "**/.git/**",
            "**/.gitignore",
            // Build artifacts and dependencies
            "**/target/**",
            "**/node_modules/**",
            "**/.cargo/**",
            // Test fixtures that might contain fake secrets
            "**/fixtures/secrets/**",
            // Common secret files
            "**/*secret*",
            "**/*password*",
            "**/*credential*",
        ];

        Self {
            excluded_patterns: patterns
                .iter()
                .filter_map(|p| Pattern::new(p).ok())
                .collect(),
        }
    }

    /// Create a filter with custom exclusion patterns
    pub fn with_patterns(patterns: Vec<String>) -> Result<Self, glob::PatternError> {
        let compiled_patterns: Result<Vec<Pattern>, _> =
            patterns.iter().map(|p| Pattern::new(p)).collect();

        Ok(Self {
            excluded_patterns: compiled_patterns?,
        })
    }

    /// Check if a file should be indexed
    ///
    /// Returns true if the file should be indexed, false if it should be excluded
    pub fn should_index(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.excluded_patterns {
            if pattern.matches(&path_str) {
                tracing::debug!("Excluding sensitive file: {}", path_str);
                return false;
            }
        }

        true
    }

    /// Get the list of excluded patterns as strings
    pub fn excluded_patterns(&self) -> Vec<String> {
        self.excluded_patterns
            .iter()
            .map(|p| p.as_str().to_string())
            .collect()
    }

    /// Add a pattern to the exclusion list
    pub fn add_pattern(&mut self, pattern: &str) -> Result<(), glob::PatternError> {
        let compiled = Pattern::new(pattern)?;
        self.excluded_patterns.push(compiled);
        Ok(())
    }
}

impl Default for SensitiveFileFilter {
    fn default() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_env_file_exclusion() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new(".env")));
        assert!(!filter.should_index(Path::new(".env.local")));
        assert!(!filter.should_index(Path::new("config/.env.production")));
    }

    #[test]
    fn test_credentials_exclusion() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new("config/credentials/api_key.txt")));
        assert!(!filter.should_index(Path::new("secrets/database.yml")));
        assert!(!filter.should_index(Path::new(".ssh/id_rsa")));
        assert!(!filter.should_index(Path::new("private_key.pem")));
    }

    #[test]
    fn test_key_files_exclusion() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new("server.key")));
        assert!(!filter.should_index(Path::new("cert.pem")));
        assert!(!filter.should_index(Path::new("keystore.p12")));
    }

    #[test]
    fn test_build_artifacts_exclusion() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new("target/debug/app")));
        assert!(!filter.should_index(Path::new("node_modules/package/index.js")));
        assert!(!filter.should_index(Path::new(".cargo/registry/cache")));
    }

    #[test]
    fn test_git_exclusion() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new(".git/config")));
        assert!(!filter.should_index(Path::new(".git/hooks/pre-commit")));
    }

    #[test]
    fn test_safe_files_allowed() {
        let filter = SensitiveFileFilter::default();

        assert!(filter.should_index(Path::new("src/main.rs")));
        assert!(filter.should_index(Path::new("README.md")));
        assert!(filter.should_index(Path::new("Cargo.toml")));
        assert!(filter.should_index(Path::new("lib/parser.rs")));
    }

    #[test]
    fn test_custom_patterns() {
        let filter = SensitiveFileFilter::with_patterns(vec![
            "**/*.secret".to_string(),
            "**/temp/**".to_string(),
        ])
        .unwrap();

        assert!(!filter.should_index(Path::new("data/api.secret")));
        assert!(!filter.should_index(Path::new("temp/test.txt")));
        assert!(filter.should_index(Path::new("src/main.rs")));
    }

    #[test]
    fn test_add_pattern() {
        let mut filter = SensitiveFileFilter::default();

        // Initially, custom files are indexed
        assert!(filter.should_index(Path::new("test.custom")));

        // Add new exclusion pattern
        filter.add_pattern("**/*.custom").unwrap();

        // Now they should be excluded
        assert!(!filter.should_index(Path::new("test.custom")));
        assert!(!filter.should_index(Path::new("dir/file.custom")));
    }

    #[test]
    fn test_secret_patterns() {
        let filter = SensitiveFileFilter::default();

        assert!(!filter.should_index(Path::new("config/secret_key.txt")));
        assert!(!filter.should_index(Path::new("passwords.txt")));
        assert!(!filter.should_index(Path::new("user_credentials.json")));
    }

    #[test]
    fn test_excluded_patterns_retrieval() {
        let filter = SensitiveFileFilter::default();
        let patterns = filter.excluded_patterns();

        assert!(!patterns.is_empty());
        assert!(patterns.contains(&".env".to_string()));
    }
}
