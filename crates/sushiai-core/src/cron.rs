//! Five-field cron expressions read in an IANA time zone. Pure: no clock, no IO.
//!
//! Fields: minute hour day-of-month month day-of-week. Each field takes `*`, a number, a
//! range `a-b`, a step `*/n`, `a-b/n` or `a/n`, and comma lists of those. Weekday 7 is
//! Sunday. When both day-of-month and day-of-week are restricted, either one matches (as
//! in Vixie cron).
//!
//! Daylight saving: a wall time that does not exist (spring forward) fires at the same
//! offset after the gap, so a daily job is not lost; a wall time that happens twice (fall
//! back) fires once, at its first occurrence, unless the hour field is `*`: then every
//! real hour runs, so the repeated hour fires again.

use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
pub use chrono_tz::Tz as Zone;
use chrono_tz::Tz;

/// The shortest gap the owner may schedule.
pub const MIN_INTERVAL_SECS: i64 = 300;

/// How many days ahead a search looks (covers 29 February).
const SEARCH_DAYS: u32 = 366 * 8;

const ALL_HOURS: u64 = (1 << 24) - 1;

/// How many consecutive fires are compared for the minimum gap.
const GAP_SAMPLES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    #[error("a cron needs 5 fields: minute hour day-of-month month day-of-week")]
    FieldCount,
    #[error("{field}: {reason}")]
    Field { field: &'static str, reason: String },
    #[error("unknown time zone {0}")]
    Zone(String),
    #[error("this cron never fires")]
    Never,
    #[error("fires every {gap_secs}s; the minimum is {MIN_INTERVAL_SECS}s")]
    TooFrequent { gap_secs: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    days_any: bool,
    weekdays_any: bool,
}

struct Field {
    mask: u64,
    any: bool,
}

fn parse_field(
    name: &'static str,
    text: &str,
    min: u32,
    max: u32,
    seven_is_sunday: bool,
) -> Result<Field, CronError> {
    let bad = |reason: String| CronError::Field {
        field: name,
        reason,
    };
    let number = |t: &str| -> Result<u32, CronError> {
        t.parse::<u32>()
            .map_err(|_| bad(format!("{t:?} is not a number")))
    };
    let top = if seven_is_sunday { 7 } else { max };
    let mut mask = 0u64;
    for part in text.split(',') {
        if part.is_empty() {
            return Err(bad("empty list item".into()));
        }
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step = number(s)?;
                if step == 0 {
                    return Err(bad("step must be at least 1".into()));
                }
                (r, Some(step))
            }
            None => (part, None),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (number(a)?, number(b)?)
        } else {
            let n = number(range)?;
            // `a/n` runs from a to the top of the field.
            (n, if step.is_some() { top } else { n })
        };
        if lo < min || hi > top || lo > hi {
            return Err(bad(format!("{part:?} is outside {min}-{top}")));
        }
        let mut v = lo;
        while v <= hi {
            let bit = if seven_is_sunday && v == 7 { 0 } else { v };
            mask |= 1u64 << bit;
            v += step.unwrap_or(1);
        }
    }
    Ok(Field {
        mask,
        any: text.starts_with('*'),
    })
}

/// Parses an IANA zone name.
pub fn parse_zone(name: &str) -> Result<Tz, CronError> {
    name.parse::<Tz>()
        .map_err(|_| CronError::Zone(name.to_string()))
}

impl Cron {
    pub fn parse(text: &str) -> Result<Cron, CronError> {
        let parts: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = parts[..] else {
            return Err(CronError::FieldCount);
        };
        let minutes = parse_field("minute", minute, 0, 59, false)?;
        let hours = parse_field("hour", hour, 0, 23, false)?;
        let days = parse_field("day-of-month", day, 1, 31, false)?;
        let months = parse_field("month", month, 1, 12, false)?;
        let weekdays = parse_field("day-of-week", weekday, 0, 6, true)?;
        Ok(Cron {
            minutes: minutes.mask,
            hours: hours.mask,
            days: days.mask,
            months: months.mask,
            weekdays: weekdays.mask,
            days_any: days.any,
            weekdays_any: weekdays.any,
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        if self.months & (1 << date.month()) == 0 {
            return false;
        }
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().num_days_from_sunday()) != 0;
        match (self.days_any, self.weekdays_any) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            (false, false) => day || weekday,
        }
    }

    /// The first fire strictly after `after` (unix seconds), in `tz`; `None` when the
    /// expression never matches (31 February).
    pub fn next_after(&self, tz: Tz, after: i64) -> Option<i64> {
        let start = Utc.timestamp_opt(after, 0).single()?.with_timezone(&tz);
        let mut date = start.date_naive().pred_opt()?;
        for _ in 0..SEARCH_DAYS {
            if self.day_matches(date) {
                let mut best: Option<i64> = None;
                for hour in (0..24).filter(|h| self.hours & (1 << h) != 0) {
                    for minute in (0..60).filter(|m| self.minutes & (1 << m) != 0) {
                        let naive = date.and_hms_opt(hour, minute, 0)?;
                        let every_hour = self.hours == ALL_HOURS;
                        for t in resolve(tz, naive, every_hour) {
                            if t > after {
                                best = Some(best.map_or(t, |b| b.min(t)));
                            }
                        }
                    }
                }
                if best.is_some() {
                    return best;
                }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// The shortest gap between consecutive fires among the next few, in seconds.
    pub fn min_gap(&self, tz: Tz, from: i64) -> Option<i64> {
        let mut prev = self.next_after(tz, from)?;
        let mut gap: Option<i64> = None;
        for _ in 0..GAP_SAMPLES {
            let Some(next) = self.next_after(tz, prev) else {
                break;
            };
            gap = Some(gap.map_or(next - prev, |g| g.min(next - prev)));
            prev = next;
        }
        gap
    }
}

/// A wall time as instants: the first one when it happens twice (both when `both`),
/// shifted past the gap when it does not exist.
fn resolve(tz: Tz, naive: NaiveDateTime, both: bool) -> Vec<i64> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(t) => vec![t.timestamp()],
        LocalResult::Ambiguous(first, second) if both => {
            vec![first.timestamp(), second.timestamp()]
        }
        LocalResult::Ambiguous(first, _) => vec![first.timestamp()],
        LocalResult::None => match tz.from_local_datetime(&(naive + Duration::hours(1))) {
            LocalResult::Single(t) => vec![t.timestamp()],
            LocalResult::Ambiguous(first, _) => vec![first.timestamp()],
            LocalResult::None => Vec::new(),
        },
    }
}

/// Parses the pair and enforces the minimum interval.
pub fn validate(cron: &str, tz: &str) -> Result<(Cron, Tz), CronError> {
    let zone = parse_zone(tz)?;
    let parsed = Cron::parse(cron)?;
    // A fixed reference far from any rule change we care about; the gaps repeat.
    let reference = 1_767_225_600; // 2026-01-01T00:00:00Z
    let gap = parsed.min_gap(zone, reference).ok_or(CronError::Never)?;
    if gap < MIN_INTERVAL_SECS {
        return Err(CronError::TooFrequent { gap_secs: gap });
    }
    Ok((parsed, zone))
}

/// A fixed delay in `0..=cap` seconds, derived from `id` (FNV-1a): the same entry always
/// gets the same one, different entries spread out.
pub fn jitter_secs(id: &str, cap: u64) -> u64 {
    if cap == 0 {
        return 0;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash % (cap + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
            .timestamp()
    }

    fn berlin() -> Tz {
        parse_zone("Europe/Berlin").unwrap()
    }

    #[test]
    fn parses_lists_ranges_and_steps() {
        let c = Cron::parse("0,30 9-11 */10 1,6 1-5").unwrap();
        assert_eq!(c.minutes, (1 << 0) | (1 << 30));
        assert_eq!(c.hours, (1 << 9) | (1 << 10) | (1 << 11));
        assert!(c.days & (1 << 1) != 0 && c.days & (1 << 11) != 0 && c.days & (1 << 2) == 0);
        assert_eq!(c.months, (1 << 1) | (1 << 6));
        assert_eq!(c.weekdays, 0b0111110);
        assert!(Cron::parse("5/20 * * * *").unwrap().minutes & (1 << 45) != 0);
        assert_eq!(
            Cron::parse("0 0 * * 7").unwrap().weekdays,
            Cron::parse("0 0 * * 0").unwrap().weekdays
        );
    }

    #[test]
    fn rejects_bad_expressions() {
        assert_eq!(Cron::parse("* * * *"), Err(CronError::FieldCount));
        assert!(matches!(
            Cron::parse("60 * * * *"),
            Err(CronError::Field {
                field: "minute",
                ..
            })
        ));
        assert!(Cron::parse("*/0 * * * *").is_err());
        assert!(Cron::parse("0 0 0 * *").is_err());
        assert!(Cron::parse("0 0 * 13 *").is_err());
        assert!(Cron::parse("5-1 * * * *").is_err());
        assert!(Cron::parse("a * * * *").is_err());
        assert!(parse_zone("Mars/Olympus").is_err());
    }

    #[test]
    fn next_is_strictly_after() {
        let utc_tz = parse_zone("UTC").unwrap();
        let c = Cron::parse("*/15 * * * *").unwrap();
        let t = utc(2026, 5, 4, 10, 15);
        assert_eq!(c.next_after(utc_tz, t), Some(utc(2026, 5, 4, 10, 30)));
        assert_eq!(c.next_after(utc_tz, t - 1), Some(t));
        assert_eq!(c.next_after(utc_tz, t + 1), Some(utc(2026, 5, 4, 10, 30)));
    }

    #[test]
    fn weekday_and_month_rules() {
        let utc_tz = parse_zone("UTC").unwrap();
        // Mondays at 08:00: 2026-05-04 is a Monday.
        let c = Cron::parse("0 8 * * 1").unwrap();
        assert_eq!(
            c.next_after(utc_tz, utc(2026, 5, 4, 8, 0)),
            Some(utc(2026, 5, 11, 8, 0))
        );
        // Day 13 or Friday.
        let c = Cron::parse("0 0 13 * 5").unwrap();
        assert_eq!(
            c.next_after(utc_tz, utc(2026, 5, 1, 0, 0)),
            Some(utc(2026, 5, 8, 0, 0))
        );
        // 29 February is found across years; 31 February never.
        let c = Cron::parse("0 0 29 2 *").unwrap();
        assert_eq!(
            c.next_after(utc_tz, utc(2026, 1, 1, 0, 0)),
            Some(utc(2028, 2, 29, 0, 0))
        );
        let never = Cron::parse("0 0 31 2 *").unwrap();
        assert_eq!(never.next_after(utc_tz, utc(2026, 1, 1, 0, 0)), None);
    }

    #[test]
    fn spring_forward_moves_a_missing_wall_time_past_the_gap() {
        // Europe/Berlin 2026-03-29: 02:00 CET jumps to 03:00 CEST.
        let c = Cron::parse("30 2 * * *").unwrap();
        let before = utc(2026, 3, 28, 12, 0);
        let day1 = c.next_after(berlin(), before).unwrap();
        assert_eq!(day1, utc(2026, 3, 29, 1, 30)); // 03:30 CEST = 01:30Z
        let day2 = c.next_after(berlin(), day1).unwrap();
        assert_eq!(day2, utc(2026, 3, 30, 0, 30)); // 02:30 CEST = 00:30Z
                                                   // A normal wall time keeps its wall clock across the change.
        let c = Cron::parse("0 3 * * *").unwrap();
        let a = c.next_after(berlin(), utc(2026, 3, 27, 12, 0)).unwrap();
        let b = c.next_after(berlin(), a).unwrap();
        assert_eq!((a, b - a), (utc(2026, 3, 28, 2, 0), 23 * 3600));
    }

    #[test]
    fn fall_back_fires_an_ambiguous_wall_time_once() {
        // Europe/Berlin 2026-10-25: 03:00 CEST falls back to 02:00 CET.
        let c = Cron::parse("30 2 * * *").unwrap();
        let first = c.next_after(berlin(), utc(2026, 10, 24, 12, 0)).unwrap();
        assert_eq!(first, utc(2026, 10, 25, 0, 30)); // 02:30 CEST
        let second = c.next_after(berlin(), first).unwrap();
        assert_eq!(second, utc(2026, 10, 26, 1, 30)); // 02:30 CET, next day
                                                      // Hourly: the repeated hour runs too, so the local day has 25 fires.
        let hourly = Cron::parse("0 * * * *").unwrap();
        let mut t = utc(2026, 10, 24, 22, 0); // 00:00 CEST on the 25th
        let mut count = 0;
        loop {
            t = hourly.next_after(berlin(), t).unwrap();
            if t >= utc(2026, 10, 25, 23, 0) {
                break;
            }
            assert_eq!(t % 3600, 0);
            count += 1;
        }
        // 25 fires on the local day, the first of them (00:00) being the start.
        assert_eq!(count, 24);
    }

    #[test]
    fn validate_enforces_the_minimum_interval() {
        assert!(validate("*/5 * * * *", "UTC").is_ok());
        assert!(validate("0 9 * * 1-5", "Europe/Berlin").is_ok());
        assert!(matches!(
            validate("*/4 * * * *", "UTC"),
            Err(CronError::TooFrequent { gap_secs: 240 })
        ));
        assert!(matches!(
            validate("* * * * *", "UTC"),
            Err(CronError::TooFrequent { gap_secs: 60 })
        ));
        // Two fires three minutes apart inside one hour.
        assert!(validate("0,3 * * * *", "UTC").is_err());
        assert_eq!(validate("0 0 31 2 *", "UTC"), Err(CronError::Never));
        assert!(matches!(
            validate("0 9 * * *", "Nowhere/Land"),
            Err(CronError::Zone(_))
        ));
    }

    #[test]
    fn jitter_is_stable_bounded_and_spread() {
        assert_eq!(jitter_secs("a", 0), 0);
        assert_eq!(jitter_secs("a", 600), jitter_secs("a", 600));
        let ids: Vec<u64> = (0..20)
            .map(|i| jitter_secs(&format!("entry-{i}"), 600))
            .collect();
        assert!(ids.iter().all(|j| *j <= 600));
        assert!(ids.iter().any(|j| *j != ids[0]));
    }
}
