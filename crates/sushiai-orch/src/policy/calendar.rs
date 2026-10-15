const DAY_MS: i64 = 86_400_000;

pub(crate) fn day(ts: i64) -> String {
    // Days since the epoch -> civil date (proleptic Gregorian, UTC).
    let z = ts.div_euclid(DAY_MS) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the epoch of a `YYYY-MM-DD` calendar date (the inverse of
/// [`day`]); an impossible date such as 2026-02-30 is an error.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn parse_day(text: &str) -> Result<i64, String> {
    let bad = || format!("{text:?} is not a UTC date YYYY-MM-DD");
    let b = text.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return Err(bad());
    }
    let num = |r: std::ops::Range<usize>| text[r].parse::<i64>().map_err(|_| bad());
    let days = days_from_civil(num(0..4)?, num(5..7)?, num(8..10)?);
    // Out-of-range months and days do not survive the round trip.
    if day(days * DAY_MS) != text {
        return Err(bad());
    }
    Ok(days)
}
