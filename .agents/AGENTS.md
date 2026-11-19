# Spotify-DL

`sdl`: one Rust TUI that builds a local music library from Yazid's Spotify Liked Songs and own
playlists (audio from YouTube Music), downloads single links, and copies the library to the Snowsky
SD card. Build spec: `.fleet/docs/SPEC-tui.md`.

## Rulings
- Scope that matters: Liked Songs and Yazid's private playlists. Followed playlists are not a requirement.
- Sync adds new tracks and never deletes.
- One Rust TUI is the whole product (ruled 2026-11-18). Every backend is a pure-Rust crate (`ratatui`, `rspotify`, `musicbrainz_rs`, `lofty`, `walkdir`), with one exception: downloads run the yt-dlp binary, called directly with tokio's process API (ruled 2026-11-19; the `yt-dlp` wrapper crate is not used, because it leaves the child's stdin open to the TUI's keyboard and does not kill children on exit). Reason for the exception: no pure-Rust crate generates YouTube's anti-bot tokens, so pure-Rust extraction depends on a workaround YouTube can close. Matching does not use yt-dlp: YouTube Music is searched with `innertube-rs` (`search_music`, songs filter), which returns title, artists, album, duration and video ID in one request. No linked bindings (no `librclone`, no ffmpeg libraries).
- A file enters the library only when complete (ruled 2026-11-19): download to a temp file, look up, tag the temp file, rename last.
- UI (ruled 2026-11-19): one screen. A main panel organised by function and the selection made for each (Spotify lists are ticked before a sync), and a running panel that opens while any task runs and closes when none does. Per-song work is a backend concern: only counts and an on-request attention list (unmatched, failed) surface. No tabs, no per-song table. Colourful by role: headings, body, secondary text, emphasis and status each have their own colour (spec F1).
- Spotify is asked only for the paged listing (about one call per 50 songs). Reason: per-song metadata calls through the developer app hit the dev-mode limit (24 h lockout) on the 2,117-song Liked Songs run; Spotify's batch endpoints were removed in February 2026. Genre and totals come from MusicBrainz, because Spotify's listing carries no genre and its artist `genres` field is deprecated.
- Sign-in is PKCE (ruled 2026-11-18): the TUI uses only the client ID, which is not a secret and lives in its config file; the client secret is never given to it. Tokens stay in memory for one run and are never written to disk. Only the scopes `user-library-read` and `playlist-read-private` are requested. fnox and Bitwarden are not used.
- A Spotify 429 ends the run and shows the wait; there is no retry layer.
- Card sync is one-way and add-only. Mount and unmount use macOS `diskutil` (DiskArbitration bindings need `unsafe`, which the build standard forbids).
- No dot-directory may ever reach the card: Disco's scanner (`disco-v3.6.1/src/ui/scanner.c:1607-1621` in the Snowsky-DAC repo) disables pruning of vanished songs when it meets any dot-directory except root `.diskos`. The TUI strips leading dots from every path component and removes `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the card before unmount.
- Paths: Spotify songs in `<music_root>/Spotify/<list name>/` (Liked Songs in `Saved tracks`); link downloads in `<music_root>/<Site>/` using yt-dlp's extractor name, with a subfolder per playlist.
- Build standard is `fleet-47/Gen.v2`: fenix stable, the global `~/.cargo/config.toml`, and the lint flags in the spec. The repo carries no `.cargo/`, `clippy.toml`, `[lints]` table, rustflags or profile overrides.
- mise config lives in `.mise/config.toml` and is committed. mise supplies yt-dlp, ffmpeg and deno and starts the TUI.

## Layout
- `Cargo.toml`, `crates/`: the workspace. `sdl-core` (shared types, config, path rules), `sdl-spotify`, `sdl-youtube`, `sdl-library` (genre, tags, card), `sdl` (binary: TUI and pipeline).
- `config.example.toml`: copy to `~/.config/sdl/config.toml`.
- `.mise/config.toml`: tools and the `tui` task.

## Run
- Build: `cargo build --release --locked` with the fenix stable toolchain.
- Start: `mise run tui` (runs `target/release/sdl`).

## Facts
- Audio is sourced from YouTube Music, not Spotify. Lossy quality is accepted; files are Opus as served, not re-encoded.
- Spotify app redirect URI: `http://127.0.0.1:9900/`. Each run opens a browser sign-in.
- Spotify dev-mode apps lock out for about 24 h after too many calls.
- If downloads break after YouTube changes, the fix arrives via a new yt-dlp release (`mise upgrade`).
- The flow before the TUI (mise tasks, spotDL, fnox, rclone bisync) is in jj history: change `nplromuk`.
