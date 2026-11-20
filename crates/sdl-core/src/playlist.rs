//! The playlist file model (F7): a pointer file `<music_root>/<List name>.m3u8`.
//!
//! UTF-8, no BOM, `#EXTM3U`, `#PLAYLIST:<name>`, then one line per song in list order: a path
//! relative to the playlist file, with forward slashes. A playlist never says where a song's file
//! lives; the library layout does ([`crate::Place`]).

use std::path::{Path, PathBuf};

use crate::Format;
use crate::place::component;

/// File extension of a playlist file.
pub const PLAYLIST_EXTENSION: &str = "m3u8";

/// A playlist: its name and its songs' paths in list order.
///
/// Invariant: every entry is relative (no leading `/`), and `render` writes it with `/`
/// separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// The list's name, as shown on the `#PLAYLIST:` line.
    pub name: String,
    /// One path per song, relative to the playlist file, in list order.
    pub entries: Vec<PathBuf>,
}

impl Playlist {
    /// The playlist's file name, `<name>.m3u8`, with the name made safe like a path component;
    /// `fallback_id` (the Spotify list ID) when it would be empty.
    #[must_use]
    pub fn file_name(name: &str, fallback_id: &str) -> String {
        let mut stem = component(name);
        if stem.is_empty() {
            stem = component(fallback_id);
        }
        format!("{stem}.{PLAYLIST_EXTENSION}")
    }

    /// The file's text: `#EXTM3U`, `#PLAYLIST:<name>`, then one line per entry, each ending in a
    /// newline. A newline inside the name becomes a space.
    #[must_use]
    pub fn render(&self) -> String {
        let name = self.name.replace(['\r', '\n'], " ");
        let mut text = format!("#EXTM3U\n#PLAYLIST:{name}\n");
        for entry in &self.entries {
            let line: Vec<String> = entry
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            text.push_str(&line.join("/"));
            text.push('\n');
        }
        text
    }

    /// Read a playlist file's text. The name is the `#PLAYLIST:` line, else `fallback_name`. Blank
    /// lines and other `#` lines are skipped; a leading BOM is tolerated.
    #[must_use]
    pub fn parse(text: &str, fallback_name: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut name = None;
        let mut entries = Vec::new();
        for line in text.lines().map(str::trim) {
            if let Some(title) = line.strip_prefix("#PLAYLIST:") {
                name = Some(title.trim().to_owned());
            } else if !line.is_empty() && !line.starts_with('#') {
                entries.push(PathBuf::from(line));
            }
        }
        Self {
            name: name.unwrap_or_else(|| fallback_name.to_owned()),
            entries,
        }
    }

    /// Correct the lines that name the song at `relative_stem` (`<artist>/<album>/<stem>`, no
    /// extension) so they end in `format`'s extension. Returns whether any line changed.
    pub fn correct(&mut self, relative_stem: &Path, format: Format) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.with_extension("") == relative_stem {
                let corrected = entry.with_extension(format.extension());
                if corrected != *entry {
                    *entry = corrected;
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Playlist {
        Playlist {
            name: "Road trip".into(),
            entries: vec![
                PathBuf::from("A/Al/one.opus"),
                PathBuf::from("B/Singles/two.mp3"),
            ],
        }
    }

    #[test]
    fn render_is_extm3u_playlist_then_lines() {
        assert_eq!(
            list().render(),
            "#EXTM3U\n#PLAYLIST:Road trip\nA/Al/one.opus\nB/Singles/two.mp3\n"
        );
        assert!(!list().render().starts_with('\u{feff}'));
    }

    #[test]
    fn parse_reads_what_render_wrote() {
        assert_eq!(Playlist::parse(&list().render(), "x"), list());
    }

    #[test]
    fn parse_uses_the_fallback_name_and_skips_comments() {
        let parsed = Playlist::parse("\u{feff}#EXTM3U\n# note\n\nA/b/c.mp3\r\n", "Fallback");
        assert_eq!(parsed.name, "Fallback");
        assert_eq!(parsed.entries, [PathBuf::from("A/b/c.mp3")]);
    }

    #[test]
    fn file_name_is_safe_with_an_id_fallback() {
        assert_eq!(Playlist::file_name("My: List", "id1"), "My List.m3u8");
        assert_eq!(Playlist::file_name("...", "id1"), "id1.m3u8");
        assert_eq!(
            Playlist::file_name("Liked Songs", "liked-songs"),
            "Liked Songs.m3u8"
        );
    }

    #[test]
    fn correct_changes_only_the_named_song() {
        let mut playlist = list();
        assert!(playlist.correct(Path::new("A/Al/one"), Format::Mp3));
        assert_eq!(playlist.entries[0], PathBuf::from("A/Al/one.mp3"));
        assert_eq!(playlist.entries[1], PathBuf::from("B/Singles/two.mp3"));
        assert!(!playlist.correct(Path::new("A/Al/one"), Format::Mp3));
        assert!(!playlist.correct(Path::new("nope"), Format::Opus));
    }

    #[test]
    fn a_newline_in_the_name_cannot_break_the_file() {
        let mut playlist = list();
        playlist.name = "a\nb".into();
        assert!(playlist.render().contains("#PLAYLIST:a b\n"));
    }
}
