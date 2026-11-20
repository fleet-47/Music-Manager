//! Ranking a list against typed text, with `nucleo-matcher`.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

/// Indices of `labels` that match `query`, best match first; equal scores keep list order. An
/// empty or blank `query` matches everything in list order. Each word of the query must match
/// (fuzzy, smart case); characters such as `!`, `^` and `'` are plain text.
#[must_use]
pub(crate) fn rank(labels: &[String], query: &str) -> Vec<usize> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut matcher = Matcher::new(Config::DEFAULT);
    pattern
        .match_list(labels.iter().enumerate().map(Indexed), &mut matcher)
        .into_iter()
        .map(|(Indexed((index, _)), _)| index)
        .collect()
}

/// A label that remembers its position, so ranking can return positions.
struct Indexed<'a>((usize, &'a String));

impl AsRef<str> for Indexed<'_> {
    fn as_ref(&self) -> &str {
        self.0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Vec<String> {
        [
            "Daft Punk",
            "Punk Rock Radio",
            "Pink Floyd",
            "Ünder Pressure",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn blank_query_keeps_every_item_in_order() {
        assert_eq!(rank(&labels(), ""), vec![0, 1, 2, 3]);
        assert_eq!(rank(&labels(), "   "), vec![0, 1, 2, 3]);
    }

    #[test]
    fn typing_filters_and_ranks() {
        let ranked = rank(&labels(), "punk");
        assert!(ranked.contains(&0) && ranked.contains(&1));
        assert!(!ranked.contains(&2));
        let two = ["b r i d g e".to_owned(), "Bridge".to_owned()];
        assert_eq!(rank(&two, "bridge"), vec![1, 0]);
        let tied = ["Bridge".to_owned(), "bridge".to_owned()];
        assert_eq!(rank(&tied, "bridge"), vec![0, 1]);
    }

    #[test]
    fn special_characters_are_plain_text_and_accents_fold() {
        assert_eq!(rank(&labels(), "!"), Vec::<usize>::new());
        assert_eq!(rank(&labels(), "under"), vec![3]);
    }
}
