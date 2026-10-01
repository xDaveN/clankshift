//! Pure timing decisions: window state, daily trigger times. No I/O here.

use jiff::civil::Time;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

/// Both providers currently use a 5-hour short-term window.
pub const WINDOW_SECS: i64 = 5 * 3600;

/// A reset time within this much of `now + WINDOW_SECS` means the window started just now.
/// Covers request latency and providers that round reset times.
const STARTED_NOW_TOLERANCE_SECS: i64 = 10 * 60;

/// An automatic start more than this late (machine asleep/off, provider failing) is skipped, not
/// run late: it would put the reset somewhere the user did not ask for.
pub const GRACE_SECS: i64 = 3600;

/// Pause between attempts after an automatic start fails.
const RETRY_SECS: i64 = 10 * 60;

/// When to retry an automatic start triggered at `triggered` that failed at `now`; `None` = give up.
pub fn next_retry(triggered: i64, now: i64) -> Option<i64> {
    let at = now + RETRY_SECS;
    (at - triggered <= GRACE_SECS).then_some(at)
}

/// Retry origin for a daily start due at `due` and reached at `now`: the scheduled time, not `now`,
/// so a late start does not get a fresh hour of retries. `None` = missed by more than the grace.
pub fn daily_trigger(due: i64, now: i64) -> Option<i64> {
    (now - due <= GRACE_SECS).then_some(due)
}

/// A window is known active only while the provider-reported reset time is in the future.
pub fn known_active(resets_at: Option<i64>, now: i64) -> bool {
    resets_at.is_some_and(|r| r > now)
}

/// Did the window reporting this reset time start at (about) `now`? If not, it was already running.
pub fn started_now(resets_at: i64, now: i64) -> bool {
    resets_at > now + WINDOW_SECS - STARTED_NOW_TOLERANCE_SECS
}

/// First occurrence of local time `t` strictly after `after`.
pub fn next_daily(after: Timestamp, t: Time, tz: &TimeZone) -> Timestamp {
    let mut date = after.to_zoned(tz.clone()).date();
    loop {
        // DST gaps resolve "compatibly" (e.g. 02:30 on spring-forward day becomes 03:30).
        if let Ok(z) = date.to_datetime(t).to_zoned(tz.clone())
            && z.timestamp() > after
        {
            return z.timestamp();
        }
        date = date.checked_add(1.day()).expect("date overflow");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::time;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn window_classification() {
        let now = 1_000_000;
        assert!(!known_active(None, now));
        assert!(!known_active(Some(now), now));
        assert!(known_active(Some(now + 1), now));
        // Fresh window: resets ~5h from now (allowing latency/rounding).
        assert!(started_now(now + WINDOW_SECS, now));
        assert!(started_now(now + WINDOW_SECS - 5 * 60, now));
        // Window that has been running for an hour.
        assert!(!started_now(now + WINDOW_SECS - 3600, now));
    }

    #[test]
    fn retry_only_within_grace() {
        let t = 1_000_000;
        assert_eq!(next_retry(t, t), Some(t + RETRY_SECS));
        assert_eq!(
            next_retry(t, t + GRACE_SECS - RETRY_SECS),
            Some(t + GRACE_SECS)
        );
        assert_eq!(next_retry(t, t + GRACE_SECS - RETRY_SECS + 1), None);
    }

    #[test]
    fn late_daily_start_retries_until_scheduled_time_plus_grace() {
        let due = 1_000_000;
        // Reached 40 min late: retries are measured from `due`, so 65 min after it is too late.
        let since = daily_trigger(due, due + 40 * 60).unwrap();
        assert_eq!(since, due);
        assert_eq!(next_retry(since, due + 40 * 60), Some(due + 50 * 60));
        assert_eq!(next_retry(since, due + 55 * 60), None);
        assert_eq!(daily_trigger(due, due + GRACE_SECS), Some(due));
        assert_eq!(daily_trigger(due, due + GRACE_SECS + 1), None);
    }

    #[test]
    fn next_daily_today_or_tomorrow() {
        let tz = TimeZone::get("Europe/London").unwrap();
        // 06:00 BST (05:00Z) -> 07:00 BST same day.
        assert_eq!(
            next_daily(ts("2026-07-01T05:00:00Z"), time(7, 0, 0, 0), &tz),
            ts("2026-07-01T06:00:00Z")
        );
        // Exactly at the trigger -> next day.
        assert_eq!(
            next_daily(ts("2026-07-01T06:00:00Z"), time(7, 0, 0, 0), &tz),
            ts("2026-07-02T06:00:00Z")
        );
    }

    #[test]
    fn next_daily_across_dst() {
        let tz = TimeZone::get("Europe/London").unwrap();
        // Clocks go forward 2026-03-29 01:00 GMT -> 02:00 BST. 07:00 local is 06:00Z after the change.
        assert_eq!(
            next_daily(ts("2026-03-28T12:00:00Z"), time(7, 0, 0, 0), &tz),
            ts("2026-03-29T06:00:00Z")
        );
        // 01:30 does not exist that day; resolves to 02:30 BST = 01:30Z.
        assert_eq!(
            next_daily(ts("2026-03-28T12:00:00Z"), time(1, 30, 0, 0), &tz),
            ts("2026-03-29T01:30:00Z")
        );
    }
}
