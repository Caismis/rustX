//! The pinned parser owns calendar matching and DST policy; no cron engine runs.
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use croner::{
    Cron,
    errors::CronError,
    parser::{CronParser, Seconds, Year},
};

pub(crate) const LAST_CRON_UTC_YEAR: i32 = croner::YEAR_UPPER_LIMIT - 2;
fn cutoff() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(LAST_CRON_UTC_YEAR, 12, 31, 23, 59, 59)
        .single()
        .expect("fixed calendar bound")
}
fn parse(expression: &str, zone: &str) -> Result<(Cron, Tz, String), String> {
    if expression.len() > 256
        || zone.len() > 128
        || expression.split_ascii_whitespace().count() != 5
        || !expression.bytes().all(|byte| {
            byte.is_ascii_digit() || byte.is_ascii_whitespace() || b"*,/-".contains(&byte)
        })
    {
        return Err(
            "Cron requires five numeric fields using spaces and * , - / (at most 256 bytes)".into(),
        );
    }
    let normalized = expression
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let zone = zone
        .parse::<Tz>()
        .map_err(|_| "Cron requires an explicit IANA time zone".to_owned())?;
    let cron = CronParser::builder()
        .seconds(Seconds::Disallowed)
        .year(Year::Disallowed)
        .dom_and_dow(false)
        .alternative_weekdays(false)
        .sloppy_ranges(false)
        .build()
        .parse(&normalized)
        .map_err(|error| format!("invalid Cron: {error}"))?;
    Ok((cron, zone, normalized))
}
fn search(result: Result<DateTime<Tz>, CronError>) -> Result<Option<DateTime<Utc>>, String> {
    match result {
        Ok(at) => {
            Ok((at.with_timezone(&Utc).year() <= LAST_CRON_UTC_YEAR)
                .then(|| at.with_timezone(&Utc)))
        }
        Err(CronError::TimeSearchLimitExceeded) => Ok(None),
        Err(error) => Err(format!("Cron calendar search failed: {error}")),
    }
}
fn next(cron: &Cron, zone: Tz, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
    if now >= cutoff() {
        return Ok(None);
    }
    search(
        cron.find_next_occurrence(
            &now.with_nanosecond(0)
                .expect("valid UTC second")
                .with_timezone(&zone),
            false,
        ),
    )
}
pub(super) fn prepare(
    expression: &str,
    zone: &str,
    now: DateTime<Utc>,
) -> Result<(String, String, DateTime<Utc>), String> {
    let (cron, zone, normalized) = parse(expression, zone)?;
    let first = next(&cron, zone, now)?
        .ok_or_else(|| "Cron has no future occurrence through UTC year 4998".to_owned())?;
    Ok((normalized, zone.to_string(), first))
}
pub(super) fn occurrence(
    expression: &str,
    zone: &str,
    saved_due: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(DateTime<Utc>, Option<DateTime<Utc>>), String> {
    let (cron, zone, _) = parse(expression, zone)?;
    let decision = now
        .min(cutoff())
        .with_nanosecond(0)
        .expect("valid UTC second")
        .with_timezone(&zone);
    let latest = search(cron.find_previous_occurrence(&decision, true))?
        .filter(|at| *at <= now)
        .unwrap_or(saved_due)
        .max(saved_due);
    // A committed UTC candidate survives timezone-data changes that no longer regenerate it.
    Ok((latest, next(&cron, zone, now)?))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn at(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }
    fn next_at(expression: &str, zone: &str, from: &str) -> DateTime<Utc> {
        prepare(expression, zone, at(from)).unwrap().2
    }
    #[test]
    fn numeric_grammar_and_explicit_zone_are_bounded() {
        for expression in [
            "@daily",
            "0 * * * * *",
            "0 * * * * * *",
            "0 0 * JAN *",
            "0 0 L * *",
            "0 0 * * MON",
            "0 0 * * ?",
            "0 0 1W * *",
            "0 0 * * 1#2",
            "60 * * * *",
            "*/0 * * * *",
        ] {
            assert!(
                prepare(expression, "UTC", at("2026-01-01T00:00:00Z")).is_err(),
                "{expression}"
            );
        }
        assert!(prepare("* * * * *", "local", at("2026-01-01T00:00:00Z")).is_err());
        assert!(prepare(&"*".repeat(257), "UTC", at("2026-01-01T00:00:00Z")).is_err());
    }
    #[test]
    fn dom_dow_wildcard_provenance_uses_the_pinned_standard_policy() {
        assert_eq!(
            next_at("0 0 1 * 1", "UTC", "2026-10-22T00:00:00Z"),
            at("2026-10-26T00:00:00Z")
        );
        assert_eq!(
            next_at("0 0 * * 1", "UTC", "2026-10-22T00:00:00Z"),
            at("2026-10-26T00:00:00Z")
        );
        assert_eq!(
            next_at("0 0 1-31 * 1", "UTC", "2026-10-22T00:00:00Z"),
            at("2026-10-23T00:00:00Z")
        );
    }
    #[test]
    fn fixed_time_gap_moves_to_first_valid_time_and_reverse_search_agrees() {
        let expected = at("2026-03-08T07:00:00Z");
        assert_eq!(
            next_at("30 2 * * *", "America/New_York", "2026-03-08T06:59:00Z"),
            expected
        );
        assert_eq!(
            occurrence(
                "30 2 * * *",
                "America/New_York",
                expected,
                at("2026-03-08T09:00:00Z")
            )
            .unwrap()
            .0,
            expected
        );
    }
    #[test]
    fn fixed_overlap_runs_once_while_wildcards_visit_both_utc_passes() {
        assert_eq!(
            next_at("30 1 * * *", "America/New_York", "2026-11-01T05:00:00Z"),
            at("2026-11-01T05:30:00Z")
        );
        assert_eq!(
            next_at("30 1 * * *", "America/New_York", "2026-11-01T05:30:00Z"),
            at("2026-11-02T06:30:00Z")
        );
        assert_eq!(
            next_at("*/30 * * * *", "America/New_York", "2026-11-01T05:30:00Z"),
            at("2026-11-01T06:00:00Z")
        );
        assert_eq!(
            next_at("*/30 * * * *", "America/New_York", "2026-11-01T06:00:00Z"),
            at("2026-11-01T06:30:00Z")
        );
        assert_eq!(
            occurrence(
                "30 1 * * *",
                "America/New_York",
                at("2026-11-01T05:30:00Z"),
                at("2026-11-01T07:00:00Z")
            )
            .unwrap()
            .0,
            at("2026-11-01T05:30:00Z")
        );
    }
    #[test]
    fn leap_day_impossible_dates_and_final_utc_year_are_explicit() {
        assert_eq!(
            next_at("0 0 29 2 *", "UTC", "2026-01-01T00:00:00Z"),
            at("2028-02-29T00:00:00Z")
        );
        assert!(prepare("0 0 31 2 *", "UTC", at("2026-01-01T00:00:00Z")).is_err());
        let last = next_at("* * * * *", "UTC", "4998-12-31T23:58:00Z");
        assert_eq!(last, at("4998-12-31T23:59:00Z"));
        assert_eq!(
            next_at("* * * * *", "Pacific/Kiritimati", "4998-12-31T23:58:00Z"),
            last
        );
        assert_eq!(
            next_at("* * * * *", "Etc/GMT+12", "4998-12-31T23:58:00Z"),
            last
        );
        assert_eq!(
            occurrence("* * * * *", "UTC", last, at("4998-12-31T23:59:59Z")).unwrap(),
            (last, None)
        );
        assert_eq!(
            occurrence(
                "* * * * *",
                "Pacific/Kiritimati",
                last,
                at("5001-01-01T00:00:00Z")
            )
            .unwrap(),
            (last, None)
        );
        assert!(prepare("* * * * *", "UTC", at("5000-01-01T00:00:00Z")).is_err());
    }
    #[test]
    fn latest_only_catchup_preserves_a_saved_candidate_and_fractional_decisions() {
        let saved = at("2026-10-23T10:00:13Z");
        let now = at("2026-10-23T10:00:20.500Z");
        assert_eq!(
            occurrence("0 * * * *", "UTC", saved, now).unwrap(),
            (saved, Some(at("2026-10-23T11:00:00Z")))
        );
        assert_eq!(
            next_at("* * * * *", "UTC", "2026-10-23T10:00:00.500Z"),
            at("2026-10-23T10:01:00Z")
        );
        let now = at("2026-12-23T10:37:42Z");
        assert_eq!(
            occurrence("*/10 * * * *", "UTC", saved, now).unwrap(),
            (at("2026-12-23T10:30:00Z"), Some(at("2026-12-23T10:40:00Z")))
        );
    }
}
