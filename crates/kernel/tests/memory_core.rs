use chrono::DateTime;
use chrono::Duration;
use chrono::FixedOffset;
use chrono::LocalResult;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::TimeZone;
use chrono::Utc;
use kernel::memory_core::DECAY_LAMBDA;
use kernel::memory_core::EMBED_INPUT_MAX;
use kernel::memory_core::FusionItem;
use kernel::memory_core::GATE_FRACTION;
use kernel::memory_core::HYBRID_POOL_MAX;
use kernel::memory_core::HYBRID_POOL_MIN;
use kernel::memory_core::REINFORCEMENT_ALPHA;
use kernel::memory_core::REINFORCEMENT_CAP;
use kernel::memory_core::SUMMARY_FUSION_BONUS;
use kernel::memory_core::W_FTS;
use kernel::memory_core::W_IMPORTANCE;
use kernel::memory_core::W_SEMANTIC;
use kernel::memory_core::W_TEMPORAL;
use kernel::memory_core::apply_embed_query_instruction;
use kernel::memory_core::composite_score;
use kernel::memory_core::fusion_score;
use kernel::memory_core::hybrid_pool;
use kernel::memory_core::literal_scan;
use kernel::memory_core::normalize_query_text;
use kernel::memory_core::passes_fusion_gate;
use kernel::memory_core::reciprocal_rank_fusion;
use kernel::memory_core::resolve_window;
use kernel::memory_core::should_trigram_fallback;
use kernel::memory_core::temporal_decay;
use serde::Deserialize;
use wire::gateway::memory::BrowseWindow;
use wire::gateway::memory::MemoryHitSource;

#[derive(Debug, Clone)]
struct Sample {
    id: String,
    source: MemoryHitSource,
    boost: f64,
}

impl FusionItem for Sample {
    fn fusion_id(&self) -> &str {
        &self.id
    }

    fn fusion_source(&self) -> MemoryHitSource {
        self.source
    }

    fn fusion_boost(&self) -> f64 {
        self.boost
    }
}

fn sample(id: &str, source: MemoryHitSource, boost: f64) -> Sample {
    Sample {
        id: id.to_owned(),
        source,
        boost,
    }
}

fn recording<T: for<'de> Deserialize<'de>>(name: &str) -> Result<Vec<T>, String> {
    let path = testkit::paths::recording(&format!("memory/diff/{name}.jsonl"));
    let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| format!("memory/diff/{name}.jsonl: {error} in {line}"))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct ScalarCase<I, O> {
    #[serde(rename = "fn")]
    function: String,
    #[serde(rename = "in")]
    input: I,
    out: O,
}

#[test]
fn normalize_query_text_replays_the_recording() -> Result<(), String> {
    for case in recording::<ScalarCase<String, String>>("normalize_query_text")? {
        assert_eq!(case.function, "normalize_query_text");
        assert_eq!(normalize_query_text(&case.input), case.out);
    }
    Ok(())
}

fn same_f64(actual: f64, expected: f64) -> bool {
    if actual.to_bits() == expected.to_bits() || (actual.is_nan() && expected.is_nan()) {
        return true;
    }
    let scale = actual.abs().max(expected.abs());
    (actual - expected).abs() <= scale.mul_add(1e-12, 1e-18)
}

#[test]
fn temporal_decay_replays_the_recording() -> Result<(), String> {
    let mut last_bit = 0u32;
    for case in recording::<ScalarCase<(f64, f64), f64>>("temporal_decay")? {
        assert_eq!(case.function, "temporal_decay");
        let (age_days, access_count) = case.input;
        let actual = temporal_decay(age_days, access_count);
        assert!(
            same_f64(actual, case.out),
            "age {age_days} access {access_count}: kernel {actual:.20e} recording {case_out:.20e}",
            case_out = case.out
        );
        if actual.to_bits() != case.out.to_bits() {
            last_bit += 1;
        }
    }
    assert_eq!(last_bit, 74);
    Ok(())
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
struct RecordedHit {
    id: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct FusionInput {
    lists: Vec<Vec<RecordedHit>>,
    k: f64,
}

#[derive(Debug, Deserialize)]
struct FusionOutput {
    key: String,
    rrf: f64,
    item: RecordedHit,
}

#[test]
fn reciprocal_rank_fusion_replays_the_recording() -> Result<(), String> {
    let mut last_bit = 0u32;
    for case in recording::<ScalarCase<FusionInput, Vec<FusionOutput>>>("reciprocal_rank_fusion")? {
        assert_eq!(case.function, "reciprocal_rank_fusion");
        let owned: Vec<Vec<Sample>> = case
            .input
            .lists
            .iter()
            .map(|arm| {
                arm.iter()
                    .map(recorded_sample)
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let arms: Vec<&[Sample]> = owned.iter().map(Vec::as_slice).collect();
        let fused = reciprocal_rank_fusion(&arms, case.input.k);
        assert_eq!(fused.len(), case.out.len());
        for (got, expected) in fused.iter().zip(&case.out) {
            assert_eq!(got.item.id, expected.key);
            assert_eq!(got.item.id, expected.item.id);
            assert_eq!(recorded_kind(got.item.source), expected.item.kind);
            assert!(
                same_f64(got.rrf, expected.rrf),
                "key {} k {}: kernel {:.20} recording {:.20}",
                expected.key,
                case.input.k,
                got.rrf,
                expected.rrf
            );
            if got.rrf.to_bits() != expected.rrf.to_bits() {
                last_bit += 1;
            }
        }
    }
    assert_eq!(last_bit, 172);
    Ok(())
}

fn recorded_sample(hit: &RecordedHit) -> Result<Sample, String> {
    Ok(sample(&hit.id, recorded_source(&hit.kind)?, 0.0))
}

fn recorded_source(kind: &str) -> Result<MemoryHitSource, String> {
    match kind {
        "summary" => Ok(MemoryHitSource::Summary),
        "message" => Ok(MemoryHitSource::Message),
        other => Err(format!("recording kind {other} is not a fusion source")),
    }
}

fn recorded_kind(source: MemoryHitSource) -> &'static str {
    match source {
        MemoryHitSource::Summary => "summary",
        MemoryHitSource::Message => "message",
    }
}

#[test]
fn apply_embed_query_instruction_replays_the_recording() -> Result<(), String> {
    let mut scalar_cuts = 0u32;
    for case in recording::<ScalarCase<(String, String), String>>("apply_embed_query_instruction")?
    {
        assert_eq!(case.function, "apply_embed_query_instruction");
        let (instruction, text) = case.input;
        let combined = format!("{instruction}{text}");
        let actual = apply_embed_query_instruction(&instruction, &text);
        assert_eq!(actual, scalar_prefix(&combined, EMBED_INPUT_MAX));
        if actual != case.out {
            scalar_cuts += 1;
        }
    }
    assert_eq!(scalar_cuts, 3);
    Ok(())
}

fn scalar_prefix(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

#[derive(Debug, Deserialize)]
struct WindowOutput {
    since: Option<String>,
    before: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordedThrow {
    name: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct WindowCase {
    #[serde(rename = "fn")]
    function: String,
    #[serde(rename = "in")]
    input: (String, String),
    out: Option<WindowOutput>,
    throws: Option<RecordedThrow>,
}

#[test]
fn resolve_window_replays_the_recording() -> Result<(), String> {
    let mut matched = 0u32;
    let mut morning = 0u32;
    let mut rejected = 0u32;
    let mut alias = 0u32;
    let mut pinned = 0u32;
    for case in recording::<WindowCase>("resolve_window")? {
        assert_eq!(case.function, "resolve_window");
        let (raw, now_raw) = case.input;
        match BrowseWindow::parse(&raw) {
            Ok(window) => {
                let now = DateTime::parse_from_rfc3339(&now_raw)
                    .unwrap()
                    .with_timezone(&Utc);
                let bounds = resolve_window(window, now, &Utc);
                let expected = case.out.expect("a parseable window has a recording result");
                assert!(case.throws.is_none());
                assert_eq!(stamp(bounds.since), expected.since, "{raw} at {now_raw}");
                if window == BrowseWindow::ThisMorning {
                    assert!(expected.before.is_none(), "{raw}");
                    let noon = bounds.since.map(|start| start + Duration::hours(12));
                    assert_eq!(bounds.before, noon, "{raw}");
                    morning += 1;
                } else {
                    assert_eq!(stamp(bounds.before), expected.before, "{raw} at {now_raw}");
                    matched += 1;
                }
            }
            Err(error) => {
                if let Some(throw) = case.throws {
                    assert_eq!(throw.name, "Error");
                    let wire = error.to_string();
                    assert_eq!(wire, wire_window_error(&raw), "{raw}");
                    assert_ne!(throw.message, wire, "{raw}");
                    pinned += pin_recorded_window_throw(&raw, &throw.message, &wire);
                    rejected += 1;
                } else {
                    alias += 1;
                }
            }
        }
    }
    assert_eq!((matched, morning, rejected, alias), (118, 20, 144, 218));
    assert_eq!(pinned, 24);
    Ok(())
}

fn stamp(bound: Option<DateTime<Utc>>) -> Option<String> {
    let time = bound?;
    Some(time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

const WINDOW_NAMES: &str = "today, yesterday, this_morning, this_week, last_24h, last_7d, last_14d";
const RECORDED_WINDOW_LIST: &str = "\"today\", \"yesterday\", \"this_morning\", \"this_week\", \"last_24h\", \"last_7d\", \"last_14d\"";

fn pin_recorded_window_throw(raw: &str, recorded: &str, wire: &str) -> u32 {
    let recorded_invalid = format!("Invalid window=\"\" — expected one of: {RECORDED_WINDOW_LIST}");
    match raw {
        "-" => {
            assert_eq!(recorded, recorded_invalid);
            assert_eq!(
                wire,
                format!(
                    "Unknown window=\"-\" (normalized to \"-\"). Expected one of: {WINDOW_NAMES}"
                )
            );
            1
        }
        "" => {
            assert_eq!(recorded, recorded_invalid);
            assert_eq!(
                wire,
                format!("Invalid window=\"\" — expected one of: {WINDOW_NAMES}")
            );
            1
        }
        "this  dayh" => {
            assert_eq!(
                recorded,
                format!(
                    "Unknown window=\"this  dayh\" (normalized to \"this_dayh\"). Expected one of: {RECORDED_WINDOW_LIST}"
                )
            );
            assert_eq!(
                wire,
                format!(
                    "Unknown window=\"this  dayh\" (normalized to \"this__dayh\"). Expected one of: {WINDOW_NAMES}"
                )
            );
            1
        }
        "past-14h" => {
            assert_eq!(
                recorded,
                format!(
                    "Unknown window=\"past-14h\" (normalized to \"past_14h\"). Expected one of: {RECORDED_WINDOW_LIST}"
                )
            );
            assert_eq!(
                wire,
                format!(
                    "Unknown window=\"past-14h\" (normalized to \"past-14h\"). Expected one of: {WINDOW_NAMES}"
                )
            );
            1
        }
        _ => 0,
    }
}

fn wire_window_error(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase().replace(' ', "_");
    if normalized.is_empty() {
        format!("Invalid window=\"\" — expected one of: {WINDOW_NAMES}")
    } else {
        format!(
            "Unknown window=\"{raw}\" (normalized to \"{normalized}\"). Expected one of: {WINDOW_NAMES}"
        )
    }
}

#[test]
fn recorded_aliases_stay_outside_resolve_window() -> Result<(), String> {
    let mut aliases = 0u32;
    let mut fortnight = false;
    let mut hyphen_week = false;
    for case in recording::<WindowCase>("resolve_window")? {
        let (raw, now_raw) = case.input;
        if BrowseWindow::parse(&raw).is_ok() || case.throws.is_some() {
            continue;
        }
        let out = case.out.expect("an alias row has a recording result");
        aliases += 1;
        if raw == "past_14_days" && now_raw == "2027-06-19T00:00:01.000Z" {
            assert_eq!(out.since.as_deref(), Some("2027-06-05T00:00:01.000Z"));
            assert_eq!(out.before, None);
            fortnight = true;
        }
        if raw == "this-week" && now_raw == "2026-01-28T02:13:18.000Z" {
            assert_eq!(out.since.as_deref(), Some("2026-01-26T00:00:00.000Z"));
            assert_eq!(out.before, None);
            hyphen_week = true;
        }
    }
    assert_eq!(aliases, 218);
    assert!(fortnight);
    assert!(hyphen_week);
    Ok(())
}

#[test]
fn scoring_literals_match_main_delta() {
    assert_eq!(DECAY_LAMBDA.to_bits(), 0.05_f64.to_bits());
    assert_eq!(REINFORCEMENT_ALPHA.to_bits(), 0.01_f64.to_bits());
    assert_eq!(REINFORCEMENT_CAP, 25);
    assert_eq!(W_FTS.to_bits(), 0.3_f64.to_bits());
    assert_eq!(W_SEMANTIC.to_bits(), 0.3_f64.to_bits());
    assert_eq!(W_TEMPORAL.to_bits(), 0.3_f64.to_bits());
    assert_eq!(W_IMPORTANCE.to_bits(), 0.1_f64.to_bits());
    assert_eq!(SUMMARY_FUSION_BONUS.to_bits(), 1.3_f64.to_bits());
    assert_eq!(GATE_FRACTION.to_bits(), 0.5_f64.to_bits());
    assert_eq!(composite_score(1.0, 2.0, 0.5).to_bits(), 1.4_f64.to_bits());
    assert_eq!(
        fusion_score(1.0, &sample("s", MemoryHitSource::Summary, 0.0)).to_bits(),
        1.3_f64.to_bits()
    );
    assert_eq!(
        fusion_score(1.0, &sample("m", MemoryHitSource::Message, 0.0)).to_bits(),
        1.0_f64.to_bits()
    );
    assert_eq!(
        fusion_score(0.04, &sample("s", MemoryHitSource::Summary, 0.25)).to_bits(),
        (0.04_f64 * 1.25 * 1.3).to_bits()
    );
    assert_eq!(
        fusion_score(0.04, &sample("m", MemoryHitSource::Message, 0.25)).to_bits(),
        (0.04_f64 * 1.25).to_bits()
    );
}

#[test]
fn hybrid_pool_clamps_between_the_recorded_bounds() {
    assert_eq!(hybrid_pool(1), HYBRID_POOL_MIN);
    assert_eq!(hybrid_pool(20), 60);
    assert_eq!(hybrid_pool(100), HYBRID_POOL_MAX);
}

#[test]
fn fusion_gate_keeps_a_second_arm_or_half_the_top_score() {
    assert!(passes_fusion_gate(2, 0.0, 1.0));
    assert!(passes_fusion_gate(1, 0.5, 1.0));
    assert!(!passes_fusion_gate(1, 0.49, 1.0));
}

#[test]
fn literal_scan_matches_paths_addresses_and_mixed_tokens() {
    assert!(literal_scan("notes.md"));
    assert!(literal_scan("192.0.2.1"));
    assert!(literal_scan("a1"));
    assert!(literal_scan("1b"));
    assert!(!literal_scan("plain words"));
    assert!(should_trigram_fallback("a.b"));
    assert!(should_trigram_fallback("notes.md"));
    assert!(!should_trigram_fallback("plain words"));
}

#[derive(Clone, Copy)]
struct Transition {
    before: FixedOffset,
    after: FixedOffset,
    at_utc: NaiveDateTime,
    later_first: bool,
}

impl Transition {
    fn maps(self, local: &NaiveDateTime, offset: FixedOffset) -> bool {
        local
            .checked_sub_offset(offset)
            .is_some_and(|utc| self.offset_from_utc_datetime(&utc) == offset)
    }

    fn fold_pair(self) -> (FixedOffset, FixedOffset) {
        if self.before.local_minus_utc() >= self.after.local_minus_utc() {
            (self.before, self.after)
        } else {
            (self.after, self.before)
        }
    }

    fn later_instant_first(mut self) -> Self {
        self.later_first = true;
        self
    }
}

impl TimeZone for Transition {
    type Offset = FixedOffset;

    fn from_offset(offset: &FixedOffset) -> Self {
        Self {
            before: *offset,
            after: *offset,
            at_utc: DateTime::<Utc>::UNIX_EPOCH.naive_utc(),
            later_first: false,
        }
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<FixedOffset> {
        self.offset_from_local_datetime(&NaiveDateTime::new(*local, NaiveTime::MIN))
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
        match (self.maps(local, self.before), self.maps(local, self.after)) {
            (true, false) => LocalResult::Single(self.before),
            (false, true) => LocalResult::Single(self.after),
            (true, true) if self.before == self.after => LocalResult::Single(self.before),
            (true, true) => {
                let (earlier, later) = self.fold_pair();
                if self.later_first {
                    LocalResult::Ambiguous(later, earlier)
                } else {
                    LocalResult::Ambiguous(earlier, later)
                }
            }
            (false, false) => LocalResult::None,
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
        self.offset_from_utc_datetime(&NaiveDateTime::new(*utc, NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
        if *utc < self.at_utc {
            self.before
        } else {
            self.after
        }
    }
}

fn utc(text: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| error.to_string())
}

fn offset_hours(hours: i32) -> Result<FixedOffset, String> {
    FixedOffset::east_opt(hours * 3600).ok_or_else(|| format!("offset {hours}h is out of range"))
}

fn transition(before_hours: i32, after_hours: i32, at: &str) -> Result<Transition, String> {
    Ok(Transition {
        before: offset_hours(before_hours)?,
        after: offset_hours(after_hours)?,
        at_utc: utc(at)?.naive_utc(),
        later_first: false,
    })
}

fn local_midnight(year: i32, month: u32, day: u32) -> Result<NaiveDateTime, String> {
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| format!("date {year}-{month:02}-{day:02} is not a calendar day"))?;
    date.and_hms_opt(0, 0, 0)
        .ok_or_else(|| format!("date {year}-{month:02}-{day:02} has no midnight"))
}

fn assert_day_start(
    zone: &Transition,
    window: BrowseWindow,
    now: DateTime<Utc>,
    expected: DateTime<Utc>,
) {
    let bounds = resolve_window(window, now, zone);
    assert_eq!(bounds.since, Some(expected), "{window:?} at {now}");
    let date = expected.with_timezone(zone).date_naive();
    let prev = expected - Duration::seconds(1);
    assert_ne!(
        prev.with_timezone(zone).date_naive(),
        date,
        "{window:?} at {now}"
    );
}

#[test]
fn calendar_windows_keep_the_earlier_midnight_when_the_clock_falls_back() -> Result<(), String> {
    let zone = transition(-3, -4, "2024-04-01T04:00:00Z")?;
    let midnight = local_midnight(2024, 4, 1)?;
    assert!(matches!(
        zone.from_local_datetime(&midnight),
        LocalResult::Ambiguous(_, _)
    ));
    let earliest = utc("2024-04-01T03:00:00Z")?;
    let monday = utc("2024-04-01T12:00:00Z")?;
    let tuesday = utc("2024-04-02T12:00:00Z")?;
    assert_day_start(&zone, BrowseWindow::Today, monday, earliest);
    assert_day_start(&zone, BrowseWindow::ThisMorning, monday, earliest);
    assert_day_start(&zone, BrowseWindow::ThisWeek, monday, earliest);
    assert_day_start(&zone, BrowseWindow::Yesterday, tuesday, earliest);
    let morning = resolve_window(BrowseWindow::ThisMorning, monday, &zone);
    assert_eq!(
        morning.before,
        morning.since.map(|start| start + Duration::hours(12))
    );
    let yesterday = resolve_window(BrowseWindow::Yesterday, tuesday, &zone);
    assert_eq!(yesterday.before, Some(utc("2024-04-02T04:00:00Z")?));
    assert_eq!(
        resolve_window(BrowseWindow::Last24h, monday, &zone).since,
        Some(monday - Duration::hours(24))
    );
    Ok(())
}

#[test]
fn calendar_windows_keep_the_first_instant_when_midnight_does_not_exist() -> Result<(), String> {
    let zone = transition(-4, -3, "2024-04-01T04:00:00Z")?;
    let midnight = local_midnight(2024, 4, 1)?;
    assert!(matches!(
        zone.from_local_datetime(&midnight),
        LocalResult::None
    ));
    let first = utc("2024-04-01T04:00:00Z")?;
    let monday = utc("2024-04-01T12:00:00Z")?;
    let tuesday = utc("2024-04-02T12:00:00Z")?;
    let wednesday = utc("2024-04-03T12:00:00Z")?;
    assert_day_start(&zone, BrowseWindow::Today, monday, first);
    assert_day_start(&zone, BrowseWindow::ThisMorning, monday, first);
    assert_day_start(&zone, BrowseWindow::ThisWeek, monday, first);
    assert_day_start(&zone, BrowseWindow::Yesterday, tuesday, first);
    assert_day_start(
        &zone,
        BrowseWindow::Today,
        wednesday,
        utc("2024-04-03T03:00:00Z")?,
    );
    let morning = resolve_window(BrowseWindow::ThisMorning, monday, &zone);
    assert_eq!(
        morning.before,
        morning.since.map(|start| start + Duration::hours(12))
    );
    let yesterday = resolve_window(BrowseWindow::Yesterday, tuesday, &zone);
    assert_eq!(yesterday.before, Some(utc("2024-04-02T03:00:00Z")?));
    Ok(())
}

#[test]
fn a_midnight_inside_the_gap_is_the_end_of_the_gap() -> Result<(), String> {
    let zone = transition(-4, -2, "2024-04-01T03:00:00Z")?;
    assert!(matches!(
        zone.from_local_datetime(&local_midnight(2024, 4, 1)?),
        LocalResult::None
    ));
    assert_day_start(
        &zone,
        BrowseWindow::Today,
        utc("2024-04-01T12:00:00Z")?,
        utc("2024-04-01T03:00:00Z")?,
    );
    Ok(())
}

#[test]
fn calendar_windows_keep_the_earlier_midnight_when_local_lists_the_later_instant_first()
-> Result<(), String> {
    let zone = transition(-4, -5, "2016-11-06T05:00:00Z")?.later_instant_first();
    let midnight = local_midnight(2016, 11, 6)?;
    let LocalResult::Ambiguous(later, earlier) = zone.from_local_datetime(&midnight) else {
        return Err(String::from("havana midnight is not ambiguous"));
    };
    assert_eq!(later.with_timezone(&Utc), utc("2016-11-06T05:00:00Z")?);
    assert_eq!(earlier.with_timezone(&Utc), utc("2016-11-06T04:00:00Z")?);
    let start = utc("2016-11-06T04:00:00Z")?;
    let sunday = utc("2016-11-06T12:00:00Z")?;
    let next_day = utc("2016-11-07T12:00:00Z")?;
    assert_day_start(&zone, BrowseWindow::Today, sunday, start);
    assert_day_start(&zone, BrowseWindow::ThisMorning, sunday, start);
    assert_day_start(&zone, BrowseWindow::Yesterday, next_day, start);
    assert_day_start(
        &zone,
        BrowseWindow::ThisWeek,
        sunday,
        utc("2016-10-31T04:00:00Z")?,
    );
    let morning = resolve_window(BrowseWindow::ThisMorning, sunday, &zone);
    assert_eq!(
        morning.before,
        morning.since.map(|since| since + Duration::hours(12))
    );
    let yesterday = resolve_window(BrowseWindow::Yesterday, next_day, &zone);
    assert_eq!(yesterday.before, Some(utc("2016-11-07T05:00:00Z")?));
    let week = transition(-3, -4, "2024-04-01T04:00:00Z")?.later_instant_first();
    assert_day_start(
        &week,
        BrowseWindow::ThisWeek,
        utc("2024-04-01T12:00:00Z")?,
        utc("2024-04-01T03:00:00Z")?,
    );
    Ok(())
}
