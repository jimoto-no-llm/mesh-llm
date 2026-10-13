//! Machine-local memory and disk sampling for dashboard snapshots.
//!
//! Sampling runs on a blocking thread behind a TTL cache so a slow disk
//! never stalls the async snapshot worker. `sysinfo::Disks` offers no list
//! refresh in 0.38, so the disk list is rebuilt on each refresh while the
//! `System` handle is reused for memory. Memory and disk are sampled
//! independently: a missing disk never hides memory data.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Locked only by the refresh thread.
    system: Mutex<sysinfo::System>,
    /// Locked briefly to read or publish a sample.
    latest: Mutex<Option<PublishedResources>>,
    refreshing: AtomicBool,
}

struct PublishedResources {
    captured_at: Instant,
    resources: SnapshotSystemResources,
}

impl SystemResourcesSampler {
    pub(crate) fn new() -> Self {
        Self {
            system: Mutex::new(sysinfo::System::new()),
            latest: Mutex::new(None),
            refreshing: AtomicBool::new(false),
        }
    }

    /// The latest published sample. Never waits for a refresh.
    pub(crate) fn latest(&self) -> Option<SnapshotSystemResources> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|published| published.resources.clone())
    }

    /// Claim the single refresh slot when there is no sample or it is older
    /// than the TTL. A refresh that never finishes keeps the slot, so stalled
    /// refreshes cannot pile up.
    pub(crate) fn try_begin_refresh(&self) -> bool {
        let stale = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_none_or(|published| published.captured_at.elapsed() >= SYSTEM_RESOURCES_CACHE_TTL);
        stale
            && self
                .refreshing
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    /// Blocking: sample memory and disk and publish the result. Call from
    /// `spawn_blocking` after `try_begin_refresh` returned true.
    pub(crate) fn refresh_blocking(&self) {
        let _slot = RefreshSlot(&self.refreshing);
        let (memory_total_bytes, memory_available_bytes) = {
            let mut system = self
                .system
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            system.refresh_memory();
            memory_resources(&system)
        };
        let disk = disk_resources();
        let resources = SnapshotSystemResources {
            memory_total_bytes,
            memory_available_bytes,
            disk_path: disk.as_ref().map(|disk| disk.0.clone()),
            disk_total_bytes: disk.as_ref().map(|disk| disk.1),
            disk_available_bytes: disk.as_ref().map(|disk| disk.2),
        };
        *self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PublishedResources {
            captured_at: Instant::now(),
            resources,
        });
    }
}

/// Releases the refresh slot even if sampling panics.
struct RefreshSlot<'a>(&'a AtomicBool);

impl Drop for RefreshSlot<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
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
    fn refresh_slot_is_single_and_released_after_refresh() {
        let sampler = SystemResourcesSampler::new();
        assert!(sampler.latest().is_none());
        assert!(sampler.try_begin_refresh());
        // A refresh in flight (or stalled) keeps the slot and readers return
        // immediately with the previous value.
        assert!(!sampler.try_begin_refresh());
        assert!(sampler.latest().is_none());

        sampler.refresh_blocking();
        assert!(sampler.latest().is_some());
        // A fresh sample needs no refresh until the TTL passes.
        assert!(!sampler.try_begin_refresh());
    }
}
