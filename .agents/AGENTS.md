# Spotify-DL

`sdl`: one Rust TUI that builds a local music library from Yazid's Spotify Liked Songs and own
playlists (audio from YouTube Music), downloads single links, and copies the library to the Snowsky
SD card. Build spec: `.fleet/docs/SPEC-tui.md`.

## Rulings
- Scope that matters: Liked Songs and Yazid's private playlists. Followed playlists are not a requirement.
- Sync adds new tracks and never deletes.
- One Rust TUI is the whole product (ruled 2026-11-18). Every backend is a pure-Rust crate (`ratatui`, `rspotify`, `musicbrainz_rs`, `lofty`, `walkdir`), with one exception: downloads run the yt-dlp binary, called directly with tokio's process API (ruled 2026-11-19; the `yt-dlp` wrapper crate is not used, because it leaves the child's stdin open to the TUI's keyboard and does not kill children on exit). Reason for the exception: no pure-Rust crate generates YouTube's anti-bot tokens, so pure-Rust extraction depends on a workaround YouTube can close. Matching does not use yt-dlp: YouTube Music is searched with `innertube-rs` (`search_music`, songs filter), which returns title, artists, album, duration and video ID in one request. No linked bindings (no `librclone`, no ffmpeg libraries).
- A file enters the library only when complete (ruled 2026-11-19): download to a temp file, look up, tag the temp file, rename last.
- UI (ruled 2026-11-19): one screen, laid out as the home screen ruled 2026-11-21 below. Per-song work is a backend concern: only counts and an on-request attention list (unmatched, failed) surface. No tabs, no per-song table. Colourful by role: headings, body, secondary text, emphasis and status each have their own colour (spec F1).
- Spotify is asked only for the paged listing (about one call per 50 songs). Reason: per-song metadata calls through the developer app hit the dev-mode limit (24 h lockout) on the 2,117-song Liked Songs run; Spotify's batch endpoints were removed in February 2026. Genre and totals come from MusicBrainz, because Spotify's listing carries no genre and its artist `genres` field is deprecated.
- Sign-in is PKCE (ruled 2026-11-18): the TUI uses only the client ID, which is not a secret and lives in its config file; the client secret is never given to it. Sign-in is remembered (ruled 2026-11-20, replaces "tokens in memory only"): the refresh token is stored in the macOS Keychain, encrypted at rest, and nowhere else; the browser opens only on first use or when the token stops working. The access token stays in memory. Only the scopes `user-library-read` and `playlist-read-private` are requested. fnox and Bitwarden are not used.
- A Spotify 429 ends the run and shows the wait; there is no retry layer.
- Device sync (ruled 2026-11-19) works with any mounted external or removable drive chosen in the UI, in either direction (to the device, or from it into the library), always add-only, with ticked folders and an optional format for the device (as is, MP3, M4A) remembered per device. No device is named in the config. Drives are found and ejected with macOS `diskutil` (DiskArbitration bindings need `unsafe`, which the build standard forbids).
- Library standard (ruled 2026-11-20, replaces the 2026-11-19 format choice at download): every song enters the library as Opus from YouTube Music, found by metadata, whatever the source of that metadata (Spotify listing, a pasted link). The link's own audio is used only when no match is found. MP3 and M4A are made only when music leaves the library (device sync, Convert). A song already present is skipped.
- Convert (ruled 2026-11-19) writes converted copies to a separate destination and never changes the library. Conversion runs the `ffmpeg` binary directly: the second wrapped tool beside yt-dlp, allowed for conversion only, because no pure-Rust MP3 encoder exists.
- No dot-directory may ever reach a device: Disco's scanner (`disco-v3.6.1/src/ui/scanner.c:1607-1621` in the Snowsky-DAC repo) disables pruning of vanished songs when it meets any dot-directory except root `.diskos`. The TUI strips leading dots from every path component and removes `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the card at the end of every device sync.
- Eject clean-up (ruled 2026-11-20) never deletes a file a player may own: a `._X` file is removed only when `sdl` wrote `X` in that sync and the file starts with the AppleDouble signature. `._` files from earlier copies are a one-time manual task (`.fleet/docs/BACKLOG.md`).
- Playlists (ruled 2026-11-20) are pointer files in a playlist format the Disc reads (`.m3u8`; Disco matches entries by path, then file name: `ui/musicdb.c` in the Snowsky-DAC repo). A playlist never dictates where a song's file lives.
- Redraw (ruled 2026-11-20) is event-driven: nothing is drawn while nothing changes, progress updates on progress events, and a timer runs only while an animation, a running task's spinner or the eject countdown is on screen. No fixed polling tick.
- Interaction (ruled 2026-11-20): animation and reactive components are wanted, modelled on Charm's suite. Mouse input is allowed as a second way to do what the keyboard already does; every action stays reachable by keyboard.
- Toolchain (ruled 2026-11-20): fenix nightly is acceptable if a chosen dependency needs it.
- `charmed_rust` licence (Yazid's reading, 2026-11-20): the rider withholds rights from OpenAI and Anthropic as companies; it does not stop Yazid, or an agent working for Yazid, from using the crates.
- Uncertain matches are held, never guessed (ruled 2026-11-20): a song with no confident YouTube Music match is not downloaded. It waits in a review list where Yazid sees the possible matches and picks one, or chooses to use the link's own audio when the song came from a link.
- Library layout (ruled 2026-11-20): `<music_root>/<Artist>/<Album>/<Title>`, one file per song. This replaces the per-source and per-list folders.
- Devices are assumed to read `.m3u8` playlist files (ruled 2026-11-20); the device form says so. No folder-per-playlist arrangement.
- No Reorganise function (ruled 2026-11-20, replaces the same-day approval): the files from the old flow (MP3s under `Spotify/`, `Youtube/`, `Soundcloud/`) are kept as they are, the songs are downloaded again as Opus into the new layout, and the old files are deleted by hand once the new approach is proven (`.fleet/docs/BACKLOG.md`).
- Workflow runs may keep the Mac awake with `caffeinate` (approved 2026-11-20).
- Link downloads (ruled 2026-11-20): only `music.youtube.com` links download directly. Every other link, plain YouTube included, is matched against YouTube Music; with no match it is held for review, where the link's own audio can be chosen.
- Disc playlist copies are accepted (ruled 2026-11-20): Disco imports a changed playlist file as "<name> (2)" and keeps the old one (`import_m3u_file` in `ui/musicdb.c`). `sdl` cannot prevent it; the device form says so (`.fleet/docs/BACKLOG.md`).
- Eject clean-up list (confirmed 2026-11-20): `.Spotlight-V100`, `.Trashes` (this empties the device's Trash; the device form says so), `.fseventsd` and `.TemporaryItems` at the device root, every `.DS_Store`, and the narrow `._` rule.
- The old-flow folders were moved out of the library to `~/Music-old` (2026-11-20), so the library starts clean.
- yt-dlp's JavaScript engine stays `deno` (ruled 2026-11-20). Reason: yt-dlp runs YouTube's script in it with no file system or network access; QuickJS is 1.4 MB against 82 MB but has no such restriction, and Bun is larger and deprecated by yt-dlp (yt-dlp wiki, EJS page).
- Fidelity before uniformity (ruled 2026-11-21): nothing is re-encoded on the way into the library. A song with no Opus stream on YouTube Music is kept as served (usually M4A).
- A cover that cannot be downloaded does not fail the song (ruled 2026-11-21); the last-run line counts songs without a cover.
- Mounted disk images are listed as devices, labelled "(disk image)" (ruled 2026-11-21).
- A Spotify rate limit is recorded as "stopped" with the wait shown (ruled 2026-11-21).
- ffmpeg stays an outside tool and is not compiled in (confirmed 2026-11-21).
- How `sdl` is installed and started (a global command, tools in mise's global config) is decided after the full live run, which is the final gate before a release build (ruled 2026-11-21).
- Running tasks show a spinner beside their name for the whole run (ruled 2026-11-21, accepting the redraw 10 times a second while a task runs; nothing is drawn while idle).
- Download link keeps its short refusal of Spotify links; no advice about copying playlists is added (ruled 2026-11-21).
- Libraries (ruled 2026-11-21): music for Yazid and music downloaded for friends never mix. Each person has a library, a folder under `music_root` (`~/Music/Me/…`, `~/Music/<Friend>/…`), with its own playlists and review entries. Download forms have a "For me" toggle; Sync device and Convert choose a library, remembered per device (spec F12).
- Home screen (ruled 2026-11-21, replaces the main panel and running panel of 2026-11-19): large cards, one per function, with no headings; a card shows only what helps now, and its content gives way to its progress while it runs. No text that carries no information (no "not run yet"); a sync says when it last ran, a download only while fresh (spec F1).
- Tools check (ruled 2026-11-21): the app checks for yt-dlp, ffmpeg and deno at start and offers to install missing ones with mise (spec F13).
- Several links at once (ruled 2026-11-22): Download link takes any number of links separated by spaces, line breaks or commas, and queues each as its own download in one run (spec F5).
- Keyboard focus is unmistakable (ruled 2026-11-22): the chosen card and the focused button are filled, so where the keyboard is never shares its cue with a status colour; every popup button can be reached and pressed by keyboard (spec F1).
- One pathway per behaviour (ruled 2026-11-22): downloading, placing files, library management and the like each run through one shared helper, not a copy per function. The five helpers, their homes and call sites are the spec's "Shared helpers" section (process runner, landing, selection, run driver, converter result); the run driver lives in `sdl-core`. New dependencies confirmed for it: `tokio-util`, `which`. Looked at and not used: `ffmpeg-sidecar`, `process-wrap`, `cap-std`, `renamore`, any playlist crate. `atomic-write-file` was confirmed and then dropped the same day: a failed save leaves its temp file behind; `tempfile` does the same job and cleans up.
- Rulings from the pre-release review (2026-11-22):
  - Two playlists whose file names would be the same (same name, or equal once made safe or with case ignored) never share a file: the later one is written as `<name> (<list ID>).m3u8` (spec F7).
  - The library is every audio file under its folder at any depth, dot names skipped, symbolic links not followed. This replaces "exactly three levels deep" (spec F7, H3).
  - Only one copy of `sdl` runs at a time: a second one started while the first is open says so and exits (spec F11).
  - An instrumental matching the vocal version under the F3 match rule is accepted for now and reviewed later (`.fleet/docs/BACKLOG.md`).
- Device clean-up and eject are separate (ruled 2026-11-23, replaces "clean up and eject" as one step): clean-up runs at the end of every Sync device run, with no question, because a Finder eject is a likely path and skips anything tied to the app's own eject. The clean-up list and the narrow `._` rule are unchanged. After a completed sync the app asks "Eject <device>?" and ejects by itself after 30 seconds without an answer. Sync device also lists plugged-in drives that are not mounted and mounts the chosen one with `diskutil mount` at Continue, read-write (replaces "does not mount anything"; a read-only mount for From device was considered and dropped the same day as complexity for a rare case). Spec F10.
- Cargo builds into `$SHIP_ROOT/outputs/cargo/target` (the fleet cargo config in `~/.config/cargo`, 2026-11-21), not into the repo; `cargo install` puts binaries in `$SHIP_ROOT/outputs/cargo/bin`.
- Build standard is `fleet-47/Gen.v2`: fenix stable, the global `~/.cargo/config.toml`, and the lint flags in the spec. The repo carries no `.cargo/`, `clippy.toml`, `[lints]` table, rustflags or profile overrides.
- mise config lives in `.mise/config.toml` and is committed. mise supplies yt-dlp, ffmpeg and deno and starts the TUI.

## Layout
- `Cargo.toml`, `crates/`: the workspace. `sdl-core` (shared types, config, state and review files, path rules), `sdl-spotify`, `sdl-youtube`, `sdl-library` (genre, tags, playlists, converter), `sdl-devices`, `sdl-tasks` (the runner and the five functions), `sdl-form` (the form module), `sdl` (binary: screen and dialogs).
- `config.example.toml`: copy to `~/.config/sdl/config.toml`.
- `.mise/config.toml`: tools and the `tui` task.
- `.fleet/docs/`: `SPEC-tui.md` (the build contract), `BACKLOG.md` (tasks waiting on a condition), `HANDOFF.md`.

## Run
- Build: `cargo build --release --locked` with the fenix stable toolchain.
- Start: `mise run tui` (runs `$SHIP_ROOT/outputs/cargo/target/release/sdl`).

## Facts
- Audio is sourced from YouTube Music, not Spotify. Lossy quality is accepted; files are Opus as served, not re-encoded.
- Spotify app redirect URI: `http://127.0.0.1:9900/`.
- Spotify dev-mode apps lock out for about 24 h after too many calls.
- If downloads break after YouTube changes, the fix arrives via a new yt-dlp release (`mise upgrade`).
- The flow before the TUI (mise tasks, spotDL, fnox, rclone bisync) is in jj history: change `nplromuk`.
