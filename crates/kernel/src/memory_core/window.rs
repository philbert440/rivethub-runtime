use chrono::DateTime;
use chrono::Datelike as _;
use chrono::Duration;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Offset as _;
use chrono::TimeZone;
use chrono::Utc;
use wire::gateway::memory::BrowseWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub since: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

#[must_use]
pub fn resolve_window<Tz: TimeZone>(
    window: BrowseWindow,
    now: DateTime<Utc>,
    zone: &Tz,
) -> TimeRange {
    let today = now.with_timezone(zone).date_naive();
    match window {
        BrowseWindow::Today => TimeRange {
            since: Some(midnight_utc(today, zone)),
            before: None,
        },
        BrowseWindow::Yesterday => TimeRange {
            since: Some(midnight_utc(today - Duration::days(1), zone)),
            before: Some(midnight_utc(today, zone)),
        },
        BrowseWindow::ThisMorning => {
            let start = midnight_utc(today, zone);
            TimeRange {
                since: Some(start),
                before: Some(start + Duration::hours(12)),
            }
        }
        BrowseWindow::ThisWeek => TimeRange {
            since: Some(midnight_utc(
                today - Duration::days(i64::from(today.weekday().num_days_from_monday())),
                zone,
            )),
            before: None,
        },
        BrowseWindow::Last24h => TimeRange {
            since: Some(now - Duration::hours(24)),
            before: None,
        },
        BrowseWindow::Last7d => TimeRange {
            since: Some(now - Duration::days(7)),
            before: None,
        },
        BrowseWindow::Last14d => TimeRange {
            since: Some(now - Duration::days(14)),
            before: None,
        },
    }
}

fn midnight_utc<Tz: TimeZone>(date: NaiveDate, zone: &Tz) -> DateTime<Utc> {
    let naive = NaiveDateTime::new(date, NaiveTime::MIN);
    match zone.from_local_datetime(&naive) {
        LocalResult::Single(local) => local.with_timezone(&Utc),
        LocalResult::Ambiguous(first, second) => first.min(second).with_timezone(&Utc),
        LocalResult::None => after_gap(naive, zone),
    }
}

fn after_gap<Tz: TimeZone>(local: NaiveDateTime, zone: &Tz) -> DateTime<Utc> {
    let after = zone
        .offset_from_utc_datetime(&(local + Duration::days(1)))
        .fix();
    let before = zone
        .offset_from_utc_datetime(&(local - Duration::days(1)))
        .fix();
    let gap = i64::from(after.local_minus_utc()) - i64::from(before.local_minus_utc());
    let first = local + Duration::seconds(seconds_until_existing(zone, local, gap));
    (first - after).and_utc()
}

fn seconds_until_existing<Tz: TimeZone>(zone: &Tz, local: NaiveDateTime, gap: i64) -> i64 {
    let mut lo = 0i64;
    let mut hi = gap.max(0);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let candidate = local + Duration::seconds(mid);
        if zone.from_local_datetime(&candidate).earliest().is_some() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}
