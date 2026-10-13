//! Machine-local memory and disk sampling for dashboard snapshots.
//!
//! Sampling runs on a blocking thread behind a TTL cache so a slow disk
//! never stalls the async snapshot worker. `sysinfo::Disks` offers no list
//! refresh in 0.38, so the disk list is rebuilt on each refresh while the
//! `System` handle is reused for memory. Memory and disk are sampled
//! independently: a missing disk never hides memory data.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) const SYSTEM_RESOURCES_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub(crate) struct SnapshotSystemResources {
    pub(crate) memory_total_bytes: Option<u64>,
    pub(crate) memory_available_bytes: Option<u64>,
    pub(crate) disk_path: Option<String>,
    pub(crate) disk_total_bytes: Option<u64>,
    pub(crate) disk_available_bytes: Option<u64>,
}

pub(crate) struct SystemResourcesSampler {
    state: Mutex<SamplerState>,
}

struct SamplerState {
    system: sysinfo::System,
    captured_at: Option<Instant>,
    cached: Option<SnapshotSystemResources>,
}

impl SystemResourcesSampler {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(SamplerState {
                system: sysinfo::System::new(),
                captured_at: None,
                cached: None,
            }),
        }
    }

    /// Blocking: refreshes at most every TTL. Call from `spawn_blocking`.
    pub(crate) fn sample_blocking(&self) -> Option<SnapshotSystemResources> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(captured_at) = state.captured_at {
            if captured_at.elapsed() < SYSTEM_RESOURCES_CACHE_TTL {
                if let Some(cached) = state.cached.clone() {
                    return Some(cached);
                }
            }
        }
        state.system.refresh_memory();
        let (memory_total_bytes, memory_available_bytes) = memory_resources(&state.system);
        let disk = disk_resources();
        let snapshot = SnapshotSystemResources {
            memory_total_bytes,
            memory_available_bytes,
            disk_path: disk.as_ref().map(|disk| disk.0.clone()),
            disk_total_bytes: disk.as_ref().map(|disk| disk.1),
            disk_available_bytes: disk.as_ref().map(|disk| disk.2),
        };
        state.captured_at = Some(Instant::now());
        state.cached = Some(snapshot.clone());
        Some(snapshot)
    }
}

fn memory_resources(system: &sysinfo::System) -> (Option<u64>, Option<u64>) {
    let total = system.total_memory();
    if total == 0 {
        return (None, None);
    }
    (Some(total), Some(system.available_memory()))
}

fn disk_resources() -> Option<(String, u64, u64)> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let disk = pick_snapshot_disk(&disks)?;
    let total = disk.total_space();
    if total == 0 {
        return None;
    }
    Some((
        disk.mount_point().to_string_lossy().into_owned(),
        total,
        disk.available_space(),
    ))
}

fn pick_snapshot_disk(disks: &sysinfo::Disks) -> Option<&sysinfo::Disk> {
    if let Ok(current) = std::env::current_dir() {
        let mut best: Option<&sysinfo::Disk> = None;
        let mut best_len = 0;
        for disk in disks.list() {
            let mount = disk.mount_point();
            if mount_matches(mount, &current) && mount.as_os_str().len() > best_len {
                best_len = mount.as_os_str().len();
                best = Some(disk);
            }
        }
        if let Some(best) = best {
            return if best.total_space() == 0 {
                None
            } else {
                Some(best)
            };
        }
    }
    disks
        .list()
        .iter()
        .max_by_key(|disk| disk.total_space())
        .filter(|disk| disk.total_space() != 0)
}

fn mount_matches(mount: &Path, current: &Path) -> bool {
    if current == mount {
        return true;
    }
    match current.strip_prefix(mount) {
        Ok(rest) => !rest.as_os_str().is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_matching_respects_component_boundaries() {
        assert!(mount_matches(Path::new("/"), Path::new("/")));
        assert!(mount_matches(Path::new("/"), Path::new("/foobar")));
        assert!(mount_matches(Path::new("/foo"), Path::new("/foo")));
        assert!(mount_matches(Path::new("/foo"), Path::new("/foo/bar")));
        assert!(!mount_matches(Path::new("/foo"), Path::new("/foobar")));
        assert!(!mount_matches(Path::new("/foo"), Path::new("/")));
        assert!(!mount_matches(Path::new("/foo"), Path::new("/other")));
    }

    #[test]
    fn empty_disk_list_selects_nothing() {
        assert!(pick_snapshot_disk(&sysinfo::Disks::new()).is_none());
    }

    #[test]
    fn sampler_caches_consecutive_samples() {
        let sampler = SystemResourcesSampler::new();
        let first = sampler.sample_blocking();
        let second = sampler.sample_blocking();
        assert_eq!(
            first.as_ref().map(|snapshot| snapshot.disk_path.clone()),
            second.as_ref().map(|snapshot| snapshot.disk_path.clone())
        );
    }
}
