//! Plain-text wording for counts, shared by the run lines.

/// `2117` as `2,117`.
pub(crate) fn thousands(n: usize) -> String {
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
pub(crate) fn x_of_y(done: usize, total: usize) -> String {
    format!("{} of {}", thousands(done), thousands(total))
}

/// `1 needs attention`, `3 need attention`.
pub(crate) fn need_attention(n: usize) -> String {
    let verb = if n == 1 { "needs" } else { "need" };
    format!("{} {verb} attention", thousands(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(2117), "2,117");
        assert_eq!(x_of_y(412, 2117), "412 of 2,117");
        assert_eq!(need_attention(1), "1 needs attention");
        assert_eq!(need_attention(3), "3 need attention");
    }
}
