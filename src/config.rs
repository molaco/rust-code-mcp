//! Configuration management with environment variable support
//!
//! Provides a centralized configuration system for the MCP server

pub mod errors;
pub mod indexer;

pub use errors::{Error, ErrorContextExt, Result};
pub use indexer::{IndexerConfig, IndexerCoreConfig, QdrantConfig, TantivyConfig};

use std::env;
use std::path::PathBuf;

/// Server configuration
#[derive(Debug, Clone)]
pub struct Config {
    /// Qdrant server URL
    pub qdrant_url: String,
    /// Port for the MCP server
    pub server_port: u16,
    /// Directory for storing indexes and cache
    pub data_dir: PathBuf,
    /// Maximum file size to index (in bytes)
    pub max_file_size: u64,
    /// Number of CPU cores to use for parallel processing (0 = auto)
    pub num_threads: usize,
    /// Enable debug logging
    pub debug: bool,
    /// Retry attempts for transient failures
    pub retry_attempts: u32,
    /// Initial retry delay in milliseconds
    pub retry_delay_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            qdrant_url: "http://localhost:6334".to_string(),
            server_port: 3000,
            data_dir: default_data_dir(),
            max_file_size: 10_000_000, // 10 MB
            num_threads: 0, // Auto-detect
            debug: false,
            retry_attempts: 3,
            retry_delay_ms: 100,
        }
    }
}

impl Config {
    /// Load configuration from environment variables
    ///
    /// Supported environment variables:
    /// - QDRANT_URL: Qdrant server URL (default: http://localhost:6334)
    /// - SERVER_PORT: MCP server port (default: 3000)
    /// - DATA_DIR: Data directory path (default: ~/.local/share/rust-code-mcp)
    /// - MAX_FILE_SIZE: Maximum file size in MB (default: 10)
    /// - NUM_THREADS: Number of threads (default: 0 for auto)
    /// - DEBUG: Enable debug logging (default: false)
    /// - RETRY_ATTEMPTS: Retry attempts (default: 3)
    /// - RETRY_DELAY_MS: Initial retry delay in ms (default: 100)
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(url) = env::var("QDRANT_URL") {
            config.qdrant_url = url;
        }

        if let Ok(port) = env::var("SERVER_PORT") {
            if let Ok(port_num) = port.parse::<u16>() {
                config.server_port = port_num;
            }
        }

        if let Ok(dir) = env::var("DATA_DIR") {
            config.data_dir = PathBuf::from(dir);
        }

        if let Ok(size_mb) = env::var("MAX_FILE_SIZE") {
            if let Ok(size) = size_mb.parse::<u64>() {
                config.max_file_size = size * 1_000_000;
            }
        }

        if let Ok(threads) = env::var("NUM_THREADS") {
            if let Ok(num) = threads.parse::<usize>() {
                config.num_threads = num;
            }
        }

        if let Ok(debug) = env::var("DEBUG") {
            config.debug = debug.eq_ignore_ascii_case("true") || debug == "1";
        }

        if let Ok(attempts) = env::var("RETRY_ATTEMPTS") {
            if let Ok(num) = attempts.parse::<u32>() {
                config.retry_attempts = num;
            }
        }

        if let Ok(delay) = env::var("RETRY_DELAY_MS") {
            if let Ok(ms) = delay.parse::<u64>() {
                config.retry_delay_ms = ms;
            }
        }

        config
    }

    /// Get the Tantivy index directory
    pub fn tantivy_dir(&self) -> PathBuf {
        self.data_dir.join("tantivy")
    }

    /// Get the metadata cache directory
    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    /// Print configuration summary
    pub fn print_summary(&self) {
        println!("\n=== Configuration ===");
        println!("Qdrant URL:      {}", self.qdrant_url);
        println!("Server Port:     {}", self.server_port);
        println!("Data Directory:  {}", self.data_dir.display());
        println!("Max File Size:   {} MB", self.max_file_size / 1_000_000);
        println!("Threads:         {}", if self.num_threads == 0 { "auto".to_string() } else { self.num_threads.to_string() });
        println!("Debug:           {}", self.debug);
        println!("Retry Attempts:  {}", self.retry_attempts);
        println!("Retry Delay:     {}ms", self.retry_delay_ms);
        println!("====================\n");
    }
}

/// Get the default data directory
fn default_data_dir() -> PathBuf {
    if let Some(data_dir) = directories::ProjectDirs::from("com", "rust-code-mcp", "rust-code-mcp") {
        data_dir.data_dir().to_path_buf()
    } else {
        // Fallback to current directory
        PathBuf::from("./data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.qdrant_url, "http://localhost:6334");
        assert_eq!(config.server_port, 3000);
        assert_eq!(config.max_file_size, 10_000_000);
        assert_eq!(config.retry_attempts, 3);
    }

    #[test]
    fn test_tantivy_dir() {
        let config = Config::default();
        let tantivy_dir = config.tantivy_dir();
        assert!(tantivy_dir.to_string_lossy().contains("tantivy"));
    }

    #[test]
    fn test_cache_dir() {
        let config = Config::default();
        let cache_dir = config.cache_dir();
        assert!(cache_dir.to_string_lossy().contains("cache"));
    }

    #[test]
    fn test_from_env() {
        // Set environment variables
        unsafe {
            env::set_var("QDRANT_URL", "http://test:6334");
            env::set_var("SERVER_PORT", "8080");
            env::set_var("MAX_FILE_SIZE", "20");
            env::set_var("DEBUG", "true");
            env::set_var("RETRY_ATTEMPTS", "5");
        }

        let config = Config::from_env();
        assert_eq!(config.qdrant_url, "http://test:6334");
        assert_eq!(config.server_port, 8080);
        assert_eq!(config.max_file_size, 20_000_000);
        assert!(config.debug);
        assert_eq!(config.retry_attempts, 5);

        // Clean up
        unsafe {
            env::remove_var("QDRANT_URL");
            env::remove_var("SERVER_PORT");
            env::remove_var("MAX_FILE_SIZE");
            env::remove_var("DEBUG");
            env::remove_var("RETRY_ATTEMPTS");
        }
    }
}
