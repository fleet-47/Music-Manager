# Spec: `sdl`, one Rust TUI for a personal music library

`sdl` builds one music library from Spotify lists and pasted links, keeps every song as one Opus
file, and copies or converts it out to players. Rulings in `.agents/AGENTS.md` bind this spec; where
they conflict, AGENTS.md wins. Each feature is followed by what it is not. Anything not listed is
not built: it is a question for Yazid.

Before using any crate, binary or system tool named below, read its docs at the version in use
(docs.rs or the source under `~/.cargo/registry/src`; `--help` and the README for binaries) and use
the calls named here. If a named call does not exist or does not do what this spec says, stop and
report it; do not substitute another crate or hand-write the capability.

## F1. One screen
- **Main panel**, always visible: one row per function, each with its current selection and its
  last-run line: Sync Spotify, Download link, Review (with the number of held songs), Sync device
  and Convert.
- **Running panel**, on the right, visible only while a task runs: per task its name, headline,
  count (`412 of 2,117`), progress bar, time estimate and `N need attention`. A spinner moves
  beside each running task's name, so a long single item never looks hung; a task with no known
  total shows no bar. When a task ends its result becomes that function's
  last-run line.
- **Footer**: the keys valid right now. `?` opens help listing every key and mouse action.
- **Popups** (centred, `tui-popup`; a popup keeps its position while it is open, even when its
  content gets shorter; long content scrolls with `tui-scrollview` and
  `tui-scrollbar`): function forms, review, attention list, confirmations, errors, help.
- **Forms** (one shared form module, modelled on Charm's Huh; every function uses it and has no
  bespoke popup). Fields:
  - *Text*: `tui-input`; paste supported; a plain message under the field when the value is not
    acceptable.
  - *Dropdown*: shows its current value; Enter, Space or a click opens the full option list.
  - *Radio group*: every option visible at once.
  - *Tick list*: a filter box above the list; typing filters (ranked with `nucleo-matcher`), Space
    ticks, a line always shows `5 of 34 ticked`; the list scrolls inside a fixed height.
  - *Toggle*.
  - *Folder field*: a text field plus a Browse action that opens `ratatui-explorer` to choose a
    folder.
  - *Buttons*: `Continue` (or `Start` on the last step) and `Cancel`, reachable by Tab and click.
  - A summary line, and under it what is still missing when the form cannot continue. These
    lines and a field's message wrap; they are never cut off.
  - Keys: Tab and Shift+Tab move between fields; Up/Down move inside a field; Enter continues;
    Esc clears a non-empty filter, otherwise cancels. Fields are pre-filled from what was
    remembered. A field's options can change while the form is open (a device is plugged in, a
    list arrives from a task).
- **Mouse** (`crossterm` mouse capture): a click focuses or activates what is under it (function
  row, field, option, button, attention item); the wheel scrolls the list under it. Each drawn
  element records its `Rect`; a click is tested against those. Every mouse action has a keyboard
  equivalent.
- **Motion** (`tachyonfx`): popups and the running panel fade or slide in and out (150 ms at most);
  a function row flashes once when its task ends. Input is handled immediately while an effect
  plays. Config `animations = false` turns effects off.
- **Attention list** (`a`): the held (yellow) and failed (red) items of the selected function's
  last or current run: title, artist and a plain reason that wraps. The last run's items are saved
  (F11), so the list is still there after a restart.
- **Errors**: a popup saying what went wrong and what to do, in plain words. Never a raw library
  error alone.
- **Confirmations**: before stopping a task (`s`), before quitting while a task runs, before
  ejecting a device (the question says that clean-up also empties the device's Trash). Quitting
  with nothing running needs none.
- **Colour** (`catppuccin` 2.9.1, feature `ratatui`, flavour Mocha, background painted with
  `base`), roles defined once in the `theme` module: headings and panel titles `mauve` bold; body
  `text`; secondary `subtext0`; emphasis (numbers, selection marker, key names) `peach`; borders
  `surface2`, the focused field or popup `lavender`; selected row background `surface0`; running
  `blue`, done `green`, attention / held `yellow`, failed `red` (failures only),
  skipped `overlay1`. A non-empty `NO_COLOR` disables colours, background and effects.
- **Size**: layouts use `Constraint::Min/Fill/Length` and `Flex`; under 60 columns the running panel
  moves below the main panel; under 12 rows only the function rows, running counts and a one-line
  footer show.
- **Not:** no tabs, no per-song table, no log pane, no theme setting, no command mode, no
  mouse-only action, no effect that blocks or delays input.

## F2. Drawing
- The screen is drawn only after something changed: a key, mouse or paste event, a task event, or
  an animation frame. All queued task events are applied before one draw.
- Timers exist only while needed: an animation timer (about 30 frames a second) while a
  `tachyonfx` effect is running; a spinner timer (100 ms) while a task runs or a form is
  busy, which also advances the elapsed time. With nothing running and no effect playing the loop waits
  on input alone and no timer exists. One pure function decides the next wake from the app state
  and is unit-tested (idle → none).
- Progress bars and counts change only when a progress event arrives.
- **Not:** no fixed polling tick, no redraw while idle.

## F3. Intake: one way into the library
- Every song enters as **Opus from YouTube Music**. Input is a *wanted song*: title, artists,
  album, album artist, track and disc numbers, date, ISRC, duration, cover URL, as far as the
  source knows them. Sources: a Spotify listing entry (F4) or a link item (F5).
- Steps per wanted song: skip if present (F7) → match (below) → download to a temp file → look up
  and tag the temp file (F6) → land it in the library (F7). A file enters the library only when
  complete.
- **One file per place, never replaced.** Within a run, wanted songs with the same place (F7) are
  handled once. Landing uses `tempfile`'s `persist_noclobber`, which fails instead of replacing an
  existing file; when the place was taken meanwhile (another task landed it), the song counts as
  skipped and the temp file is removed. This holds in the library. On a device or a Convert
  destination the drive may be FAT32, where macOS refuses that call (measured: os error 45); there
  a file is renamed into place only after checking the place is free, and only one task writes to
  a given device or destination at a time.
- **Match** (`sdl-youtube`): `innertube-rs` 0.12 `Innertube::search_music(query,
  Some(MusicSearchFilter::Songs))`, one client per run. Query the ISRC first when there is one; if
  nothing passes, query `<first artist> <title>`. Among the first 5 songs a candidate *passes* when
  its duration is within 3 s, its title matches and at least one artist matches. Text is compared
  lower-cased, without punctuation and without bracketed parts naming a feature or version
  (`feat`, `ft`, `with`, `remaster`, `version`, `edit`); titles match when `strsim::jaro_winkler`
  ≥ 0.9 or the shorter is a whole-word prefix of the longer; artists match only at
  `jaro_winkler` ≥ 0.9; empty text matches nothing. Score = title similarity + best artist
  similarity + 0.5 when the albums match. The best passing candidate is used.
- **Held, never guessed**: when no candidate passes, the song is not downloaded. It is saved to
  the review list (F8) with the candidates found (up to 5: video ID, title, artists, album,
  duration) and the reason. A search that fails (network) is a failed item, not a held one.
- **A YouTube Music link item (`music.youtube.com`) is its own source**: no matching; it downloads
  directly. Every other link item, plain YouTube included, goes through matching.
- **Download** (`sdl-youtube`): the `yt-dlp` binary through `tokio::process::Command`
  (`.stdin(Stdio::null())`, `.kill_on_drop(true)`, output piped):
  `-f bestaudio[acodec=opus]/bestaudio -x --no-playlist --no-progress
  -o <temp dir>/audio.%(ext)s -- <url>`, with no `--audio-format`, so nothing is ever re-encoded; a
  match's URL is `https://music.youtube.com/watch?v=<video_id>`. An Opus stream is kept as it is.
  When a song has no Opus stream, the best stream is kept as served (usually M4A): fidelity comes
  before a uniform file type. Each download has its own temp directory (`tempfile`) inside
  `<music_root>/.sdl-tmp/`; the result is the single audio file left in it, and its extension is
  its real type. The directory is removed on success, failure or cancel; `.sdl-tmp` is removed when
  no task runs.
- **The link's own audio** (only when chosen in review, F8): `-f bestaudio -x` with no
  `--audio-format`, so the audio is kept as served. Opus, MP3 and M4A files enter as they are; any
  other type is converted to Opus with the converter (F9).
- `ErrorKind::NotFound` on spawn becomes "yt-dlp was not found on PATH"; a non-zero exit reports
  the last `ERROR:` line of stderr.
- **Not:** no format choice at download, no re-encoding inside the library, no yt-dlp search, no
  plain-YouTube search fallback, no fingerprinting, no paid or ripped sources, no cookies, not the
  `yt-dlp` wrapper crate.

## F4. Sync Spotify
- Steps: choose the function → sign in if needed (when the browser is needed, a popup first says
  "Sync Spotify opens your browser to sign in" with Continue and Cancel) → form: a tick list of
  Liked Songs and each playlist the user owns, with song counts, remembered → Continue lists the
  ticked lists' songs and shows `N new of M · K waiting in Review` (K counts listed songs already
  in the review list; nothing is searched before Start) → Start. Matching happens during the run,
  in the running panel.
- **Sign-in is remembered.** `rspotify` 0.16 `AuthCodePkceSpotify`, scopes `user-library-read` and
  `playlist-read-private`, redirect `http://127.0.0.1:9900/`. The refresh token is kept in the
  macOS Keychain with the `keyring` crate (service `sdl`, account `spotify`) and nowhere else. The
  crate is keyring 4.2 with its default `v1` feature, which selects the real macOS Keychain store
  (`apple-native-keyring-store`); there is no mock store in this build. On
  start of a sync: a stored refresh token is exchanged for an access token with rspotify's refresh
  call, and the new refresh token Spotify returns replaces the stored one (when the response
  carries none, the stored one is kept). With no stored token, or
  when the refresh is refused, the browser sign-in runs: `get_authorize_url` opened with the `open`
  crate; the code arrives on our own `tokio::net::TcpListener` on `127.0.0.1:9900`, which keeps
  accepting until a request carries `code=`, answers with a short "you can close this tab" page and
  is always dropped afterwards; `parse_response_code`, then `request_token`. A redirect carrying
  `error=` ends sign-in as failed with that reason. A Keychain read or write that fails never blocks a sync: the
  browser sign-in is used and the reason is shown once.
- Listing: `current_user_playlists` (owner = `me()`), Liked Songs count from the first
  `current_user_saved_tracks` page, songs with rspotify's paginators at 50 per page. Local files
  and episodes are skipped.
- Each listed song goes through F3. After the run each ticked list's playlist file is written (F7).
- A 429 ends the task; the popup shows the wait from `Retry-After` (seconds or an HTTP date,
  `httpdate`), or "wait unknown". A 403 shows Spotify's reason.
- **Not:** no client secret, no token in any file, no retry layer, no followed or public
  playlists, no per-song Spotify calls, no deleting of songs, no sign-out function (access is
  revoked from the Spotify account page).

## F5. Download link
- Steps: choose the function → form: the link → Start.
- Listing: `yt-dlp --dump-single-json --flat-playlist -- <url>` gives the items and, for a
  playlist, its name and order. A flat listing can lack fields, so each item's own metadata is read
  at its turn with `yt-dlp --dump-single-json --no-playlist -- <item url>` and becomes a wanted
  song: title = `track`, else `title`; artist = `artist`, else `uploader`; `album`; `duration`;
  cover = the best JPEG or PNG in `thumbnails`, else `thumbnail` (the tag library cannot store
  WebP). An entry the site reports as unavailable (private or removed) is a failed item with that
  reason. YouTube Music items download directly; every other item goes through
  matching and is held for review when nothing passes (F3).
- A playlist link also writes a playlist file named after it (F7).
- **Not:** no Spotify links (they are F4), no format field, no confirmation step.

## F6. Genre, totals and tags (`sdl-library`)
- `musicbrainz_rs` 0.14 (features `async`, `rate_limit`, `rustls`), one process-wide client,
  User-Agent `sdl/<version>`.
- For a song with an ISRC: recording search `isrc:<code>`, a recording lookup, and one release
  lookup per release ID per run. Genre = the recording's top genre, else its release group's, else
  its first artist's. Track total and disc total from the release whose title matches the album.
- `lofty` 0.25, one write to the temp file through its format-independent tag interface: title,
  artists, album, album artist, track number and total, disc number and total, date, ISRC, genre,
  and the cover (fetched with `reqwest`, giving up after 20 seconds) as the front cover.
- A failed or empty lookup leaves those tags empty and the song still completes. A cover that
  cannot be downloaded leaves the song without a cover and the song still completes. The last-run
  line counts songs left without a genre and songs left without a cover. A failed tag write fails
  the song.
- **Not:** no Last.fm, no Spotify genre, no lyrics, no ReplayGain, no retagging of files already in
  the library.

## F7. Library layout and playlists (`sdl-core` path rules, `sdl-library` playlist files)
- A song lives at `<music_root>/<Album artist>/<Album>/<Title>.<ext>`: one file however many lists
  name it. Album artist falls back to the first artist; a song with no album goes in `Singles`.
  Names come from the wanted song's own fields (Spotify's, or the link's), never from MusicBrainz.
- Every path component is made FAT-safe (`sanitize-filename`), has leading dots stripped, is capped
  at 150 bytes on a character boundary (Disco reads each playlist line into a 700-byte buffer), and
  falls back to the Spotify or video ID when it would be empty.
- **The library** is every `.opus`, `.mp3` and `.m4a` file exactly at
  `<music_root>/<folder>/<folder>/<file>`, skipping any folder whose name starts with a dot. This is
  what "Whole library" means in F9 and F10; files elsewhere under `music_root` are not the app's.
- A song is **present** when a file with its stem exists at that place with extension `.opus`,
  `.mp3` or `.m4a`. Present songs are skipped before any network call.
- A **playlist** is a pointer file `<music_root>/<List name>.m3u8` (Liked Songs: `Liked Songs`):
  UTF-8, no BOM, `#EXTM3U`, `#PLAYLIST:<name>`, then one line per song in list order, as a path
  relative to the playlist file with forward slashes. Every song of the list is written at its
  place whether or not its file exists yet; an existing file's real extension is used, otherwise
  `.opus`. A playlist file is rewritten whenever its list is synced, and a song's line is corrected
  when review (F8) lands it with another extension. Disco skips a line it cannot resolve and
  imports the rest. Confirm against Disco's reader
  (`ui/musicdb.c` in `/Users/worf/Enterprise/engineering/Snowsky-DAC/disco-v3.6.1/src`) that this
  form imports.
- **Not:** no folders per source or per playlist, no duplicate copies, no database or index file,
  no playlist formats other than `.m3u8`, no editing of playlists in the app.

## F8. Review
- The review list (`~/.config/sdl/review.toml`, `serde` + `toml`) keeps every held song until it
  is resolved: the wanted song, where it came from (Spotify list names, or the link), its
  candidates and the reason. A song is identified by its Spotify track ID; a YouTube or YouTube
  Music item by its video ID (so the same video pasted with either address is one song); any other
  link item by its item URL. A sync never searches again for a song that is held or was marked "never".
- Steps: choose Review → a filterable list of held songs → Enter on a song shows what was wanted
  (title, artists, album, duration) and a radio group of choices: each candidate (title · artists
  · album · duration, with the parts that differ in `yellow`), **Use the link's own audio** (only
  for a song that came from a link), **Paste a link** (a text field; the pasted link's audio is
  used with the wanted song's tags), **Leave for later**, **Never download**. `o` opens the
  focused candidate in the browser (`open`). → `Download N chosen` starts a task that takes each
  chosen song through F3 from the download step and removes it from the list when it lands.
- **Not:** no automatic pick, no in-app audio preview, no editing of tags, no re-search button.

## F9. Converter and Convert (`sdl-library`)
- **Converter**, one module also used by F3 and F10: the `ffmpeg` binary through
  `tokio::process::Command` (`.stdin(Stdio::null())`, `.kill_on_drop(true)`), audio only, sample
  rate kept, written to a temp file and renamed when complete, then tags (and the cover, when kept)
  copied from the source with `lofty`. Presets, each confirmed in ffmpeg's docs:
  MP3 `-c:a libmp3lame -q:a 2` (High, about 190 kbps) / `-q:a 4` (Medium, about 165) / `-q:a 6`
  (Small, about 115); M4A `-c:a aac -b:a 192k` / `160k` / `128k`; Opus `-c:a libopus -b:a 160k`. A
  file already in the target format is copied as is. `ErrorKind::NotFound` becomes "ffmpeg was not
  found on PATH". Conversions run `std::thread::available_parallelism()` at a time.
- **Convert function**: form: **What** (tick list: Whole library, then each playlist),
  **Format** (dropdown: MP3 / M4A), **Quality** (dropdown: High / Medium / Small, each with its
  approximate bitrate), **Keep cover art** (toggle, on), **Destination** (folder field, default
  `~/Music-converted`), all remembered → summary `N files to convert · M already there` → Start.
  Output keeps the library's layout under the destination, with the ticked playlists' files
  written there listing what is present.
- **Not:** never changes or deletes the library; a destination inside `music_root` is refused in
  plain words in the summary; no other codecs or quality values; no sample-rate or channel
  options; no cover resizing; no ffmpeg library bindings.

## F10. Sync device (`sdl-library`)
- Steps: choose the function → form: **Device** (radio group: every mounted external or removable
  volume with its free space; the last one synced is preselected; "No device found. Plug one
  in." when there is none), **Direction** (radio: To device / From device), **What** (To device: a
  tick list of Whole library and each playlist; From device: a tick list of the device's top-level
  folders), **Format on device** (To device: dropdown As is / MP3 / M4A), **Quality** and **Keep
  cover art** (shown only when converting). A note says playlists are written as `.m3u8` files,
  which the player must support, and that a Disc shows a changed playlist as a new numbered copy
  beside the old one. Remembered per device → summary `N files, about X GB to copy · M
  already there · Y GB free on <device>`; when there is not enough room it says so in plain words
  and Start is unavailable → Start → when done, "Clean up and eject <device>?".
- Devices: every entry of `/Volumes` asked with `diskutil info -plist <mount point>`
  (`tokio::process::Command`), parsed with `plist` and `serde`; kept when external or removable. A
  mounted disk image is listed with "(disk image)" after its name (confirm in real `diskutil`
  output which key tells an image from a physical drive).
  Free space is `APFSContainerFree` when the volume has it, otherwise `FreeSpace` (on APFS
  `FreeSpace` reads 0; confirm both keys in real output). A
  device is identified by its volume UUID, falling back to its name. The list updates while the
  form is open when a volume appears or goes (`notify` watching `/Volumes`, non-recursive); `r`
  looks again by hand.
- To device: each chosen song is written to `<device>/Music/<same relative path>` when not present
  there in any format: copied to a `*.part` name and renamed, or converted first (F9) when a format
  is chosen and the file is not already in it. Size shown is exact for copies and estimated for
  conversions (duration × the preset's bitrate). Then each ticked playlist is written to
  `<device>/Music/<List name>.m3u8`, listing only songs present on the device, with the extension
  each has there.
- From device: `.opus`, `.mp3` and `.m4a` files under the ticked folders are copied as they are into the library at
  the place their tags give (F7); a file without a title or artist tag goes to
  `<music_root>/Unsorted/<its relative path>`. Files present in the library are skipped.
- Names starting with a dot are never copied, in either direction. A write that fails because the
  device is full or gone ends the run with a plain message; the files already complete stay.
- Stopping is cooperative here and in Convert: the file in hand is finished, then the run stops;
  the device stays mounted.
- Clean up and eject: remove `.Spotlight-V100`, `.Trashes`, `.fseventsd` and `.TemporaryItems`
  from the device root and every `.DS_Store`; remove a `._X` file only when `sdl` created the file
  or folder `X` in this run and the `._X` file starts with the AppleDouble signature (bytes `00 05 16 07`); then
  `diskutil unmount <mount point>`.
- **Not:** not a two-way merge in one run, never deletes a song on either side, removes nothing
  from a device except the macOS items named in clean-up, does not mount or format anything, no folder-per-playlist arrangement,
  no import of playlists from a device, no rclone, no DiskArbitration.

## F11. Tasks and state (`sdl`)
- Each function runs as one task; different functions can run at the same time, the same function
  not twice. The UI keeps per task only: counts (total, done, added, skipped, held, failed), the
  headline and the attention items.
- Stopping a download task aborts it (`JoinHandle::abort`; children die through `kill_on_drop`);
  Convert and Sync device stop cooperatively (F9, F10). The last-run line says "stopped" with the
  count finished before the stop.
- `state.toml` and `review.toml` each have one writer: the task layer's runner. Tasks send it
  changes; no task writes either file itself, so two tasks can never overwrite each other's
  changes. Each write is atomic (a temp file in the same folder, then a rename), so a crash never
  leaves a half-written file.
- `~/.config/sdl/state.toml`: the ticked Spotify lists, the Convert choices, per device its
  name and choices, the last device (so the main panel names it after a restart), the "never download" songs, and each function's last-run line with its
  time, its outcome (done / needs attention / failed / stopped) and its attention items.
- Song downloads run `download_jobs` at a time (`futures` `buffer_unordered`).
- **Not:** no resume of half-finished files, no history beyond the last run, no queue of pending
  functions.

## F12. Libraries
- A **library** is a named folder directly under `music_root`: `<music_root>/<Library>/`. It holds
  its own songs in the F7 layout and its own playlist files; wherever F3 to F11 say "the library"
  or `<music_root>/<Album artist>/…`, read the chosen library's folder. One library is the owner's
  own, named by the config key `my_library` (default `Me`). The others are created by name in the
  app, one per person the owner downloads for. The libraries are the folders directly under
  `music_root` whose names do not start with a dot; the owner's own always exists.
- **Download link and Sync Spotify** forms start with a **For me** toggle, on every time the form
  opens. Switched off, a **Library** dropdown appears: the other libraries (the one last used is
  preselected) and **New library…**, which shows a text field for its name (made FAT-safe as in F7;
  an existing name is refused in plain words).
- **Sync device and Convert** forms start with a **Library** dropdown (the owner's own first),
  remembered per device and for Convert. "Whole library" and the playlists offered are those of the
  chosen library.
- **Review**: every held song belongs to the library it was wanted for, and lands there. The held
  list shows each song's library when it is not the owner's own. "Never download" applies to that
  song in that library only.
- **A song another library already has is copied, not downloaded again**: when the wanted song is
  present at the same place in another library, that file is copied (`std::fs::copy`, which clones
  on APFS and so takes no extra space) to a temp file and landed as in F3. It counts as added.
- The main panel's selection line for a function names the library when it is not the owner's own.
- On a device, songs go to `<device>/Music/<same relative path>` with no library level: a device
  holds one person's music. Syncing two libraries to the same device merges them there, and the
  form's note says so when the device was last synced from a different library.
- **Not:** no moving or copying of songs between libraries by hand, no deleting or renaming of
  libraries in the app, no merged view across libraries, no per-library settings, no library level
  on devices.

## Config: `~/.config/sdl/config.toml`
`client_id`, `music_root` (default `~/Music`, `~` expanded), `my_library` (default `Me`),
`download_jobs` (default 4, 0 is an error), `animations` (default true). Unknown keys are ignored. A missing or malformed file is a
plain error naming the path, printed before the screen starts. `config.example.toml` at the repo
root.

## Workspace
Cargo workspace at the repo root, edition 2024, `resolver = "3"`. Crates, each one deep module with
a small interface, split so that each can be built and tested by itself:
- `sdl-core`: shared types, config, state and review files, path rules.
- `sdl-spotify`: sign-in and listing. `sdl-youtube`: matching, link listing, downloads.
- `sdl-library`: MusicBrainz, tags, playlist files, the converter and Convert.
- `sdl-devices`: device list, plans, copy runs, clean-up and eject (uses `sdl-library`).
- `sdl-tasks`: the runner and the five functions' tasks; the only writer of the settings files.
- `sdl-form`: the form module (F1), free of any knowledge of this app.
- `sdl`: the binary: screen, dialogs, event loop.
Library crates depend only on `sdl-core` (and `sdl-devices` on `sdl-library`); `sdl-tasks` depends
on the library crates; `sdl` depends on `sdl-tasks`, `sdl-form` and `sdl-core`. In `sdl`, app state
and its update logic are free of drawing code. Errors: `thiserror` in libraries, `color-eyre` in the
binary.

## Build standard (`fleet-47/Gen.v2`)
- Toolchain: fenix stable. The global `~/.cargo/config.toml` supplies profiles, rustflags and the
  clippy config. The repo carries no `.cargo/`, `clippy.toml`, `rustfmt.toml`, `[lints]` table,
  rustflags or profile overrides.
- Gate, all with `--workspace --all-targets --all-features --locked`: `cargo fmt --all`,
  `cargo check`, `cargo clippy -- -D warnings -D clippy::all -D clippy::pedantic
  -D clippy::disallowed_methods -D clippy::unwrap_used -D clippy::expect_used -D clippy::panic
  -D clippy::arithmetic_side_effects -D clippy::cast_possible_truncation -D clippy::cast_possible_wrap
  -D clippy::cast_sign_loss -F unsafe_code -D clippy::allow_attributes
  -D clippy::allow_attributes_without_reason`, `cargo build`, and `cargo test` run as
  `mise x -- cargo test …`, because tests use the real `ffmpeg`, which is on `PATH` only through mise.
- No `#[allow]` or `#[expect]` anywhere. Tests return `Result` and use `?`.
- Tests cover pure logic and file operations in temp directories. No invented API fixtures.

## mise
`.mise/config.toml` holds the tools (yt-dlp, ffmpeg, deno) and one task, `tui`, that runs
`target/release/sdl` with them on `PATH`.
