/// Resident memory of this process, in bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MemoryUsage {
    pub rss: u64,
    pub peak_rss: u64,
}

/// Current and peak resident set size.
///
/// Implemented on Linux (via `/proc`). Returns `None` elsewhere for now; Windows and macOS
/// need small FFI calls (`GetProcessMemoryInfo`, `task_info`) that come with the platform
/// work in later phases.
pub fn memory_usage() -> Option<MemoryUsage> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let field = |name: &str| {
            status.lines().find_map(|l| {
                let rest = l.strip_prefix(name)?.trim();
                rest.strip_suffix("kB")?
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(|kb| kb * 1024)
            })
        };
        Some(MemoryUsage {
            rss: field("VmRSS:")?,
            peak_rss: field("VmHWM:")?,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        None
    }
}
