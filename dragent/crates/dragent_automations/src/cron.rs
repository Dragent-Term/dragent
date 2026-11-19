//! Five-field cron schedules: `minute hour day-of-month month day-of-week`.
//!
//! Each field accepts `*`, a number, a range `a-b`, a step (`*/n`, `a-b/n`,
//! `a/n`), or a comma list of those. Months and weekdays also accept
//! three-letter names, and Sunday may be written 0 or 7. When both day
//! fields are restricted, a day matches if either does (classic cron).
//!
//! [`Repeat`] is the structured form the schedule picker edits; any
//! expression it cannot represent stays a custom expression.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    allowed: Vec<bool>,
    /// Written as `*` (matters for the day-of-month/day-of-week rule).
    any: bool,
}

impl Field {
    fn contains(&self, value: u32) -> bool {
        self.allowed.get(value as usize).copied().unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    minutes: Field,
    hours: Field,
    days: Field,
    months: Field,
    weekdays: Field,
    source: String,
}

const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAY_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn parse_value(raw: &str, names: Option<(&[&str], u32)>) -> Result<u32, String> {
    if let Ok(value) = raw.parse::<u32>() {
        return Ok(value);
    }
    if let Some((names, first)) = names {
        let lower = raw.to_ascii_lowercase();
        if let Some(index) = names.iter().position(|name| *name == lower) {
            return Ok(index as u32 + first);
        }
    }
    Err(format!("“{raw}” isn't a number"))
}

fn parse_field(
    raw: &str,
    min: u32,
    max: u32,
    label: &str,
    names: Option<(&[&str], u32)>,
) -> Result<Field, String> {
    let mut allowed = vec![false; max as usize + 1];
    let any = raw == "*";
    for part in raw.split(',') {
        if part.is_empty() {
            return Err(format!("empty value in the {label} field"));
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| format!("“{step}” isn't a valid {label} step"))?;
                if step == 0 {
                    return Err(format!("the {label} step can't be zero"));
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (parse_value(a, names)?, parse_value(b, names)?)
        } else {
            let start = parse_value(range, names)?;
            // `a/n` runs from `a` to the end of the field.
            (start, if part.contains('/') { max } else { start })
        };
        if start < min || end > max || start > end {
            return Err(format!("{label} values must be within {min}-{max}"));
        }
        let mut value = start;
        while value <= end {
            allowed[value as usize] = true;
            value += step;
        }
    }
    Ok(Field { allowed, any })
}

impl Schedule {
    pub fn parse(expression: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err("use five fields: minute, hour, day of month, month, day of week".into());
        }
        let mut weekdays = parse_field(fields[4], 0, 7, "day-of-week", Some((&WEEKDAY_NAMES, 0)))?;
        // 7 is another name for Sunday.
        if weekdays.allowed[7] {
            weekdays.allowed[0] = true;
        }
        weekdays.allowed.truncate(7);
        Ok(Self {
            minutes: parse_field(fields[0], 0, 59, "minute", None)?,
            hours: parse_field(fields[1], 0, 23, "hour", None)?,
            days: parse_field(fields[2], 1, 31, "day-of-month", None)?,
            months: parse_field(fields[3], 1, 12, "month", Some((&MONTH_NAMES, 1)))?,
            weekdays,
            source: fields.join(" "),
        })
    }

    pub fn expression(&self) -> &str {
        &self.source
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom = self.days.contains(date.day());
        let dow = self
            .weekdays
            .contains(date.weekday().num_days_from_sunday());
        match (self.days.any, self.weekdays.any) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }

    /// The first matching minute strictly after `after`, in `after`'s zone.
    /// A local time skipped by a clock change runs at the next valid minute;
    /// a repeated one runs once, at its first occurrence.
    pub fn next_after<Tz: TimeZone>(&self, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        let zone = after.timezone();
        let local = after.naive_local();
        let mut candidate = local
            .with_second(0)?
            .with_nanosecond(0)?
            .checked_add_signed(Duration::minutes(1))?;
        // Five years of searching covers every satisfiable expression
        // (e.g. 29 February on a given weekday).
        let limit = local.checked_add_signed(Duration::days(366 * 5))?;
        while candidate <= limit {
            let date = candidate.date();
            if !self.months.contains(date.month()) {
                let (year, month) = if date.month() == 12 {
                    (date.year() + 1, 1)
                } else {
                    (date.year(), date.month() + 1)
                };
                candidate = NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.day_matches(date) {
                candidate = date.succ_opt()?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.hours.contains(candidate.hour()) {
                candidate = next_hour(candidate)?;
                continue;
            }
            if !self.minutes.contains(candidate.minute()) {
                candidate = candidate.checked_add_signed(Duration::minutes(1))?;
                continue;
            }
            match zone.from_local_datetime(&candidate) {
                chrono::LocalResult::Single(at) => return Some(at),
                chrono::LocalResult::Ambiguous(first, _) => {
                    if first > *after {
                        return Some(first);
                    }
                }
                // Inside a spring-forward gap: try the next minute.
                chrono::LocalResult::None => {}
            }
            candidate = candidate.checked_add_signed(Duration::minutes(1))?;
        }
        None
    }

    /// Plain-English summary, e.g. “Every Monday at 9:00 AM”.
    pub fn describe(&self) -> String {
        match Repeat::from_expression(&self.source) {
            Some(repeat) => repeat.describe(),
            None => self
                .describe_common()
                .unwrap_or_else(|| format!("Custom schedule ({})", self.source)),
        }
    }

    /// Common shapes the picker can't make: several days of the week,
    /// several times a day, or every few minutes or hours. Day-of-month and
    /// month restrictions stay custom.
    fn describe_common(&self) -> Option<String> {
        if !self.days.any || !self.months.any {
            return None;
        }
        let days = Some(set(&self.weekdays))
            .filter(|days| days.len() < 7)
            .unwrap_or_default();
        let (every_day, on_days) = match days.as_slice() {
            [] => ("Every day".to_string(), String::new()),
            [1, 2, 3, 4, 5] => ("Weekdays".to_string(), " on weekdays".to_string()),
            [0, 6] => ("Weekends".to_string(), " on weekends".to_string()),
            _ => {
                let names = and_list(
                    days.iter()
                        .map(|day| WEEKDAY_LABELS[*day as usize].to_string()),
                );
                (format!("Every {names}"), format!(" on {names}"))
            }
        };
        let fields: Vec<&str> = self.source.split(' ').collect();
        let step = |field: &str| field.strip_prefix("*/").and_then(|n| n.parse::<u32>().ok());
        if let Some(minutes) = step(fields[0]) {
            return self
                .hours
                .any
                .then(|| format!("{}{on_days}", every_n(minutes, "minute")));
        }
        let minute = single(&self.minutes)?;
        if let Some(hours) = step(fields[1]) {
            return Some(format!(
                "{} at :{minute:02}{on_days}",
                every_n(hours, "hour")
            ));
        }
        let hours = set(&self.hours);
        if hours.len() == 24 {
            return Some(format!("Every hour at :{minute:02}{on_days}"));
        }
        if hours.len() >= 3 && hours.windows(2).all(|pair| pair[1] == pair[0] + 1) {
            return Some(format!(
                "Every hour from {} to {}{on_days}",
                clock_label(hours[0], minute),
                clock_label(hours[hours.len() - 1], minute)
            ));
        }
        if hours.len() > 4 {
            return None;
        }
        let times = and_list(hours.iter().map(|hour| clock_label(*hour, minute)));
        Some(format!("{every_day} at {times}"))
    }
}

/// The values a field allows, in order.
fn set(field: &Field) -> Vec<u32> {
    (0..field.allowed.len() as u32)
        .filter(|value| field.contains(*value))
        .collect()
}

fn single(field: &Field) -> Option<u32> {
    match set(field).as_slice() {
        [value] => Some(*value),
        _ => None,
    }
}

/// `Every 15 minutes`, `Every hour`.
fn every_n(n: u32, unit: &str) -> String {
    if n == 1 {
        format!("Every {unit}")
    } else {
        format!("Every {n} {unit}s")
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = items.collect();
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

fn next_hour(at: NaiveDateTime) -> Option<NaiveDateTime> {
    at.date()
        .and_hms_opt(at.hour(), 0, 0)?
        .checked_add_signed(Duration::hours(1))
}

/// When a planned occurrence is due and when it goes stale (the following
/// occurrence), so a late run is skipped instead of piling up.
pub fn next_window<Tz: TimeZone>(
    schedule: &Schedule,
    after: &DateTime<Tz>,
) -> Option<(DateTime<Tz>, Option<DateTime<Tz>>)> {
    let due = schedule.next_after(after)?;
    let stale = schedule.next_after(&due);
    Some((due, stale))
}

// ── Picker model ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Period {
    pub const ALL: [Period; 6] = [
        Self::Minute,
        Self::Hour,
        Self::Day,
        Self::Week,
        Self::Month,
        Self::Year,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

pub const WEEKDAY_LABELS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
pub const MONTH_LABELS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A schedule the picker can show: one repetition period plus the fields
/// that period uses. Fields a period ignores are kept, so switching
/// periods back and forth preserves what the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    pub period: Period,
    pub minute: u32,
    pub hour: u32,
    /// 0 = Sunday.
    pub weekday: u32,
    pub day: u32,
    pub month: u32,
}

impl Default for Repeat {
    fn default() -> Self {
        Self {
            period: Period::Day,
            minute: 0,
            hour: 9,
            weekday: 1,
            day: 1,
            month: 1,
        }
    }
}

fn plain_number(raw: &str, max: u32) -> Option<u32> {
    raw.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| raw.parse::<u32>().ok())
        .flatten()
        .filter(|value| *value <= max)
}

impl Repeat {
    pub fn from_expression(expression: &str) -> Option<Self> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return None;
        };
        let base = Self::default();
        if fields.iter().all(|field| *field == "*") {
            return Some(Self {
                period: Period::Minute,
                ..base
            });
        }
        let minute = plain_number(minute, 59)?;
        if [*hour, *day, *month, *weekday] == ["*"; 4] {
            return Some(Self {
                period: Period::Hour,
                minute,
                ..base
            });
        }
        let hour = plain_number(hour, 23)?;
        match (*day, *month, *weekday) {
            ("*", "*", "*") => Some(Self {
                period: Period::Day,
                minute,
                hour,
                ..base
            }),
            ("*", "*", weekday) => Some(Self {
                period: Period::Week,
                minute,
                hour,
                weekday: plain_number(weekday, 7)? % 7,
                ..base
            }),
            (day, "*", "*") => Some(Self {
                period: Period::Month,
                minute,
                hour,
                day: plain_number(day, 31).filter(|day| *day >= 1)?,
                ..base
            }),
            (day, month, "*") => Some(Self {
                period: Period::Year,
                minute,
                hour,
                day: plain_number(day, 31).filter(|day| *day >= 1)?,
                month: plain_number(month, 12).filter(|month| *month >= 1)?,
                ..base
            }),
            _ => None,
        }
    }

    pub fn expression(&self) -> String {
        let Self {
            minute,
            hour,
            weekday,
            day,
            month,
            ..
        } = *self;
        match self.period {
            Period::Minute => "* * * * *".into(),
            Period::Hour => format!("{minute} * * * *"),
            Period::Day => format!("{minute} {hour} * * *"),
            Period::Week => format!("{minute} {hour} * * {weekday}"),
            Period::Month => format!("{minute} {hour} {day} * *"),
            Period::Year => format!("{minute} {hour} {day} {month} *"),
        }
    }

    pub fn describe(&self) -> String {
        let time = clock_label(self.hour, self.minute);
        match self.period {
            Period::Minute => "Every minute".into(),
            Period::Hour => format!("Every hour at :{:02}", self.minute),
            Period::Day => format!("Every day at {time}"),
            Period::Week => format!(
                "Every {} at {time}",
                WEEKDAY_LABELS[self.weekday as usize % 7]
            ),
            Period::Month => format!("Monthly on the {} at {time}", ordinal(self.day)),
            Period::Year => format!(
                "Every year on {} {} at {time}",
                MONTH_LABELS[(self.month.max(1) - 1) as usize % 12],
                self.day
            ),
        }
    }
}

/// `9:05 AM`, `12:00 PM`.
pub fn clock_label(hour: u32, minute: u32) -> String {
    let (display, suffix) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{display}:{minute:02} {suffix}")
}

/// `1st`, `2nd`, `11th`, `23rd`.
pub fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, Utc};

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn next(expression: &str, after: &str) -> String {
        Schedule::parse(expression)
            .unwrap()
            .next_after(&at(after))
            .unwrap()
            .to_rfc3339()
    }

    #[test]
    fn rejects_malformed_expressions() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
        ] {
            assert!(Schedule::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn finds_the_next_matching_minute() {
        // Daily at 09:00.
        assert_eq!(
            next("0 9 * * *", "2026-10-01T08:59:30Z"),
            "2026-10-01T09:00:00+00:00"
        );
        assert_eq!(
            next("0 9 * * *", "2026-10-01T09:00:00Z"),
            "2026-10-02T09:00:00+00:00"
        );
        // Every 15 minutes.
        assert_eq!(
            next("*/15 * * * *", "2026-10-01T10:07:00Z"),
            "2026-10-01T10:15:00+00:00"
        );
        // Mondays at 10:00 (2026-10-01 is a Thursday).
        assert_eq!(
            next("0 10 * * 1", "2026-10-01T12:00:00Z"),
            "2026-10-05T10:00:00+00:00"
        );
        assert_eq!(
            next("0 10 * * mon", "2026-10-01T12:00:00Z"),
            "2026-10-05T10:00:00+00:00"
        );
        // Sunday as 7.
        assert_eq!(
            next("0 8 * * 7", "2026-10-01T12:00:00Z"),
            "2026-10-04T08:00:00+00:00"
        );
        // Month rollover and year rollover.
        assert_eq!(
            next("30 6 1 * *", "2026-10-15T00:00:00Z"),
            "2026-11-01T06:30:00+00:00"
        );
        assert_eq!(
            next("0 0 1 jan *", "2026-10-15T00:00:00Z"),
            "2027-01-01T00:00:00+00:00"
        );
        // Weekdays only.
        assert_eq!(
            next("0 9 * * 1-5", "2026-10-02T10:00:00Z"),
            "2026-10-05T09:00:00+00:00"
        );
        // Leap day.
        assert_eq!(
            next("0 12 29 2 *", "2026-10-01T00:00:00Z"),
            "2028-02-29T12:00:00+00:00"
        );
    }

    #[test]
    fn restricted_day_fields_match_either() {
        // The 13th, or any Friday: Friday 2 October comes first.
        assert_eq!(
            next("0 0 13 * 5", "2026-10-01T00:00:00Z"),
            "2026-10-02T00:00:00+00:00"
        );
    }

    #[test]
    fn evaluates_in_the_given_zone() {
        let zone = FixedOffset::east_opt(3600).unwrap();
        let after = zone.with_ymd_and_hms(2026, 10, 1, 8, 30, 0).unwrap();
        let due = Schedule::parse("0 9 * * *")
            .unwrap()
            .next_after(&after)
            .unwrap();
        assert_eq!(due.to_rfc3339(), "2026-10-01T09:00:00+01:00");
    }

    #[test]
    fn impossible_dates_have_no_occurrence() {
        assert!(Schedule::parse("0 0 31 2 *")
            .unwrap()
            .next_after(&at("2026-01-01T00:00:00Z"))
            .is_none());
    }

    #[test]
    fn spring_forward_skips_the_missing_time() {
        // America/New_York springs forward on 2026-03-08: 02:00 EST jumps to
        // 03:00 EDT, so 02:30 never happens that day. A daily 02:30 run skips
        // the day and runs next on 2026-03-09 02:30 EDT (= 06:30 UTC).
        use chrono::TimeZone;
        let zone = chrono_tz::America::New_York;
        let schedule = Schedule::parse("30 2 * * *").unwrap();
        let after = zone.with_ymd_and_hms(2026, 3, 7, 2, 31, 0).unwrap();
        let next = schedule.next_after(&after).unwrap();
        assert_eq!(
            next.to_rfc3339(),
            "2026-03-09T02:30:00-04:00",
            "spring-forward day has no 02:30, so it runs the next day"
        );
        // From just before the gap, it still skips the missing time.
        let before_gap = zone.with_ymd_and_hms(2026, 3, 8, 0, 0, 0).unwrap();
        let next = schedule.next_after(&before_gap).unwrap();
        assert_eq!(next.to_rfc3339(), "2026-03-09T02:30:00-04:00");
    }

    #[test]
    fn fall_back_runs_the_repeated_time_once() {
        // Australia/Sydney falls back on 2026-04-05: 03:00 AEDT returns to
        // 02:00 AEST, so 02:30 happens twice. It runs once, at the first
        // 02:30 (still on daylight time).
        use chrono::TimeZone;
        let zone = chrono_tz::Australia::Sydney;
        let schedule = Schedule::parse("30 2 * * *").unwrap();
        let before = zone.with_ymd_and_hms(2026, 4, 4, 12, 0, 0).unwrap();
        let first = schedule.next_after(&before).unwrap();
        assert_eq!(
            first.to_rfc3339(),
            "2026-04-05T02:30:00+11:00",
            "repeated times run once, at the first occurrence"
        );
        // After the first occurrence, the next daily run is the following
        // day, not the second 02:30 on the same wall-clock day.
        let second = schedule.next_after(&first).unwrap();
        assert_eq!(second.to_rfc3339(), "2026-04-06T02:30:00+10:00");
    }

    #[test]
    fn window_ends_at_the_following_occurrence() {
        let schedule = Schedule::parse("0 9 * * *").unwrap();
        let (due, stale) = next_window(&schedule, &at("2026-10-01T08:00:00Z")).unwrap();
        assert_eq!(due.to_rfc3339(), "2026-10-01T09:00:00+00:00");
        assert_eq!(stale.unwrap().to_rfc3339(), "2026-10-02T09:00:00+00:00");
    }

    #[test]
    fn picker_round_trips_its_periods() {
        for expression in [
            "* * * * *",
            "15 * * * *",
            "30 9 * * *",
            "0 10 * * 1",
            "0 8 1 * *",
            "45 23 31 12 *",
        ] {
            let repeat = Repeat::from_expression(expression).unwrap();
            assert_eq!(repeat.expression(), expression);
        }
        for custom in ["*/15 * * * *", "0 9 * * 1-5", "0 9,17 * * *", "0 9 13 * 5"] {
            assert!(Repeat::from_expression(custom).is_none(), "{custom}");
        }
        // Switching periods keeps the time.
        let weekly = Repeat {
            period: Period::Week,
            ..Repeat::from_expression("30 7 * * *").unwrap()
        };
        assert_eq!(weekly.expression(), "30 7 * * 1");
    }

    #[test]
    fn descriptions_read_naturally() {
        let describe = |e: &str| Schedule::parse(e).unwrap().describe();
        assert_eq!(describe("* * * * *"), "Every minute");
        assert_eq!(describe("5 * * * *"), "Every hour at :05");
        assert_eq!(describe("0 9 * * *"), "Every day at 9:00 AM");
        assert_eq!(describe("30 14 * * 3"), "Every Wednesday at 2:30 PM");
        assert_eq!(describe("0 0 1 * *"), "Monthly on the 1st at 12:00 AM");
        assert_eq!(
            describe("0 12 25 12 *"),
            "Every year on December 25 at 12:00 PM"
        );
        assert_eq!(describe("*/10 * * * *"), "Every 10 minutes");
        assert_eq!(describe("0 9 * * 1-5"), "Weekdays at 9:00 AM");
        assert_eq!(describe("30 10 * * sat,sun"), "Weekends at 10:30 AM");
        assert_eq!(
            describe("0 9 * * 1,3,5"),
            "Every Monday, Wednesday and Friday at 9:00 AM"
        );
        assert_eq!(describe("0 9,17 * * *"), "Every day at 9:00 AM and 5:00 PM");
        assert_eq!(
            describe("0 9,17 * * 1-5"),
            "Weekdays at 9:00 AM and 5:00 PM"
        );
        assert_eq!(describe("0 */2 * * *"), "Every 2 hours at :00");
        assert_eq!(describe("*/15 * * * 1-5"), "Every 15 minutes on weekdays");
        assert_eq!(describe("0 * * * 1-5"), "Every hour at :00 on weekdays");
        assert_eq!(
            describe("0 9-17 * * 1-5"),
            "Every hour from 9:00 AM to 5:00 PM on weekdays"
        );
        // Anything else keeps its expression rather than guessing.
        assert_eq!(describe("0 9 13 * 5"), "Custom schedule (0 9 13 * 5)");
        assert_eq!(
            describe("*/15 9-17 * * *"),
            "Custom schedule (*/15 9-17 * * *)"
        );
        assert_eq!(describe("5,35 9 * * *"), "Custom schedule (5,35 9 * * *)");
        assert_eq!(ordinal(22), "22nd");
        assert_eq!(ordinal(13), "13th");
    }
}
