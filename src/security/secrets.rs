//! Secrets scanner to prevent sensitive data from being indexed
//!
//! Detects common patterns for API keys, passwords, private keys, etc.
//! and prevents them from being indexed into the search system.

use regex::Regex;

/// A match for a potential secret
#[derive(Debug, Clone)]
pub struct SecretMatch {
    /// Name of the pattern that matched
    pub pattern_name: String,
    /// Line number where the match was found (if available)
    pub line_number: Option<usize>,
}

/// Scanner for detecting secrets in code
pub struct SecretsScanner {
    patterns: Vec<(String, Regex)>,
}

impl SecretsScanner {
    /// Create a new secrets scanner with default patterns
    pub fn new() -> Self {
        let patterns = vec![
            // AWS Access Keys
            (
                "AWS Access Key".to_string(),
                Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            ),
            // Private Keys
            (
                "Private Key".to_string(),
                Regex::new(r"-----BEGIN (RSA |EC |DSA |OPENSSH |)PRIVATE KEY-----").unwrap(),
            ),
            // Generic API Keys
            (
                "Generic API Key".to_string(),
                Regex::new(r#"(?i)(api[_-]?key|apikey|api[_-]?token)[\s:=]+['"]([^'"]{20,})['"]"#).unwrap(),
            ),
            // Generic Secrets
            (
                "Generic Secret".to_string(),
                Regex::new(r#"(?i)(secret|password|passwd|pwd)[\s:=]+['"]([^'"]{8,})['"]"#).unwrap(),
            ),
            // GitHub Token
            (
                "GitHub Token".to_string(),
                Regex::new(r"ghp_[0-9a-zA-Z]{36}").unwrap(),
            ),
            // Slack Token
            (
                "Slack Token".to_string(),
                Regex::new(r"xox[baprs]-[0-9a-zA-Z]{10,48}").unwrap(),
            ),
            // Google API Key
            (
                "Google API Key".to_string(),
                Regex::new(r"AIza[0-9A-Za-z\\-_]{35}").unwrap(),
            ),
            // Stripe API Key
            (
                "Stripe API Key".to_string(),
                Regex::new(r"sk_live_[0-9a-zA-Z]{24}").unwrap(),
            ),
            // JWT Token
            (
                "JWT Token".to_string(),
                Regex::new(r"eyJ[A-Za-z0-9-_=]+\.eyJ[A-Za-z0-9-_=]+\.?[A-Za-z0-9-_.+/=]*").unwrap(),
            ),
        ];

        Self { patterns }
    }

    /// Scan content for potential secrets
    ///
    /// Returns a list of matches found in the content
    pub fn scan(&self, content: &str) -> Vec<SecretMatch> {
        let mut matches = Vec::new();

        for (name, pattern) in &self.patterns {
            for (line_idx, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    matches.push(SecretMatch {
                        pattern_name: name.clone(),
                        line_number: Some(line_idx + 1),
                    });
                }
            }
        }

        matches
    }

    /// Check if content should be excluded from indexing
    ///
    /// Returns true if any secrets are detected
    pub fn should_exclude(&self, content: &str) -> bool {
        !self.scan(content).is_empty()
    }

    /// Get a summary of what secrets were found
    pub fn scan_summary(&self, content: &str) -> String {
        let matches = self.scan(content);

        if matches.is_empty() {
            return "No secrets detected".to_string();
        }

        let mut summary = format!("Found {} potential secret(s):\n", matches.len());

        for secret in &matches {
            if let Some(line) = secret.line_number {
                summary.push_str(&format!("  - {} at line {}\n", secret.pattern_name, line));
            } else {
                summary.push_str(&format!("  - {}\n", secret.pattern_name));
            }
        }

        summary
    }
}

impl Default for SecretsScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_key_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"const AWS_KEY = "AKIAIOSFODNN7EXAMPLE";"#;

        assert!(scanner.should_exclude(content));

        let matches = scanner.scan(content);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pattern_name, "AWS Access Key");
    }

    #[test]
    fn test_private_key_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"
-----BEGIN RSA PRIVATE KEY-----
MIIEpAIBAAKCAQEA...
-----END RSA PRIVATE KEY-----
        "#;

        assert!(scanner.should_exclude(content));

        let matches = scanner.scan(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].pattern_name, "Private Key");
    }

    #[test]
    fn test_api_key_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"
let api_key = "sk_test_1234567890abcdefghijklmn";
        "#;

        let matches = scanner.scan(content);
        assert!(!matches.is_empty());
    }

    #[test]
    fn test_github_token_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"
GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwxyz
        "#;

        assert!(scanner.should_exclude(content));

        let matches = scanner.scan(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].pattern_name, "GitHub Token");
    }

    #[test]
    fn test_safe_content() {
        let scanner = SecretsScanner::new();
        let content = r#"
fn main() {
    println!("Hello, world!");
}

const MAX_SIZE: usize = 1024;
let api_url = "https://api.example.com";
        "#;

        assert!(!scanner.should_exclude(content));
        assert!(scanner.scan(content).is_empty());
    }

    #[test]
    fn test_scan_summary() {
        let scanner = SecretsScanner::new();

        // Safe content
        let safe = "fn test() {}";
        assert!(scanner.scan_summary(safe).contains("No secrets detected"));

        // Content with secrets
        let unsafe_content = r#"const KEY = "AKIAIOSFODNN7EXAMPLE";"#;
        let summary = scanner.scan_summary(unsafe_content);
        assert!(summary.contains("Found 1 potential secret"));
        assert!(summary.contains("AWS Access Key"));
    }

    #[test]
    fn test_line_numbers() {
        let scanner = SecretsScanner::new();
        let content = r#"
fn main() {
    let key = "AKIAIOSFODNN7EXAMPLE";
    println!("test");
}
        "#;

        let matches = scanner.scan(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].line_number, Some(3));
    }

    #[test]
    fn test_jwt_detection() {
        let scanner = SecretsScanner::new();
        let content = r#"
let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        "#;

        assert!(scanner.should_exclude(content));

        let matches = scanner.scan(content);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].pattern_name, "JWT Token");
    }

    #[test]
    fn test_multiple_secrets() {
        let scanner = SecretsScanner::new();
        let content = r#"
const AWS_KEY = "AKIAIOSFODNN7EXAMPLE";
const GITHUB_TOKEN = "ghp_1234567890abcdefghijklmnopqrstuvwxyz";
        "#;

        let matches = scanner.scan(content);
        assert_eq!(matches.len(), 2);
    }
}
