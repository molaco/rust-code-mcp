//! Memory usage monitoring
//!
//! Tracks system memory usage for indexing operations

use sysinfo::System;

/// Monitor for tracking memory usage
pub struct MemoryMonitor {
    system: System,
}

impl MemoryMonitor {
    /// Create a new memory monitor
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_memory();
        Self { system }
    }

    /// Refresh memory statistics
    pub fn refresh(&mut self) {
        self.system.refresh_memory();
    }

    /// Get used memory in bytes
    pub fn used_bytes(&self) -> u64 {
        self.system.used_memory()
    }

    /// Get total memory in bytes
    pub fn total_bytes(&self) -> u64 {
        self.system.total_memory()
    }

    /// Get available memory in bytes
    pub fn available_bytes(&self) -> u64 {
        self.system.available_memory()
    }

    /// Get memory usage as percentage
    pub fn usage_percent(&self) -> f64 {
        100.0 * (self.used_bytes() as f64 / self.total_bytes() as f64)
    }
}

impl Default for MemoryMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_monitor_creation() {
        let monitor = MemoryMonitor::new();
        assert!(monitor.total_bytes() > 0);
    }

    #[test]
    fn test_memory_usage() {
        let mut monitor = MemoryMonitor::new();
        monitor.refresh();

        let used = monitor.used_bytes();
        let total = monitor.total_bytes();
        let available = monitor.available_bytes();

        assert!(used > 0);
        assert!(total > 0);
        assert!(available > 0);
        assert!(used <= total);
    }

    #[test]
    fn test_usage_percent() {
        let monitor = MemoryMonitor::new();
        let percent = monitor.usage_percent();

        assert!(percent >= 0.0);
        assert!(percent <= 100.0);
    }
}
