//! System memory awareness.
//!
//! Used to keep a configurable headroom of physical RAM free while doing
//! memory-heavy work — currently batch export, where a single 24 MP frame's
//! working set runs to hundreds of megabytes.
//!
//! ## Availability, not free
//!
//! On Linux this reports `MemAvailable`, not `MemFree`. The difference is not
//! academic: `MemFree` excludes page cache that the kernel would happily
//! reclaim on demand, and a RAW decoder fills page cache constantly. On the
//! development machine the two read 4.9 GB and 3.4 GB at the same instant —
//! gating on `MemFree` would pause an export that had plenty of room.
//!
//! ## Unsupported platforms return `None`
//!
//! `None` means "could not determine", NOT "no memory available". Callers must
//! treat it as permission to proceed, or the feature would silently block all
//! work on any platform without an implementation.
//!
//! macOS needs `host_statistics64` over Mach FFI (and there is real
//! disagreement about which page counters constitute "available"); Windows
//! needs `GlobalMemoryStatusEx`. Both are a single function to fill in here,
//! or a reason to take a `sysinfo` dependency — which would be this project's
//! first platform crate, so it is a deliberate decision rather than a detail.

/// Physical memory available for allocation, in bytes.
///
/// `None` when the platform has no implementation — see the module docs; this
/// must not be read as zero.
pub fn available_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_mem_available(&meminfo)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Pull `MemAvailable` (kB) out of `/proc/meminfo` and convert to bytes.
///
/// Split out from the file read so it can be tested against fixtures.
/// `MemAvailable` has been present since Linux 3.14 (2014); on a kernel old
/// enough to lack it this returns `None` and the caller proceeds ungated,
/// which is the right failure direction.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_mem_available(meminfo: &str) -> Option<u64> {
    for line in meminfo.lines() {
        let rest = match line.strip_prefix("MemAvailable:") {
            Some(r) => r,
            None => continue,
        };
        // Format is `MemAvailable:   12345678 kB` — take the number, ignore
        // the unit, which the kernel always reports as kB.
        let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return kb.checked_mul(1024);
    }
    None
}

/// Human-readable MB, for status text and logs.
pub fn as_mb(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:       16307760 kB
MemFree:         3457316 kB
MemAvailable:    4972468 kB
Buffers:          123456 kB
Cached:          2345678 kB
";

    #[test]
    fn parses_mem_available_and_converts_to_bytes() {
        let bytes = parse_mem_available(SAMPLE).expect("should parse");
        assert_eq!(bytes, 4_972_468 * 1024);
        assert_eq!(as_mb(bytes), 4855);
    }

    /// Must not fall back to `MemFree`, which is the whole point of the module.
    #[test]
    fn does_not_confuse_mem_free_with_mem_available() {
        let bytes = parse_mem_available(SAMPLE).unwrap();
        assert_ne!(bytes, 3_457_316 * 1024);
    }

    /// A kernel without `MemAvailable` must yield `None` (proceed ungated), not
    /// zero (pause forever).
    #[test]
    fn missing_field_is_none_not_zero() {
        let without = "MemTotal: 16307760 kB\nMemFree: 3457316 kB\n";
        assert_eq!(parse_mem_available(without), None);
    }

    #[test]
    fn malformed_value_is_none() {
        assert_eq!(parse_mem_available("MemAvailable:   banana kB\n"), None);
        assert_eq!(parse_mem_available("MemAvailable:\n"), None);
        assert_eq!(parse_mem_available(""), None);
    }

    /// `MemAvailableFoo` must not match the `MemAvailable:` prefix test.
    #[test]
    fn does_not_match_a_similarly_named_field() {
        assert_eq!(parse_mem_available("MemAvailableSomething: 5 kB\n"), None);
    }

    /// On Linux the real file must parse and give a sane number — this is the
    /// check that catches a kernel changing the format out from under us.
    #[test]
    #[cfg(target_os = "linux")]
    fn reads_the_real_system_and_returns_something_plausible() {
        let bytes = available_bytes().expect("Linux should report MemAvailable");
        assert!(bytes > 16 * 1024 * 1024, "implausibly low: {bytes} bytes");
        assert!(
            bytes < 64 * 1024 * 1024 * 1024 * 1024,
            "implausibly high: {bytes} bytes"
        );
    }
}
