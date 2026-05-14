# monitoring — Abstract Logic

## Module: monitoring (mod.rs)
**Purpose:** Root module that exposes the `health` and `backup` submodules.

1. **Declare submodules** -> `health`, `backup`

## Module: backup
**Purpose:** Manages versioned snapshots of the Merkle index on disk with retention-based rotation.

1. **Initialize backup directory and retention policy** -> `BackupManager::new()`
2. **Create timestamped Merkle snapshot and enforce retention** -> `BackupManager::create_backup()`, `BackupManager::rotate_backups()`
3. **Restore the most recent snapshot** -> `BackupManager::restore_latest()`
4. **Enumerate snapshot files in the backup directory** -> `BackupManager::list_backups()`
5. **Expose backup configuration accessors** -> `BackupManager::backup_dir()`, `BackupManager::retention_count()`

## Module: health
**Purpose:** Probes BM25, vector store, and Merkle subsystems concurrently and aggregates a unified health report.

1. **Build per-component health results** -> `ComponentHealth::healthy()`, `ComponentHealth::degraded()`, `ComponentHealth::unhealthy()`
2. **Construct the monitor with optional subsystem handles** -> `HealthMonitor::new()`
3. **Run all component checks concurrently and aggregate status** -> `HealthMonitor::check_health()`
4. **Probe individual subsystems with latency measurement** -> `HealthMonitor::check_bm25()`, `HealthMonitor::check_vector()`, `HealthMonitor::check_merkle()`
5. **Derive overall status from per-component results** -> `HealthMonitor::calculate_overall_status()`
