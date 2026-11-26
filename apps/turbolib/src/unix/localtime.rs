//! Local time, as C's `localtime` gives it to `ar tv` (both tools print
//! members' dates in local time): the zone `TZ` names (a POSIX TZ string,
//! or a file under `/usr/share/zoneinfo`, or `:path`), else
//! `/etc/localtime`. Zone files are TZif (RFC 8536): their transitions, and
//! past the last, the POSIX TZ string in their footer. Without a zone, or
//! on a system without these files, UTC.

/// Seconds east of UTC at `t` (seconds since 1970), in the zone `tz` (the
/// `TZ` variable's value, if set) names.
pub fn offset_at(t: i64, tz: Option<&str>) -> i64 {
    let zone = match tz {
        None => std::fs::read("/etc/localtime").ok(),
        Some(tz) => {
            let name = tz.strip_prefix(':').unwrap_or(tz);
            if name.is_empty() {
                None
            } else if name.starts_with('/') {
                std::fs::read(name).ok()
            } else {
                // A zone file, or else a POSIX TZ string.
                std::fs::read(format!("/usr/share/zoneinfo/{name}"))
                    .ok()
                    .or_else(|| Some(name.as_bytes().to_vec()))
            }
        }
    };
    match zone {
        Some(bytes) if bytes.starts_with(b"TZif") => tzif(&bytes, t).unwrap_or(0),
        Some(bytes) => std::str::from_utf8(&bytes)
            .ok()
            .and_then(|r| Rule::parse(r.trim()))
            .map_or(0, |r| r.offset_at(t)),
        None => 0,
    }
}

fn be32(b: &[u8], at: usize) -> Option<i64> {
    Some(i64::from(i32::from_be_bytes(
        b.get(at..at + 4)?.try_into().ok()?,
    )))
}

fn be64(b: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A TZif file's offset at `t`.
fn tzif(b: &[u8], t: i64) -> Option<i64> {
    let count = |at: usize| be32(b, at).and_then(|v| usize::try_from(v).ok());
    let header = |at: usize| -> Option<[usize; 6]> {
        Some([
            count(at + 20)?,
            count(at + 24)?,
            count(at + 28)?,
            count(at + 32)?,
            count(at + 36)?,
            count(at + 40)?,
        ])
    };
    let version = *b.get(4)?;
    let [isut, isstd, leap, times, types, chars] = header(0)?;
    let v1_len = times * 4 + times + types * 6 + chars + leap * 8 + isstd + isut;
    let (start, wide, counts) = if version >= b'2' {
        let second = 44 + v1_len;
        if b.get(second..second + 4) != Some(b"TZif") {
            return None;
        }
        (second + 44, true, header(second)?)
    } else {
        (44, false, [isut, isstd, leap, times, types, chars])
    };
    let [isut, isstd, leap, times, types, chars] = counts;
    let w = if wide { 8 } else { 4 };
    let at_time = |i: usize| {
        if wide {
            be64(b, start + i * 8)
        } else {
            be32(b, start + i * 4)
        }
    };
    let index_at = start + times * w;
    let types_at = index_at + times;
    let ttinfo = |k: usize| be32(b, types_at + k * 6);
    let end = types_at + types * 6 + chars + leap * (w + 4) + isstd + isut;
    // Past the last transition: the footer's rule, if any.
    if wide && times > 0 && t >= at_time(times - 1)? {
        let footer = b.get(end..)?;
        if let Some(text) = std::str::from_utf8(footer).ok().map(str::trim)
            && !text.is_empty()
            && let Some(rule) = Rule::parse(text)
        {
            return Some(rule.offset_at(t));
        }
    }
    if times == 0 || t < at_time(0)? {
        return ttinfo(0);
    }
    // The last transition at or before t.
    let (mut lo, mut hi) = (0, times);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if at_time(mid)? <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let k = usize::from(*b.get(index_at + lo)?);
    ttinfo(k)
}

/// When a POSIX TZ rule's daylight saving time starts or ends.
#[derive(Clone, Copy, Debug)]
enum Day {
    /// `Mm.w.d`: day `d` (0 Sunday) of week `w` (5: the last) of month `m`.
    Month(u32, u32, u32),
    /// `Jn`: day `n` (1 to 365), February 29 never counted.
    Julian(u32),
    /// `n`: day `n` (0 to 365), February 29 counted.
    Zero(u32),
}

/// A POSIX TZ rule.
#[derive(Clone, Copy, Debug)]
struct Rule {
    /// Seconds east of UTC, standard time.
    std: i64,
    /// Daylight saving time: its offset, and when it starts and ends (each
    /// a day and a local time in seconds).
    dst: Option<(i64, Day, i64, Day, i64)>,
}

/// `hh[:mm[:ss]]`, with an optional sign: seconds.
fn hms(s: &str) -> Option<(i64, &str)> {
    let (sign, s) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let mut total = 0i64;
    let mut rest = s;
    for (i, scale) in [3600, 60, 1].into_iter().enumerate() {
        if i > 0 {
            match rest.strip_prefix(':') {
                Some(r) => rest = r,
                None => break,
            }
        }
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        total += rest[..n].parse::<i64>().ok()? * scale;
        rest = &rest[n..];
    }
    Some((sign * total, rest))
}

/// A zone name: letters, or `<...>`.
fn zone_name(s: &str) -> Option<&str> {
    if let Some(r) = s.strip_prefix('<') {
        let end = r.find('>')?;
        return Some(&r[end + 1..]);
    }
    let n = s.bytes().take_while(u8::is_ascii_alphabetic).count();
    (n >= 3).then(|| &s[n..])
}

/// A decimal number at the start of `s`, and the rest.
fn number(s: &str) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    Some((s.get(..n)?.parse().ok()?, &s[n..]))
}

fn day(s: &str) -> Option<(Day, &str)> {
    if let Some(r) = s.strip_prefix('M') {
        let (m, r) = number(r)?;
        let (w, r) = number(r.strip_prefix('.')?)?;
        let (d, r) = number(r.strip_prefix('.')?)?;
        return ((1..=12).contains(&m) && (1..=5).contains(&w) && d <= 6)
            .then_some((Day::Month(m, w, d), r));
    }
    if let Some(r) = s.strip_prefix('J') {
        let (n, r) = number(r)?;
        return (1..=365).contains(&n).then_some((Day::Julian(n), r));
    }
    let (n, r) = number(s)?;
    (n <= 365).then_some((Day::Zero(n), r))
}

fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days from 1970-01-01 to `y`-`m`-`d`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The year `t` (seconds since 1970) is in.
fn year_of(t: i64) -> i64 {
    let days = t.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(m <= 2)
}

impl Day {
    /// Days from 1970 to this day of year `y`.
    fn in_year(self, y: i64) -> i64 {
        let jan1 = days_from_civil(y, 1, 1);
        match self {
            Day::Zero(n) => jan1 + i64::from(n),
            Day::Julian(n) => {
                let n = i64::from(n) - 1;
                jan1 + n + i64::from(leap(y) && n >= 59)
            }
            Day::Month(m, w, d) => {
                let m = i64::from(m);
                let first = days_from_civil(y, m, 1);
                // 1970-01-01 was a Thursday (4).
                let wday = (first + 4).rem_euclid(7);
                let mut day = first + (i64::from(d) - wday).rem_euclid(7) + (i64::from(w) - 1) * 7;
                let next = if m == 12 {
                    days_from_civil(y + 1, 1, 1)
                } else {
                    days_from_civil(y, m + 1, 1)
                };
                while day >= next {
                    day -= 7;
                }
                day
            }
        }
    }
}

impl Rule {
    fn parse(s: &str) -> Option<Rule> {
        let r = zone_name(s)?;
        let (west, r) = hms(r)?;
        let std = -west;
        if r.is_empty() {
            return Some(Rule { std, dst: None });
        }
        let r = zone_name(r)?;
        let (dst_offset, r) = match hms(r) {
            Some((west, rest)) => (-west, rest),
            None => (std + 3600, r),
        };
        let (start, start_time, end, end_time) = match r.strip_prefix(',') {
            Some(r) => {
                let (start, r) = day(r)?;
                let (start_time, r) = match r.strip_prefix('/') {
                    Some(r) => hms(r)?,
                    None => (7200, r),
                };
                let (end, r) = day(r.strip_prefix(',')?)?;
                let (end_time, _) = match r.strip_prefix('/') {
                    Some(r) => hms(r)?,
                    None => (7200, r),
                };
                (start, start_time, end, end_time)
            }
            // The US rules, as glibc assumes without them.
            None => (Day::Month(3, 2, 0), 7200, Day::Month(11, 1, 0), 7200),
        };
        Some(Rule {
            std,
            dst: Some((dst_offset, start, start_time, end, end_time)),
        })
    }

    fn offset_at(&self, t: i64) -> i64 {
        let Some((dst, start, start_time, end, end_time)) = self.dst else {
            return self.std;
        };
        let y = year_of(t + self.std);
        // The transitions in UTC: the start in standard time, the end in
        // daylight saving time.
        let on = start.in_year(y) * 86_400 + start_time - self.std;
        let off = end.in_year(y) * 86_400 + end_time - dst;
        let in_dst = if on < off {
            t >= on && t < off
        } else {
            // The southern hemisphere: daylight saving time across new year.
            !(t >= off && t < on)
        };
        if in_dst { dst } else { self.std }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_rules() {
        let la = Rule::parse("PST8PDT,M3.2.0,M11.1.0").unwrap();
        // 2026-07-01 12:00 UTC: PDT.
        assert_eq!(la.offset_at(1_782_907_200), -7 * 3600);
        // 2026-11-26 09:00 UTC: PST.
        assert_eq!(la.offset_at(1_795_683_600), -8 * 3600);
        // 2026-03-08 09:59:59 and 10:00 UTC (2:00 PST): the change.
        assert_eq!(la.offset_at(1_772_963_999), -8 * 3600);
        assert_eq!(la.offset_at(1_772_964_000), -7 * 3600);
        let utc = Rule::parse("UTC0").unwrap();
        assert_eq!(utc.offset_at(0), 0);
        let india = Rule::parse("IST-5:30").unwrap();
        assert_eq!(india.offset_at(0), 5 * 3600 + 1800);
        // Sydney: daylight saving time from October to April.
        let syd = Rule::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(syd.offset_at(1_767_225_600), 11 * 3600);
        assert_eq!(syd.offset_at(1_782_907_200), 10 * 3600);
        assert!(Rule::parse("X1").is_none());
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(year_of(-1), 1969);
        assert_eq!(year_of(1_795_683_600), 2026);
        assert_eq!(
            Day::Month(3, 2, 0).in_year(2026),
            days_from_civil(2026, 3, 8)
        );
        assert_eq!(
            Day::Month(2, 5, 6).in_year(2025),
            days_from_civil(2025, 2, 22)
        );
        assert_eq!(Day::Julian(60).in_year(2024), days_from_civil(2024, 3, 1));
    }
}
