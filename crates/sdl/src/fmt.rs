//! Plain-text helpers for numbers, ages and time estimates.

use std::time::Duration;

/// `2117` as `2,117`.
#[must_use]
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len().saturating_add(digits.len() / 3));
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// `412 of 2,117`.
#[must_use]
pub fn x_of_y(done: usize, total: usize) -> String {
    format!("{} of {}", thousands(done), thousands(total))
}

/// How long ago `at` was, relative to `now` (both seconds since the Unix epoch).
#[must_use]
pub fn ago(now: u64, at: u64) -> String {
    let secs = now.saturating_sub(at);
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3_600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        format!("{} h ago", secs / 3_600)
    } else {
        format!("{} d ago", secs / 86_400)
    }
}

/// The time-estimate line for a running task; `left` is `None` until something is done.
#[must_use]
pub fn estimate_text(left: Option<Duration>) -> String {
    match left {
        None => "estimating time left".to_owned(),
        Some(left) => format!("{} left", duration_text(left)),
    }
}

/// A rough duration in words: `under a minute`, `about 3 min`, `about 2 h 5 min`.
#[must_use]
pub fn duration_text(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        "under a minute".to_owned()
    } else if secs < 3_600 {
        format!("about {} min", secs / 60)
    } else {
        format!("about {} h {} min", secs / 3_600, (secs % 3_600) / 60)
    }
}

/// Whole percent of `done` in `total`, 0 to 100.
#[must_use]
pub fn percent(done: usize, total: usize) -> u16 {
    let pct = u128::try_from(done.min(total))
        .ok()
        .and_then(|d| d.checked_mul(100))
        .and_then(|n| n.checked_div(u128::try_from(total).ok()?))
        .unwrap_or(0);
    u16::try_from(pct.min(100)).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2_117), "2,117");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(x_of_y(412, 2_117), "412 of 2,117");
    }

    #[test]
    fn estimate_and_age_wording() {
        assert_eq!(estimate_text(None), "estimating time left");
        assert_eq!(
            estimate_text(Some(Duration::from_mins(5))),
            "about 5 min left"
        );
        assert_eq!(duration_text(Duration::from_mins(125)), "about 2 h 5 min");
        assert_eq!(ago(100, 90), "just now");
        assert_eq!(ago(1_000, 400), "10 min ago");
        assert_eq!(ago(100_000, 1), "1 d ago");
    }

    #[test]
    fn percent_is_clamped() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(9, 4), 100);
    }
}
