use chrono::DateTime;
use chrono::Datelike as _;
use chrono::Duration;
use chrono::NaiveDate;
use chrono::TimeZone;
use chrono::Utc;
use wire::gateway::memory::BrowseWindow;

#[must_use]
pub fn resolve_window<Tz: TimeZone>(
    window: BrowseWindow,
    now: DateTime<Utc>,
    zone: &Tz,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let today = now.with_timezone(zone).date_naive();
    match window {
        BrowseWindow::Today => (midnight_utc(today, zone), None),
        BrowseWindow::Yesterday => (
            midnight_utc(today - Duration::days(1), zone),
            midnight_utc(today, zone),
        ),
        BrowseWindow::ThisMorning => (
            midnight_utc(today, zone),
            midnight_utc(today, zone).map(|start| start + Duration::hours(12)),
        ),
        BrowseWindow::ThisWeek => (
            midnight_utc(
                today - Duration::days(i64::from(today.weekday().num_days_from_monday())),
                zone,
            ),
            None,
        ),
        BrowseWindow::Last24h => (Some(now - Duration::hours(24)), None),
        BrowseWindow::Last7d => (Some(now - Duration::days(7)), None),
        BrowseWindow::Last14d => (Some(now - Duration::days(14)), None),
    }
}

fn midnight_utc<Tz: TimeZone>(date: NaiveDate, zone: &Tz) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(0, 0, 0)?;
    zone.from_local_datetime(&naive)
        .single()
        .map(|local| local.with_timezone(&Utc))
}
