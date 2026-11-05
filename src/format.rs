//! Column text for the file list, in the user's locale.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{StrFormatByteSizeW, StrFormatKBSizeW};
use windows::core::PCWSTR;

/// Explorer's size column: whole KB, rounded up, locale digit grouping. With decimal sizes
/// chosen in Settings, "12.3 MB" in units of 1,000.
pub fn size(bytes: u64) -> String {
    if crate::prefs::get().decimal_sizes {
        return decimal(bytes);
    }
    let mut buf = [0u16; 64];
    unsafe { StrFormatKBSizeW(bytes as i64, &mut buf) };
    from_wide_z(&buf)
}

/// Status-bar size: "1.50 MB", like Explorer's selection total.
pub fn bytes(bytes: u64) -> String {
    if crate::prefs::get().decimal_sizes {
        return decimal(bytes);
    }
    let mut buf = [0u16; 64];
    unsafe { StrFormatByteSizeW(bytes as i64, &mut buf) };
    from_wide_z(&buf)
}

/// `name` without its extension, as shown with extensions hidden: "report.pdf" is "report",
/// ".gitignore" stays whole.
pub fn without_extension(name: &[u16]) -> &[u16] {
    match name.iter().rposition(|&c| c == b'.' as u16) {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// Explorer's Attributes column: R H S A, then compressed, encrypted and link flags.
pub fn attributes(attrs: u32) -> String {
    use windows::Win32::Storage::FileSystem::*;
    [
        (FILE_ATTRIBUTE_READONLY, 'R'),
        (FILE_ATTRIBUTE_HIDDEN, 'H'),
        (FILE_ATTRIBUTE_SYSTEM, 'S'),
        (FILE_ATTRIBUTE_ARCHIVE, 'A'),
        (FILE_ATTRIBUTE_COMPRESSED, 'C'),
        (FILE_ATTRIBUTE_ENCRYPTED, 'E'),
        (FILE_ATTRIBUTE_REPARSE_POINT, 'L'),
    ]
    .iter()
    .filter(|(flag, _)| attrs & flag.0 != 0)
    .map(|&(_, c)| c)
    .collect()
}

/// Sizes in units of 1,000, with three significant digits: "999 bytes", "1.50 kB", "12.3 MB".
fn decimal(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in ["kB", "MB", "GB", "TB", "PB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.digits$} {unit}")
}

/// Local date and time without seconds, in the style chosen in Settings: the system's short
/// date, relative ("Today 14:02"), or ISO 8601. `0` (unknown) renders blank.
pub fn time(filetime: u64) -> String {
    if filetime == 0 {
        return String::new();
    }
    let ft = FILETIME { dwLowDateTime: filetime as u32, dwHighDateTime: (filetime >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return String::new();
        }
        let (mut d, mut t) = ([0u16; 80], [0u16; 80]);
        GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&local),
            PCWSTR::null(),
            Some(&mut d),
            PCWSTR::null(),
        );
        GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&local), PCWSTR::null(), Some(&mut t));
        let (date, clock) = (from_wide_z(&d), from_wide_z(&t));
        match crate::prefs::get().dates {
            crate::prefs::Dates::System => format!("{date} {clock}"),
            crate::prefs::Dates::Iso => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute
            ),
            crate::prefs::Dates::Relative => {
                let today = crate::groups::today();
                let day = crate::groups::local_day(filetime);
                match day.map(|d| today - d) {
                    Some(0) => format!("Today {clock}"),
                    Some(1) => format!("Yesterday {clock}"),
                    _ => format!("{date} {clock}"),
                }
            }
        }
    }
}

fn from_wide_z(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_sizes_use_thousands_and_three_digits() {
        assert_eq!(decimal(999), "999 bytes");
        assert_eq!(decimal(1_500), "1.50 kB");
        assert_eq!(decimal(12_345_678), "12.3 MB");
        assert_eq!(decimal(512_000_000_000), "512 GB");
    }

    #[test]
    fn sizes_round_up_to_kb_like_explorer() {
        assert_eq!(size(0), "0 KB");
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(1024), "1 KB");
        assert_eq!(size(1025), "2 KB");
        assert_eq!(size(500 * 1024), "500 KB");
    }

    #[test]
    fn times_render_in_local_date_and_time() {
        // 2024-06-15 12:00:00 UTC as FILETIME ticks.
        let s = time(133_628_976_000_000_000);
        assert!(s.contains("2024"), "{s}");
        assert!(s.contains(':'), "{s}");
    }

    #[test]
    fn attributes_read_like_explorer() {
        use windows::Win32::Storage::FileSystem::*;
        assert_eq!(attributes(0), "");
        assert_eq!(
            attributes(FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_READONLY.0 | FILE_ATTRIBUTE_ARCHIVE.0),
            "RHA"
        );
        assert_eq!(attributes(FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0), "L");
    }

    #[test]
    fn zero_time_is_blank() {
        assert_eq!(time(0), "");
    }
}
