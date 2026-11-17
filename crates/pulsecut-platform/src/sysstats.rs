//! CPU / memory / disk snapshots for the dashboard and resource guards.

use serde::{Deserialize, Serialize};
use std::path::Path;
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub cpu_percent: f32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub mem_available_mb: u64,
    pub process_mem_mb: u64,
    pub disk_free_gb: f32,
}

pub struct Monitor {
    sys: Option<System>,
    disks: Option<Disks>,
}

impl Default for Monitor {
    fn default() -> Self {
        Monitor::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        Monitor {
            sys: System::new().ok(),
            disks: Disks::new_with_refreshed_list().ok(),
        }
    }

    /// Refresh and snapshot. `data_dir` selects which disk's free space is reported.
    pub fn snapshot(&mut self, data_dir: &Path) -> Snapshot {
        let mut snap = Snapshot::default();
        if let Some(sys) = self.sys.as_mut() {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let pid = sysinfo::get_current_pid().ok();
            if let Some(pid) = pid {
                sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
            }
            snap.cpu_percent = sys.global_cpu_usage();
            snap.mem_used_mb = sys.used_memory() / 1_048_576;
            snap.mem_total_mb = sys.total_memory() / 1_048_576;
            snap.mem_available_mb = sys.available_memory() / 1_048_576;
            snap.process_mem_mb = pid
                .and_then(|p| sys.process(p))
                .map(|p| p.memory() / 1_048_576)
                .unwrap_or(0);
        }
        if let Some(d) = self.disks.as_mut() {
            d.refresh(true);
            snap.disk_free_gb = disk_free_gb(d, data_dir);
        }
        snap
    }
}

fn disk_free_gb(disks: &Disks, dir: &Path) -> f32 {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    disks
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space() as f32 / 1e9)
        .unwrap_or(f32::NAN)
}

/// One-shot free space check (GB) for the disk containing `dir`.
pub fn free_gb(dir: &Path) -> f32 {
    Disks::new_with_refreshed_list()
        .map(|d| disk_free_gb(&d, dir))
        .unwrap_or(f32::NAN)
}

/// One-shot available memory (MB).
pub fn available_mem_mb() -> u64 {
    System::new()
        .map(|mut s| {
            s.refresh_memory();
            s.available_memory() / 1_048_576
        })
        .unwrap_or(u64::MAX)
}
