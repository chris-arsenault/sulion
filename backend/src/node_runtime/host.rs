//! Whole-machine resource sampling for the development node.
//!
//! The node reports this on every heartbeat, and it is the only source of the
//! memory/CPU figures the browser shows. The control plane deliberately does
//! not sample itself: under a split deployment it runs no PTY, no build, and
//! no language server, so its own numbers describe the wrong machine.

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::Mutex;

use crate::node_protocol::NodeHostStats;

/// Holds its own `System` so successive CPU samples can diff against the
/// previous one. The first sample after construction reports 0%.
pub struct HostProbe {
    sys: Mutex<System>,
}

impl HostProbe {
    pub fn new() -> Self {
        Self {
            sys: Mutex::new(System::new_with_specifics(
                RefreshKind::new()
                    .with_memory(MemoryRefreshKind::new().with_ram())
                    .with_cpu(CpuRefreshKind::new().with_cpu_usage()),
            )),
        }
    }

    pub async fn sample(&self) -> NodeHostStats {
        let mut sys = self.sys.lock().await;
        sys.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());
        sys.refresh_cpu_usage();
        // `total - available` rather than sysinfo's `used_memory`, so
        // reclaimable page cache does not read as pressure on a box that has
        // just finished a build.
        let host_used = sys.total_memory().saturating_sub(sys.available_memory());
        let (memory_used_bytes, memory_total_bytes) = match cgroup_memory() {
            // A capped container knows a smaller ceiling than the machine's;
            // reporting the machine's would hide the limit it will actually
            // hit. The dedicated node is uncapped and falls through.
            Some((current, max)) => (current, max),
            None => (host_used, sys.total_memory()),
        };
        NodeHostStats {
            memory_used_bytes,
            memory_total_bytes,
            cpu_percent: sys.global_cpu_usage(),
        }
    }
}

impl Default for HostProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// Cgroup v2 usage and ceiling, when this process runs under a real memory
/// limit. Returns None on hosts without cgroup v2 and when the ceiling is
/// `max`, which is the uncapped dedicated node.
fn cgroup_memory() -> Option<(u64, u64)> {
    let max = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let max = max.trim();
    if max == "max" {
        return None;
    }
    let max = max.parse::<u64>().ok()?;
    let current = std::fs::read_to_string("/sys/fs/cgroup/memory.current")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some((current, max))
}
