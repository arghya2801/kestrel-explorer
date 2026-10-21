//! Group by date: Explorer's buckets ("Today", "Last week"…) over the rows in display order.

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

use crate::listing::Snapshot;

pub const LABELS: [&str; 8] = [
    "Today",
    "Yesterday",
    "Earlier this week",
    "Last week",
    "Earlier this month",
    "Last month",
    "Earlier this year",
    "A long time ago",
];

/// The local calendar day of a UTC FILETIME, as days since 1970-01-01. `None` if unknown.
pub fn local_day(filetime: u64) -> Option<i64> {
    if filetime == 0 {
        return None;
    }
    let ft = FILETIME { dwLowDateTime: filetime as u32, dwHighDateTime: (filetime >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(days_from_civil(local.wYear as i64, local.wMonth as i64, local.wDay as i64))
}

/// Today's local day.
pub fn today() -> i64 {
    local_day(crate::sort::now()).unwrap_or(0)
}

/// Days since 1970-01-01 (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// (year, month) of a day since 1970-01-01.
fn year_month(days: i64) -> (i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m)
}

/// Index into `LABELS` for `day` seen from `today`. Future dates count as today; unknown
/// dates as long ago.
/// ponytail: weeks start on Monday; read LOCALE_IFIRSTDAYOFWEEK if Sunday-first users mind.
pub fn bucket(day: Option<i64>, today: i64) -> usize {
    let Some(day) = day else { return 7 };
    let week_start = today - (today + 3).rem_euclid(7); // 1970-01-01 was a Thursday
    let ((y, m), (dy, dm)) = (year_month(today), year_month(day));
    let months_ago = (y * 12 + m) - (dy * 12 + dm);
    match day {
        d if d >= today => 0,
        d if d == today - 1 => 1,
        d if d >= week_start => 2,
        d if d >= week_start - 7 => 3,
        _ if months_ago == 0 => 4,
        _ if months_ago == 1 => 5,
        _ if dy == y => 6,
        _ => 7,
    }
}

/// Reorders entry indices `order` into date groups, newest first, keeping their order within
/// each group. Groups then run as contiguous rows, so the list's row order (ranges, Home/End,
/// type-to-select) matches what it shows.
pub fn group_order(s: &Snapshot, order: &mut [u32], today: i64) {
    order.sort_by_cached_key(|&e| bucket(local_day(s.entries[e as usize].modified), today));
}

/// The non-empty buckets, newest first, each with its rows in display order.
pub fn by_date(days: impl IntoIterator<Item = Option<i64>>, today: i64) -> Vec<(usize, Vec<u32>)> {
    let mut rows: [Vec<u32>; LABELS.len()] = Default::default();
    for (row, day) in days.into_iter().enumerate() {
        rows[bucket(day, today)].push(row as u32);
    }
    rows.into_iter().enumerate().filter(|(_, r)| !r.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 10, 21), 20747);
        for d in [-1, 0, 59, 20747, 11016] {
            let (y, m) = year_month(d);
            assert!(days_from_civil(y, m, 1) <= d && d < days_from_civil(y, m + 1, 1), "{d}");
        }
    }

    #[test]
    fn buckets_match_explorer() {
        // Wednesday 2026-10-21; that week started Monday the 19th.
        let today = days_from_civil(2026, 10, 21);
        let at = |m, d| bucket(Some(days_from_civil(2026, m, d)), today);
        assert_eq!(bucket(Some(today + 3), today), 0, "future");
        assert_eq!(at(10, 21), 0);
        assert_eq!(at(10, 20), 1);
        assert_eq!(at(10, 19), 2);
        assert_eq!(at(10, 18), 3);
        assert_eq!(at(10, 12), 3);
        assert_eq!(at(10, 11), 4);
        assert_eq!(at(9, 30), 5);
        assert_eq!(at(1, 1), 6);
        assert_eq!(bucket(Some(days_from_civil(2025, 12, 31)), today), 7);
        assert_eq!(bucket(None, today), 7);
    }

    #[test]
    fn last_month_wraps_the_year_and_yesterday_beats_last_week() {
        // Monday 2027-01-04: yesterday is in last week and last year.
        let today = days_from_civil(2027, 1, 4);
        assert_eq!(bucket(Some(today - 1), today), 1);
        assert_eq!(bucket(Some(days_from_civil(2026, 12, 20)), today), 5);
        assert_eq!(bucket(Some(days_from_civil(2026, 11, 20)), today), 7);
    }

    #[test]
    fn group_order_puts_newest_groups_first_and_keeps_order_within() {
        let mut s = Snapshot::new("C:\\".into());
        for (name, modified) in
            [("old-a", 0), ("new-a", crate::sort::now()), ("old-b", 0), ("new-b", crate::sort::now())]
        {
            s.add(&name.encode_utf16().collect::<Vec<_>>(), 0, 0, modified);
        }
        let mut order = vec![0, 1, 2, 3];
        group_order(&s, &mut order, today());
        assert_eq!(order, [1, 3, 0, 2]);
    }

    #[test]
    fn rows_keep_display_order_within_newest_first_groups() {
        let today = 100;
        let groups = by_date([Some(90), Some(100), None, Some(100), Some(99)], today);
        assert_eq!(groups, [(0, vec![1, 3]), (1, vec![4]), (3, vec![0]), (7, vec![2])]);
    }
}
