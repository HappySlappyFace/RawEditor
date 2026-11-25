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
//! ## Platform support
//!
//! | Platform | Source | Status |
//! |---|---|---|
//! | Linux | `/proc/meminfo` → `MemAvailable` | tested on hardware |
//! | Windows | `GlobalMemoryStatusEx` → `ullAvailPhys` | type-checked, **never linked or run** |
//! | macOS | — | returns `None`, check inert |
//!
//! `None` means "could not determine", NOT "no memory available". Callers must
//! treat it as permission to proceed, or the feature would silently block all
//! work on any platform without an implementation.
//!
//! macOS would need `host_statistics64` over Mach FFI, where there is genuine
//! disagreement about which page counters constitute "available" — that is a
//! judgement call best made by someone who can measure it on the machine.
//!
//! The Windows path was authored on a Linux-only toolchain. It has been
//! type-checked (by temporarily compiling the module for the host), so the
//! struct, the `extern` signature and the call all compile — but it has never
//! been linked against kernel32 or executed. Two things contain that risk:
//! `MEMORYSTATUSEX_LAYOUT_CHECK` turns a layout mistake into a build error
//! rather than a buffer overrun, and any runtime failure returns `None`, which
//! leaves the guard inert instead of wrong.

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
    #[cfg(target_os = "windows")]
    {
        windows_impl::available_bytes()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// `GlobalMemoryStatusEx` from kernel32.
///
/// Declared by hand rather than pulling in `windows-sys`. The tradeoff was
/// decided by what could actually be checked: `windows-sys`' module paths and
/// feature names move between versions and its source is not vendored here, so
/// depending on it would mean guessing at names that fail to compile on the
/// user's machine. A hand-written declaration has exactly one risk — struct
/// layout — and that risk is eliminated by the size assertion below.
#[cfg(target_os = "windows")]
#[allow(non_snake_case)]
mod windows_impl {
    /// Mirrors the Win32 `MEMORYSTATUSEX`. Field order and widths are fixed by
    /// the ABI and have been stable since Windows 2000:
    ///
    /// ```c
    /// typedef struct _MEMORYSTATUSEX {
    ///   DWORD     dwLength;                 // u32
    ///   DWORD     dwMemoryLoad;             // u32
    ///   DWORDLONG ullTotalPhys;             // u64
    ///   DWORDLONG ullAvailPhys;             // u64
    ///   DWORDLONG ullTotalPageFile;         // u64
    ///   DWORDLONG ullAvailPageFile;         // u64
    ///   DWORDLONG ullTotalVirtual;          // u64
    ///   DWORDLONG ullAvailVirtual;          // u64
    ///   DWORDLONG ullAvailExtendedVirtual;  // u64
    /// } MEMORYSTATUSEX;
    /// ```
    // Only `ull_avail_phys` is read, but every field must exist for the layout
    // to match what the API writes — dead_code would otherwise flag seven of
    // them on the Windows build.
    #[repr(C)]
    #[allow(dead_code)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    /// 2 × u32 then 7 × u64, with the u64s naturally aligned at offset 8 —
    /// 8 + 56 = 64 bytes and no tail padding.
    ///
    /// This is the whole safety argument for hand-declaring the struct: if the
    /// layout is wrong, `GlobalMemoryStatusEx` would write past the end of our
    /// allocation. Asserting the size turns that into a compile error on the
    /// Windows build instead.
    const MEMORYSTATUSEX_LAYOUT_CHECK: () = {
        assert!(std::mem::size_of::<MemoryStatusEx>() == 64);
        assert!(std::mem::align_of::<MemoryStatusEx>() == 8);
    };

    #[link(name = "kernel32")]
    extern "system" {
        /// Returns nonzero on success; zero means call `GetLastError`.
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub fn available_bytes() -> Option<u64> {
        // Force the layout assertion to be evaluated.
        let () = MEMORYSTATUSEX_LAYOUT_CHECK;

        let mut status = MemoryStatusEx {
            // MUST be set before the call — the API uses it to tell struct
            // versions apart, and leaving it zero makes the call fail. This is
            // the classic mistake with this function.
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };

        // SAFETY: `status` is a live, correctly sized and aligned
        // `MEMORYSTATUSEX` (see the layout assertion), `dw_length` is
        // initialised as the API requires, and the callee only writes within
        // that struct.
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 {
            return None;
        }

        // `ullAvailPhys` is physical memory available for allocation — the
        // closest analogue to Linux's MemAvailable. Deliberately NOT
        // `ullAvailPageFile` or `ullAvailVirtual`, which include swap and
        // address space and would badly overstate real headroom.
        Some(status.ull_avail_phys)
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
