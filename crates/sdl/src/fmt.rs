//! Plain-text helpers for sizes, ages and time estimates (counts are `sdl_core::words`).

use std::time::Duration;

use sdl_core::LibraryId;

/// A size in decimal units with one decimal: `0 B`, `950 B`, `4.2 GB`.
#[must_use]
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0;
    let mut scale = 1_u64;
    while n >= scale.saturating_mul(1_000) && unit < UNITS.len().saturating_sub(1) {
        scale = scale.saturating_mul(1_000);
        unit = unit.saturating_add(1);
    }
    let name = UNITS.get(unit).copied().unwrap_or("TB");
    if unit == 0 {
        return format!("{n} {name}");
    }
    let whole = n.checked_div(scale).unwrap_or(0);
    let tenth = n
        .checked_rem(scale)
        .and_then(|rest| rest.checked_mul(10))
        .and_then(|rest| rest.checked_div(scale))
        .unwrap_or(0);
    format!("{whole}.{tenth} {name}")
}

/// `library Sam` for another person's library; `None` for the owner's own, which is never named
/// where it would only be noise.
#[must_use]
pub fn library_words(library: &LibraryId) -> Option<String> {
    match library {
        LibraryId::Own => None,
        LibraryId::Other(name) => Some(format!("library {name}")),
    }
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
    fn counts_and_sizes_in_words() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 KB");
        assert_eq!(bytes(4_250_000_000), "4.2 GB");
        assert_eq!(bytes(u64::MAX), "18446744.0 TB");
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
