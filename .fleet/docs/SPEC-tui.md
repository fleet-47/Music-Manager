# Spec: `sdl`, one Rust TUI for Spotify → library → Snowsky card

Rulings in `.agents/AGENTS.md` bind this spec; where they conflict, AGENTS.md wins. Each feature is
followed by what it is not. Anything not listed is not built: it is a question for Yazid.

Before using any crate below, read its docs at the version in `Cargo.lock` (docs.rs, or the source
under `~/.cargo/registry/src`) and use the calls named here. If a named call does not exist or does
not do what this spec says, stop and report it; do not substitute another crate or hand-write the
capability.

## F1. One screen
- **Main panel**, always visible: one row per function (Sync Spotify, Download link, Sync card), each
  with its current selection and the result of its last run underneath.
- **Running panel**, on the right, visible only while at least one task runs. It opens when a task
  starts and closes when the last one finishes. One block per running task: name, spinner, the current step in a few words (the task's headline), count
  (`412 of 2,117`), bar, time estimate (elapsed ÷ done × remaining), and `N need attention` when N > 0.
  When a task ends, its result becomes that function's "Last run" line in the main panel.
- **Footer**: one line of keys valid right now. `?` opens a help popup listing every key.
- **Popups** (centred, over the screen): function steps (F2–F4), attention list, confirmations, errors, help.
- **Attention list** (`a`): only unmatched and failed items of the selected function's last or
  current run: title, artist, and a one-line plain reason. Scrollable.
- **Errors**: a popup that says what went wrong and what to do, in plain words (e.g. "yt-dlp was not
  found. Start sdl with `mise run tui` so it is on PATH."). Never a raw library error alone.
- **Confirmations**: before stopping a running task (`s`), before quitting while a task runs, and
  before unmounting the card. Quitting with nothing running needs none.
- **Colour** (crate `catppuccin` 2.9.1, feature `ratatui`, flavour Mocha, background painted with its
  `base` so the look does not depend on the terminal theme). Roles, defined once in one `theme`
  module and used everywhere:
  headings and panel titles `mauve` bold; body text `text`; secondary text (hints, "Last run")
  `subtext0`; emphasis (numbers, the selected row's marker, key names in the footer) `peach`;
  borders `surface2`, the focused popup's border `lavender`; selected row background `surface0`;
  status: running `blue`, done `green`, needs attention / unmatched `yellow`, failed `red` (red is
  used for failures only), skipped `overlay1`. When the `NO_COLOR` environment variable is set to a non-empty value, no
  colours or background are applied.
- **Size**: layouts use `Constraint::Min/Fill/Length` and `Flex`; under 60 columns the running panel
  moves below the main panel; under 12 rows only the function rows and the running counts show.
- **Paste**: bracketed paste is enabled (`crossterm` `EnableBracketedPaste`, `Event::Paste`); a pasted
  newline never submits.
- Crates: `ratatui` 0.30 (`Layout`, `Flex`, `Block`, `Paragraph`, `List`, `Gauge`, `Clear`,
  `Scrollbar`), `crossterm`, `tui-input`, `throbber-widgets-tui` (spinner), `catppuccin`.
- **Not:** no tabs, no per-song table, no log pane, no mouse support, no theme setting, no command
  mode, no animations beyond the spinner. Per-song stages are never shown; only counts and the
  attention list.

## F2. Sync Spotify
- Steps: choose the function → sign-in popup (state: opening browser / waiting / signed in / failed)
  → list-selection popup: Liked Songs and each playlist the user owns, with its song count, each
  tickable (Space), remembered from last time → Enter lists the ticked lists' songs and shows
  `N new of M` → Enter starts.
- Sign-in: `rspotify` 0.16 `AuthCodePkceSpotify` with `Config { token_cached: false, .. }`, scopes
  `user-library-read` and `playlist-read-private`, redirect `http://127.0.0.1:9900/`.
  `get_authorize_url`, opened with the `open` crate; the code is received on our own
  `tokio::net::TcpListener` bound to `127.0.0.1:9900`, which keeps accepting until a request
  carries `code=` (browsers open speculative connections), answers with a short "you can close this
  tab" page, and is dropped on success, failure or cancel so the port is always freed; the code is
  extracted with rspotify's `parse_response_code`, then `request_token`. A redirect carrying
  `error=` ends sign-in as failed with that reason.
- Listing: playlist names and counts from `current_user_playlists` (owner = the signed-in user,
  `me()`); Liked Songs count from the first `current_user_saved_tracks` page. Songs of ticked lists
  with rspotify's paginators at 50 per page. Local files and episodes are skipped.
- Per new song: match (F5) → download to a temp file (F6) → look up and tag the temp file (F7) →
  rename into `<music_root>/Spotify/<list name>/<artists> - <title>.opus` (Liked Songs: list name
  `Saved tracks`). A file that already exists at that path is skipped before any network call.
- A 429 ends the task; the error popup shows the wait from `Retry-After` (seconds or an HTTP date,
  parsed with the `httpdate` crate), or "wait unknown" when absent. A 403 shows Spotify's reason
  from the response body.
- **Not:** no client secret, no token on disk, no retry layer, no followed playlists, no deleting of
  local songs, no per-song Spotify calls (track, artist, album endpoints are never called).

## F3. Download link
- Steps: choose the function → popup with one text field → Enter starts.
- Listing: `yt-dlp --dump-single-json --flat-playlist -- <url>`; each item then downloads as in F6
  into `<music_root>/<Site>/` (the item's own yt-dlp extractor key, e.g. `Youtube`, `Soundcloud`, never
  the playlist wrapper's such as `YoutubeTab`), a playlist into a subfolder named after
  it. Existing files are skipped; a failed item is counted and the rest continue.
- **Not:** no Spotify links here (they are F2), no MusicBrainz lookup, no format choice.

## F4. Sync card
- Steps: choose the function → mount (`diskutil mount <card_uuid>` via `tokio::process::Command`,
  args passed separately) → popup `N files to copy, M already on the card` → Enter starts (Esc here unmounts the card again and ends the task) → when
  done, popup "Clean up and unmount?" → remove `.Spotlight-V100`, `.Trashes`, `.fseventsd` from the
  card root, `diskutil unmount <card_uuid>`.
- Copy: `walkdir` over `<music_root>`; a file is copied to `<card>/Music/<same relative path>` when
  missing there or different in size, with `File::create` + `io::copy`, to a `*.part` name first and
  renamed when complete. Files and directories whose name starts with a dot are never copied.
- Stopping finishes the file being copied, then stops.
- **Not:** not two-way, never deletes from the card, no rclone, no DiskArbitration, no formatting.

## F5. Matching (crate `sdl-youtube`)
- Search: `innertube-rs` 0.12 `Innertube::search_music(query, Some(MusicSearchFilter::Songs))`,
  reading `MusicSearchResults.songs` (`MusicTrackItem`: `video_id`, `title`, `artists[].name`,
  `album.title`, `duration_ms`). One client per run. Query the ISRC first; if no candidate is
  accepted, query `<first artist> <title>`.
- Rule, a pure function with unit tests: among the first 5 songs, score each and take the best that
  passes; none passing → Unmatched. A candidate passes when its duration is within 3 s of Spotify's,
  its title matches, and at least one artist matches. Text is compared after lower-casing, removing
  punctuation, and removing bracketed parts that name a feature or version (`feat`, `ft`, `with`,
  `remaster`, `version`, `edit`) from both sides; two texts match when `strsim::jaro_winkler` ≥ 0.9
  or one contains the other. Score = title similarity + best artist similarity + 0.5 when the album
  titles match. The tests include "Get Lucky" against "Get Lucky (feat. Pharrell Williams and Nile
  Rodgers)".
- An item with no result, or a search error, becomes an attention item with the reason.
- **Not:** no yt-dlp search, no plain-YouTube video fallback, no fingerprinting, no manual pick.

## F6. Download (crate `sdl-youtube`)
- Run the `yt-dlp` binary with `tokio::process::Command`: `.stdin(Stdio::null())`,
  `.kill_on_drop(true)`, stdout and stderr piped. Arguments for one item:
  `-f bestaudio[acodec=opus]/bestaudio -x --audio-format opus --no-playlist --no-progress
  --ffmpeg-location <dir of ffmpeg on PATH, only if yt-dlp's docs require it> -o <temp path> -- <url>`
  where the URL for a match is `https://music.youtube.com/watch?v=<video_id>`. Confirm in yt-dlp's
  documentation that an Opus source is copied, not re-encoded, with these flags.
- Each download gets its own temp directory, created with the `tempfile` crate inside
  `<music_root>/.sdl-tmp/` (same volume, so the final rename is atomic); `-o` names a file in it that
  does not exist yet, so yt-dlp never sees an existing output. The directory is removed on success,
  failure or cancel. `.sdl-tmp` is removed when empty.
- `ErrorKind::NotFound` on spawn becomes "yt-dlp was not found on PATH". A non-zero exit reports the
  last `ERROR:` line of stderr.
- **Not:** not the `yt-dlp` wrapper crate, no self-installed binaries, no re-encoding, no cookies.

## F7. Genre, totals and tags (crate `sdl-library`)
- `musicbrainz_rs` 0.14 with features `async`, `rate_limit`, `rustls`; one process-wide
  `MusicBrainzClient` with User-Agent `sdl/<version>` (no contact details).
- Per downloaded song with an ISRC: recording search `isrc:<code>`, then a recording lookup with
  genres, artists, releases, release groups and media. Genre = the recording's top genre, else its
  release group's, else its first artist's. Track total = the track count of the medium whose
  position equals the song's disc number, on the release whose title matches the album. Disc total
  = that release's medium count, from one release lookup per release ID, cached for the run.
- `lofty` 0.25: one write to the temp `.opus` file: title, artists, album, album artist, track
  number and total, disc number and total, date, ISRC, genre, and the cover (fetched with `reqwest`
  from Spotify's cover URL) as a front-cover picture.
- A failed or empty lookup leaves those tags empty and the song still completes. A failed tag write
  fails the song and the temp file is removed.
- **Not:** no Last.fm, no Spotify genre, no lyrics, no ReplayGain, no retagging of existing files.

## F8. Tasks and state (crate `sdl`)
- Each function runs as one task; different functions can run at the same time, the same function
  not twice. Every event carries its task. The UI keeps per task only: counts (total, done, added,
  skipped, unmatched, failed), the current headline, and the attention items.
- Stopping aborts the task (`JoinHandle::abort`; child processes die through `kill_on_drop`) and
  the task's last-run line says "stopped".
- `~/.config/sdl/state.toml` (written with `serde` + `toml`): the ticked Spotify lists and each
  function's last-run line with its time.
- Song downloads run `download_jobs` at a time (`futures` `buffer_unordered`).
- **Not:** no database, no resume of half-finished files, no history beyond the last run, no queue
  of pending functions.

## Config: `~/.config/sdl/config.toml`
`client_id`, `music_root` (default `~/Music`, a leading `~` is expanded), `card_uuid`,
`download_jobs` (default 4, 0 is an error). A missing or malformed file is a plain error naming the
path, printed before the screen starts. `config.example.toml` at the repo root.

## Paths
Every path component is made FAT-safe (`sanitize-filename`), has leading dots stripped, is capped so
the file name fits 255 bytes, and falls back to the Spotify or video ID when it would be empty.

## Workspace
Cargo workspace at the repo root, edition 2024, `resolver = "3"`. Crates: `crates/sdl-core`
(shared types, config, state file, path rules), `crates/sdl-spotify`, `crates/sdl-youtube`,
`crates/sdl-library` (MusicBrainz, tags, card), `crates/sdl` (binary: tasks and TUI). Library crates
depend only on `sdl-core`. Errors: `thiserror` in libraries, `color-eyre` in the binary.

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
- Tests cover pure logic only (matching, path rules, config and state parsing, task-state
  transitions, card copy rule against temp directories). No invented API fixtures.

## mise
`.mise/config.toml` holds the tools (yt-dlp, ffmpeg, deno) and one task, `tui`, that runs
`target/release/sdl` with them on `PATH`.
