//! A small five-field cron reader, only to refuse a schedule that fires more often than every
//! five minutes before the daemon sees it. The daemon runs its own reader for the schedule.

const MIN_GAP_MINUTES: u32 = 5;

/// One field as the sorted list of values it allows.
fn field(text: &str, lo: u32, hi: u32, name: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (
                r,
                s.parse::<u32>()
                    .ok()
                    .filter(|s| *s > 0)
                    .ok_or_else(|| format!("bad step in {name}"))?,
            ),
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            (num(a, lo, hi, name)?, num(b, lo, hi, name)?)
        } else {
            let n = num(range, lo, hi, name)?;
            // `5/10` means from 5 to the end.
            (n, if part.contains('/') { hi } else { n })
        };
        if from > to {
            return Err(format!("bad range in {name}"));
        }
        let mut v = from;
        while v <= to {
            out.push(v);
            v += step;
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn num(text: &str, lo: u32, hi: u32, name: &str) -> Result<u32, String> {
    let n: u32 = text
        .parse()
        .map_err(|_| format!("{name} must be numbers, `*`, ranges, lists or steps"))?;
    if n < lo || n > hi {
        return Err(format!("{name} is outside {lo}-{hi}"));
    }
    Ok(n)
}

/// Checks the expression and returns the smallest gap, in minutes, between two firings.
pub fn min_gap_minutes(expr: &str) -> Result<u32, String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err("cron needs five fields: minute hour day-of-month month day-of-week".into());
    }
    let minutes = field(parts[0], 0, 59, "minute")?;
    let hours = field(parts[1], 0, 23, "hour")?;
    let doms = field(parts[2], 1, 31, "day-of-month")?;
    let months = field(parts[3], 1, 12, "month")?;
    let mut dows = field(parts[4], 0, 7, "day-of-week")?;
    for d in &mut dows {
        *d %= 7;
    }
    dows.sort_unstable();
    dows.dedup();
    // The daemon's reader (`sushiai-core` cron) treats a field as "any" when its text starts
    // with `*`, not when it happens to list every value: `1-31` restricts the day of month.
    // The same rule here keeps a refused or accepted gap equal to what the daemon schedules.
    let dom_any = parts[2].starts_with('*');
    let dow_any = parts[4].starts_with('*');
    let mut times: Vec<u32> = Vec::new();
    // A leap year covers every date once; day-of-week repeats are covered by one week offset.
    for day in 0..366u32 {
        let date = chrono::NaiveDate::from_yo_opt(2024, day + 1).ok_or("calendar")?;
        use chrono::Datelike;
        let in_dom = doms.contains(&date.day());
        let in_dow = dows.contains(&date.weekday().num_days_from_sunday());
        let day_ok = match (dom_any, dow_any) {
            (true, true) => true,
            (true, false) => in_dow,
            (false, true) => in_dom,
            (false, false) => in_dom || in_dow,
        };
        if !day_ok || !months.contains(&date.month()) {
            continue;
        }
        for h in &hours {
            for m in &minutes {
                times.push(day * 1440 + h * 60 + m);
            }
        }
    }
    if times.is_empty() {
        return Err("cron never fires".into());
    }
    let gap = times
        .windows(2)
        .map(|w| w[1] - w[0])
        .min()
        // One firing in the year: far apart.
        .unwrap_or(366 * 1440);
    Ok(gap)
}

/// `Ok` when the expression is valid and never fires faster than every five minutes.
pub fn check(expr: &str) -> Result<(), String> {
    let gap = min_gap_minutes(expr)?;
    if gap < MIN_GAP_MINUTES {
        return Err(format!(
            "cron fires every {gap} min; the smallest allowed gap is {MIN_GAP_MINUTES} min"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps() {
        assert_eq!(min_gap_minutes("0 9 * * 1-5").unwrap(), 1440);
        assert_eq!(min_gap_minutes("*/5 * * * *").unwrap(), 5);
        assert_eq!(min_gap_minutes("* * * * *").unwrap(), 1);
        assert_eq!(min_gap_minutes("0,3 * * * *").unwrap(), 3);
        assert!(check("*/4 * * * *").is_err());
        assert!(check("*/5 * * * *").is_ok());
        assert!(check("0 9 * *").is_err());
        assert!(check("61 9 * * *").is_err());
    }

    #[test]
    fn day_fields_follow_the_daemon_rule_that_a_leading_star_means_any() {
        // `1-31` restricts the day of month, so with a weekday the two are OR-ed: every day.
        assert_eq!(min_gap_minutes("0 9 1-31 * 1").unwrap(), 1440);
        // A literal `*` day with a weekday: Mondays only.
        assert_eq!(min_gap_minutes("0 9 * * 1").unwrap(), 7 * 1440);
        // `*/2` starts with `*`: the day of month is "any", so only the weekday counts.
        assert_eq!(min_gap_minutes("0 9 */2 * 1").unwrap(), 7 * 1440);
    }
}
