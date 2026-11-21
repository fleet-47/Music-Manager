//! Counts in words, the same on every line the owner reads (the running panel, the last-run lines,
//! the plan lines).

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

/// `1 file`, `2 files`, `2,117 files`.
#[must_use]
pub fn files(n: usize) -> String {
    let noun = if n == 1 { "file" } else { "files" };
    format!("{} {noun}", thousands(n))
}

/// `1 needs attention`, `3 need attention`.
#[must_use]
pub fn need_attention(n: usize) -> String {
    let verb = if n == 1 { "needs" } else { "need" };
    format!("{} {verb} attention", thousands(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_in_words() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2_117), "2,117");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(x_of_y(412, 2_117), "412 of 2,117");
        assert_eq!(files(1), "1 file");
        assert_eq!(files(2_117), "2,117 files");
        assert_eq!(need_attention(1), "1 needs attention");
        assert_eq!(need_attention(1_200), "1,200 need attention");
    }
}
