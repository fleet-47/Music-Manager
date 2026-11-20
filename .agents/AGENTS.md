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
- Sign-in is PKCE (ruled 2026-11-18): the TUI uses only the client ID, which is not a secret and lives in its config file; the client secret is never given to it. Sign-in is remembered (ruled 2026-11-20, replaces "tokens in memory only"): the refresh token is stored in the macOS Keychain, encrypted at rest, and nowhere else; the browser opens only on first use or when the token stops working. The access token stays in memory. Only the scopes `user-library-read` and `playlist-read-private` are requested. fnox and Bitwarden are not used.
- A Spotify 429 ends the run and shows the wait; there is no retry layer.
- Device sync (ruled 2026-11-19) works with any mounted external or removable drive chosen in the UI, in either direction (to the device, or from it into the library), always add-only, with ticked folders and an optional format for the device (as is, MP3, M4A) remembered per device. No device is named in the config. Drives are found and ejected with macOS `diskutil` (DiskArbitration bindings need `unsafe`, which the build standard forbids).
- Library standard (ruled 2026-11-20, replaces the 2026-11-19 format choice at download): every song enters the library as Opus from YouTube Music, found by metadata, whatever the source of that metadata (Spotify listing, a pasted link). The link's own audio is used only when no match is found. MP3 and M4A are made only when music leaves the library (device sync, Convert). A song already present is skipped.
- Convert (ruled 2026-11-19) writes converted copies to a separate destination and never changes the library. Conversion runs the `ffmpeg` binary directly: the second wrapped tool beside yt-dlp, allowed for conversion only, because no pure-Rust MP3 encoder exists.
- No dot-directory may ever reach a device: Disco's scanner (`disco-v3.6.1/src/ui/scanner.c:1607-1621` in the Snowsky-DAC repo) disables pruning of vanished songs when it meets any dot-directory except root `.diskos`. The TUI strips leading dots from every path component and removes `.Spotlight-V100`, `.Trashes` and `.fseventsd` from the card before unmount.
- Eject clean-up (ruled 2026-11-20) never deletes a file a player may own: a `._X` file is removed only when `sdl` wrote `X` in that sync and the file starts with the AppleDouble signature. `._` files from earlier copies are a one-time manual task (`.fleet/docs/BACKLOG.md`).
- Playlists (ruled 2026-11-20) are pointer files in a playlist format the Disc reads (`.m3u8`; Disco matches entries by path, then file name: `ui/musicdb.c` in the Snowsky-DAC repo). A playlist never dictates where a song's file lives.
- Redraw (ruled 2026-11-20) is event-driven: nothing is drawn while nothing changes, progress updates on progress events, and a timer runs only while an animation is on screen. No fixed polling tick.
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
- Eject clean-up list (confirmed 2026-11-20): `.Spotlight-V100`, `.Trashes` (this empties the device's Trash; the eject question says so), `.fseventsd` and `.TemporaryItems` at the device root, every `.DS_Store`, and the narrow `._` rule.
- The old-flow folders were moved out of the library to `~/Music-old` (2026-11-20), so the library starts clean.
- yt-dlp's JavaScript engine stays `deno` (ruled 2026-11-20). Reason: yt-dlp runs YouTube's script in it with no file system or network access; QuickJS is 1.4 MB against 82 MB but has no such restriction, and Bun is larger and deprecated by yt-dlp (yt-dlp wiki, EJS page).
- Build standard is `fleet-47/Gen.v2`: fenix stable, the global `~/.cargo/config.toml`, and the lint flags in the spec. The repo carries no `.cargo/`, `clippy.toml`, `[lints]` table, rustflags or profile overrides.
- mise config lives in `.mise/config.toml` and is committed. mise supplies yt-dlp, ffmpeg and deno and starts the TUI.

## Layout
- `Cargo.toml`, `crates/`: the workspace. `sdl-core` (shared types, config, state and review files, path rules), `sdl-spotify`, `sdl-youtube`, `sdl-library` (genre, tags, playlists, converter), `sdl-devices`, `sdl-tasks` (the runner and the five functions), `sdl-form` (the form module), `sdl` (binary: screen and dialogs).
- `config.example.toml`: copy to `~/.config/sdl/config.toml`.
- `.mise/config.toml`: tools and the `tui` task.
- `.fleet/docs/`: `SPEC-tui.md` (the build contract), `BACKLOG.md` (tasks waiting on a condition), `HANDOFF.md`.

## Run
- Build: `cargo build --release --locked` with the fenix stable toolchain.
- Start: `mise run tui` (runs `target/release/sdl`).

## Facts
- Audio is sourced from YouTube Music, not Spotify. Lossy quality is accepted; files are Opus as served, not re-encoded.
- Spotify app redirect URI: `http://127.0.0.1:9900/`. Each run opens a browser sign-in.
- Spotify dev-mode apps lock out for about 24 h after too many calls.
- If downloads break after YouTube changes, the fix arrives via a new yt-dlp release (`mise upgrade`).
- The flow before the TUI (mise tasks, spotDL, fnox, rclone bisync) is in jj history: change `nplromuk`.
