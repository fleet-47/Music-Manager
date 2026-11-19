# Spec: `sdl`, one Rust TUI for Spotify → library → Snowsky card

Rulings in `.agents/AGENTS.md` bind this spec; where they conflict, AGENTS.md wins. Nothing beyond
this spec is built: a status, guard, fallback or feature not listed here is a question for Yazid.

## What it does
1. **Sync Spotify:** sign in, list Liked Songs and the user's own playlists, and for every song not
   already on disk: match on YouTube Music, download, look up genre, write tags. Adds, never deletes.
2. **Download link:** one pasted link (track or playlist, any site yt-dlp supports) into the library.
3. **Sync card:** copy the library to the Snowsky SD card, add-only, then clean up and unmount.

## Backends (pure Rust except yt-dlp)
| Job | Crate | Rules |
|---|---|---|
| Spotify | `rspotify` (PKCE, `AuthCodePkceSpotify`) | Client ID from the config file. Scopes: `user-library-read`, `playlist-read-private` only. Token in memory only (`token_cached = false`). Sign-in: open the authorize URL in the browser (`open` crate) and receive the code on rspotify's own local listener (`get_authcode_listener`) at `http://127.0.0.1:9900/`, run inside `spawn_blocking`; never call `prompt_for_token` or anything that prints or reads stdin. Liked Songs at 50 per page. Playlists: only those the user owns. No retry layer: an HTTP 429 ends the run and the UI shows the `Retry-After` wait. |
| Search, download | `yt-dlp` crate (wraps the yt-dlp binary) | Uses the `yt-dlp` and `ffmpeg` found on `PATH` (mise supplies them); the crate must not download its own binaries. Audio is the best Opus stream, extracted without re-encoding, as `.opus`. Download to a temp file outside the library, then rename into place. |
| Matching | small function, `strsim` | Search YouTube Music by ISRC first; if nothing, by `artist title`. Accept a candidate only when its duration is within a few seconds of Spotify's and artist/title are similar. Otherwise the song is `Unmatched` and is not downloaded. This is the only non-glue logic in the app. |
| Genre, disc total | `musicbrainz_rs` (`rate_limit` feature) | Search recording by `isrc:`, then look up with genres, releases and media. Generic User-Agent (app name and version, no personal contact). Missing data leaves the tag empty; it never fails the song. |
| Tags, cover | `lofty` | One write per file after the genre lookup: title, artists, album, album artist, track and disc number and totals, date, ISRC, genre, cover (downloaded with `reqwest`). |
| Card | `walkdir`, macOS `diskutil` via `std::process::Command` | Mount by volume UUID (`diskutil mount <uuid>`); DiskArbitration bindings are not used because their API is `unsafe` and the build standard forbids unsafe code. Copy a file when it is missing on the card or its size differs; never delete. Copy with `File::create` + `io::copy`. Then remove `.Spotlight-V100`, `.Trashes`, `.fseventsd` from the card root and unmount (`diskutil unmount <uuid>`) after the user confirms. No rclone. |
| TUI | `ratatui`, `crossterm`, `tui-input`, `tokio` | One app state; worker tasks send events over a tokio channel. |
| Config, errors | `serde` + `toml`, `directories`, `color-eyre` (binary), `thiserror` (libraries) | |

## Paths
- Spotify songs: `<music_root>/Spotify/<list name>/<artists> - <title>.opus`; Liked Songs use the
  list name `Saved tracks`. A file that exists at that path is skipped before any network call.
- Link downloads: `<music_root>/<Site>/` using yt-dlp's extractor name; a playlist link gets a
  subfolder named after the playlist.
- Card: `<music_root>/` mirrors to `<card>/Music/`.
- Every path component is made FAT-safe (`sanitize-filename`) and has leading dots stripped, so no
  dot-directory can ever reach the card.

## Config: `~/.config/sdl/config.toml`
`client_id`, `music_root` (default `~/Music`), `card_uuid`, `download_jobs` (default 4). A missing
or malformed file is a visible error naming the path. `config.example.toml` at the repo root.

## Screens
- **Home:** Sync Spotify, Download link (text input), Sync card, Quit.
- **Run:** table of songs (title, artist, list, stage, status), filter by status (all / failed /
  unmatched), overall progress gauge, current-activity line. Sign-in state and a 429 wait are shown here.
No other screens.

## Workspace
Cargo workspace at the repo root, edition 2024, `resolver = "3"`. Crates: `crates/sdl-core`
(shared types: `Track`, `Stage`, progress `Event`, `Config`, path rules), `crates/sdl-spotify`,
`crates/sdl-youtube`, `crates/sdl-library` (genre, tags, card), `crates/sdl` (binary: TUI and the
per-song pipeline with bounded concurrency). Library crates do not depend on each other, only on
`sdl-core`.

## Build standard (`fleet-47/Gen.v2`)
- Toolchain: fenix stable. The global `~/.cargo/config.toml` supplies profiles, rustflags and the
  clippy config. The repo carries no `.cargo/`, `clippy.toml`, `rustfmt.toml`, `[lints]` table,
  rustflags or profile overrides.
- Gate, all with `--workspace --all-targets --all-features --locked`: `cargo fmt --all`,
  `cargo check`, `cargo clippy -- -D warnings -D clippy::all -D clippy::pedantic
  -D clippy::disallowed_methods -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
  -D clippy::arithmetic_side_effects -D clippy::cast_possible_truncation -D clippy::cast_possible_wrap
  -D clippy::cast_sign_loss -F unsafe_code -D clippy::allow_attributes
  -D clippy::allow_attributes_without_reason`, `cargo build`, `cargo test`.
- No `#[allow]` or `#[expect]` anywhere. Tests return `Result` and use `?`.
- Tests cover pure logic only (matching, path rules, config parsing). No invented API fixtures.

## mise
`.mise/config.toml` holds the tools (yt-dlp, ffmpeg, deno) and one task, `tui`, that runs the built
binary with them on `PATH`.
