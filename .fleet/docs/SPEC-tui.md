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
- **Home screen**: one card per function, and nothing else. In the grid the cards sit in three
  columns, centred both ways: Sync Spotify above Download link (bring music in), Review in the
  middle spanning both rows (check), Sync device above Convert (take music out). Each card has its
  name in the border and says only what helps now:
  - what it will act on (`3 lists chosen`, `SNOWSKY · To device · MP3`), or, while nothing is
    remembered, one line saying what the function does;
  - how its last run went, only where that helps: the two syncs always say so with the time
    (`✓ 14 added · 14:05 21/11/2026`); Download link, Review and Convert say so only until the app
    is closed. Nothing is shown for a function that never ran;
  - on its last line what Enter does (`sync`, `paste a link`, `review`, `convert`); the chosen
    card shows the key. Review offers nothing while no song waits.
  Where the keyboard is and what state a card is in are told by different things, so one never
  hides the other. The chosen card is filled (`surface0`), has a heavy border and `›` before its
  name. The border's colour tells the state of any card, chosen or not: `yellow` for one that
  waits for the owner (held songs, or a last run that left something failed or held; a double
  border when it is not the chosen one), `blue` while it runs, otherwise `lavender` when chosen
  and `surface2` when not. The arrow keys (and `h j k l`) move between cards.
- **Progress lives in the card.** While a function runs, its card's content gives way to its
  headline, count (`412 of 2,117`), progress bar, time estimate and `N need attention`, with a
  spinner beside its name, so a long single item never looks hung; a task with no known total
  shows no bar. When it ends its result takes the progress's place. There is no separate running
  panel.
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
    The button with the focus is filled (`base` on `lavender`); the same holds for the buttons
    of every other popup.
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
- **Motion** (`tachyonfx`): popups fade in and out (150 ms at most); a card flashes once when its
  task ends. Input is handled immediately while an effect
  plays. Config `animations = false` turns effects off.
- **Attention list** (`a`): the held (yellow) and failed (red) items of the selected function's
  last or current run: title, artist and a plain reason that wraps. The last run's items are saved
  (F11), so the list is still there after a restart.
- **Errors**: a popup saying what went wrong and what to do, in plain words. Never a raw library
  error alone.
- **Confirmations**: before stopping a task (`s`), before quitting while a task runs, before
  ejecting a device (F10: this one answers itself after 30 seconds). Quitting
  with nothing running needs none. A question's two answers are buttons: Left, Right and Tab move
  the focus between them, Enter presses the focused one, and `y` and `n` answer directly. The
  focus starts on the answer that does no damage.
- **Colour** (`catppuccin` 2.9.1, feature `ratatui`, flavour Mocha, background painted with
  `base`), roles defined once in the `theme` module: headings and panel titles `mauve` bold; body
  `text`; secondary `subtext0`; emphasis (numbers, selection marker, key names) `peach`; borders
  `surface2`, the focused field or popup `lavender`; the focused button `base` on `lavender`;
  the chosen card's and the selected row's background `surface0`; running
  `blue`, done `green`, attention / held `yellow`, failed `red` (failures only),
  skipped `overlay1`. A non-empty `NO_COLOR` disables colours, background and effects.
- **Size**: the grid needs 72 columns and 14 rows besides the footer; a card is at most 36 columns
  wide, so a wide terminal gets air around the grid. A narrower terminal stacks the cards in one
  centred column (two lines each); one too short for that shows one line per function with its
  count, and a one-line footer.
- **Not:** no tabs, no per-song table, no log pane, no theme setting, no command mode, no
  mouse-only action, no effect that blocks or delays input.

## F2. Drawing
- The screen is drawn only after something changed: a key, mouse or paste event, a task event, or
  an animation frame. All queued task events are applied before one draw.
- Timers exist only while needed: an animation timer (about 30 frames a second) while a
  `tachyonfx` effect is running; a spinner timer (100 ms) while a task runs or a form is
  busy, which also advances the elapsed time; a countdown timer (once a second) while the eject
  question is the top popup (F10). With none of these the loop waits
  on input alone and no timer exists. One pure function decides the next wake from the app state
  and is unit-tested (idle → none).
- Progress bars and counts change only when a progress event arrives.
- **Not:** no fixed polling tick, no redraw while idle.

## F3. Intake: one way into the library
- Every song enters as **Opus from YouTube Music**. Input is a *wanted song*: title, artists,
  album, album artist, track and disc numbers, date, ISRC, duration, cover URL, as far as the
  source knows them. Sources: a Spotify listing entry (F4) or a link item (F5).
- Steps per wanted song: skip if present (F7) → match (below) → download to a temp file → tag
  the temp file with the wanted song's own fields and cover (F6) → land it in the library (F7) →
  queue it for enrichment (F6). A file enters the library only when complete: its audio, the
  source's tags and its cover. No step asks MusicBrainz (ruled 2026-11-23).
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
  in the review list; nothing is searched before Start) → Start. When Spotify sends playlist
  entries that cannot be read as songs, the line ends `· N entries could not be read` (added
  2026-11-22; local files, episodes and removed songs are left out without a count). Matching
  happens during the run, shown in the card.
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
- Steps: choose the function → form: one or more links → Start.
- **Several links at once**: the Link field takes any number of links separated by spaces, line
  breaks or commas (a pasted line break is kept as a separator). Each must be an `http` or `https`
  link that is not a Spotify link; the field's message names the link that is not. A link given
  twice is taken once. The run reads each link in turn, then takes all their items as one run
  with one total; the "For me" choice applies to all of them. A link that cannot be read is a
  failed item naming the link and the run goes on; when no link can be read the run ends with
  that error.
- Listing: `yt-dlp --dump-single-json --flat-playlist -- <url>` gives the items and, for a
  playlist, its name and order. A flat listing can lack fields, so each item's own metadata is read
  at its turn with `yt-dlp --dump-single-json --no-playlist -- <item url>` and becomes a wanted
  song: title = `track`, else `title`; artist = `artist`, else `uploader`; `album`; `duration`;
  cover = the best JPEG or PNG in `thumbnails`, else `thumbnail` (the tag library cannot store
  WebP). An entry the site reports as unavailable (private or removed) is a failed item with that
  reason. YouTube Music items download directly; every other item goes through
  matching and is held for review when nothing passes (F3).
- Each playlist link also writes its own playlist file, named after it (F7).
- **Not:** no Spotify links (they are F4), no format field, no confirmation step.

## F6. Genre, totals and tags (`sdl-library`)
- `musicbrainz_rs` 0.14 (features `async`, `rate_limit`, `rustls`), one process-wide client,
  User-Agent `sdl/<version> ( <repo URL> )`, the contact MusicBrainz requires. Its addresses are tried IPv4 first and IPv6 after (ruled
  2026-11-23): the client's `ureq` agent is built with `Agent::with_parts` and a resolver that
  wraps ureq's `DefaultResolver` and puts the IPv4 addresses in front.
- **At landing** (F3): `lofty` 0.25, one write to the temp file through its format-independent tag
  interface: title, artists, album, album artist, track number, disc number, date, ISRC, and the
  cover (fetched with `reqwest`, giving up after 20 seconds) as the front cover. A cover that
  cannot be downloaded leaves the song without a cover and the song still completes; the last-run
  line counts songs left without a cover. A failed tag write fails the song.
- **Enrichment queue** (ruled 2026-11-23): every song a download lands that has an ISRC is added
  to `~/.config/sdl/pending_enrichment.toml` (`serde` + `toml`): its library and the wanted song,
  once per library and song (F8's identity). The file's place is worked out from those (F7); no
  path is stored. The runner is its only writer (F11).
- **Enrichment worker** (ruled 2026-11-23): one background task, started with the app, alive as
  long as the app runs and ended with it; it is not one of the five functions and shows nothing
  on screen. It takes the queue's songs one at a time, in order, woken when a song is queued:
  - asks MusicBrainz: recording search `isrc:<code>`, a recording lookup, and one release lookup
    per release ID while the app runs. Genre = the recording's top genre, else its release
    group's, else its first artist's. Track total and disc total from the release whose title
    matches the album;
  - adds genre, track total and disc total to the song's file in the library: read into memory,
    tagged with `lofty`, and written back whole with H2's atomic write, so a crash leaves the old
    file or the new one. No other tag, the cover and the audio are not changed;
  - MusicBrainz answered, with or without data: the song leaves the queue. Its file is no longer
    in the library: it leaves the queue. The request or the write failed: it stays, and the
    worker tries again after 60 seconds.
- **Not:** no Last.fm, no Spotify genre, no lyrics, no ReplayGain; no MusicBrainz call inside a
  run; no enrichment of songs without an ISRC, of songs copied from another library (F12) or of
  songs that were in the library before 2026-11-23; no parallel lookups; no retry limit; no
  record of what was enriched.

## F7. Library layout and playlists (`sdl-core` path rules, `sdl-library` playlist files)
- A song lives at `<music_root>/<Album artist>/<Album>/<Title>.<ext>`: one file however many lists
  name it. Album artist falls back to the first artist; a song with no album goes in `Singles`.
  Names come from the wanted song's own fields (Spotify's, or the link's), never from MusicBrainz.
- Every path component is made FAT-safe (`sanitize-filename`), has leading dots stripped, is capped
  at 150 bytes on a character boundary (Disco reads each playlist line into a 700-byte buffer), and
  falls back to the Spotify or video ID when it would be empty.
- **The library** is every `.opus`, `.mp3` and `.m4a` file under the library's folder at any depth,
  skipping any file or folder whose name starts with a dot and not following symbolic links (ruled
  2026-11-22, replaces "exactly three levels deep": files From device puts under `Unsorted` sit at
  other depths). This is what "Whole library" means in F9 and F10.
- A song is **present** when a file with its stem exists at that place with extension `.opus`,
  `.mp3` or `.m4a`. Present songs are skipped before any network call.
- A **playlist** is a pointer file `<music_root>/<List name>.m3u8` (Liked Songs: `Liked Songs`):
  UTF-8, no BOM, `#EXTM3U`, `#PLAYLIST:<name>`, then one line per song in list order, as a path
  relative to the playlist file with forward slashes. Every song of the list is written at its
  place whether or not its file exists yet; an existing file's real extension is used, otherwise
  `.opus`. A playlist file is rewritten whenever its list is synced, and a song's line is corrected
  when review (F8) lands it with another extension. Two lists never share a file (ruled
  2026-11-22): when a list's file name is already taken by another list, compared with case
  ignored, it is written as `<List name> (<list ID>).m3u8`, and its held songs record that name.
  Disco skips a line it cannot resolve and
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
  volume with its free space, then the not-mounted ones, named below; the last one synced is preselected; "No device found. Plug one
  in." when there is none), **Direction** (radio: To device / From device), **What** (To device: a
  tick list of Whole library and each playlist; From device: a tick list of the device's top-level
  folders), **Format on device** (To device: dropdown As is / MP3 / M4A), **Quality** and **Keep
  cover art** (shown only when converting). A note says playlists are written as `.m3u8` files,
  which the player must support, and that a Disc shows a changed playlist as a new numbered copy
  beside the old one. Remembered per device → summary `N files, about X GB to copy · M
  already there · Y GB free on <device>`; when there is not enough room it says so in plain words
  and Start is unavailable → Start → clean-up at the end of the run → when the run completed,
  "Eject <device>?". The form says that every sync ends with a clean-up that also empties the
  device's Trash.
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
- Clean-up (ruled 2026-11-23: its own step, no longer part of eject): runs as the last step of
  every Sync device run, in either direction, whether the run completed, was stopped or failed,
  as long as the device is still mounted; no question is asked. It removes `.Spotlight-V100`,
  `.Trashes`, `.fseventsd` and `.TemporaryItems` from the device root and every `.DS_Store`;
  removes a `._X` file only when `sdl` created the file or folder `X` in this run and the `._X`
  file starts with the AppleDouble signature (bytes `00 05 16 07`). The task is still running
  while it cleans, so the device stays busy until it is done. A clean-up that fails is reported
  on the run's line and does not change the run's own result.
- Eject (ruled 2026-11-23): after a completed run and its clean-up, the question "Eject
  <device>?" shows a countdown from 30 seconds and ejects when it reaches zero; Eject answers
  at once, Keep mounted cancels. Eject is `diskutil unmount <mount point>` and nothing else. A
  stopped or failed run asks nothing. The countdown redraws once a second while the question
  is on screen (F2). When another popup covers the question, the countdown starts again from
  30 seconds once the question is back on top (ruled 2026-11-23).
- Same drive (ruled 2026-11-23): clean-up and eject act only when the volume at the mount point
  is still the device the run used, checked inside `sdl-devices` by asking
  `diskutil info -plist <mount point>` and comparing the device's identity (volume UUID,
  falling back to its name, as in the device list). When the place is gone or another volume is
  mounted there, clean-up does nothing; eject unmounts only the same volume and otherwise
  counts as done, because the drive is already out. A plain folder that is not a volume is
  cleaned like any folder (it holds nothing a drive owns) and is never unmounted.
- Not-mounted drives (ruled 2026-11-23): the Device list also shows external or removable
  volumes that are plugged in but not mounted, each with "(not mounted)" after its name, found
  with `diskutil list -plist external` (confirm the keys in real output and in `man diskutil`;
  partitions that hold no user file system, such as EFI, are left out). Choosing one and
  continuing mounts it with `diskutil mount <device identifier>` (read-write, the system's
  default); a mount that fails shows `diskutil`'s own words and the form stays open. Nothing is
  mounted by moving through the list or by opening the form. From device: a not-mounted drive
  has no folders to tick yet, so Continue is available without them; it mounts the drive, the
  folders then appear, and Continue is unavailable again until one is ticked. The list is read again when `/Volumes` changes and on `r`.
- **Not:** not a two-way merge in one run, never deletes a song on either side, removes nothing
  from a device except the macOS items named in clean-up, does not format anything, no folder-per-playlist arrangement,
  no import of playlists from a device, no rclone, no DiskArbitration.

## F11. Tasks and state (`sdl`)
- Each function runs as one task; different functions can run at the same time, the same function
  not twice. The UI keeps per task only: counts (total, done, added, skipped, held, failed), the
  headline and the attention items.
- Stopping a download task aborts it (`JoinHandle::abort`; yt-dlp and everything it started die
  with it, H1);
  Convert and Sync device stop cooperatively (F9, F10). The last-run line says "stopped" with the
  count finished before the stop.
- Only one copy of `sdl` runs at a time (ruled 2026-11-22): at start it takes a lock on
  `~/.config/sdl/lock` (the standard library's file lock, which the system releases when the
  process ends however it ends); a second copy prints `sdl is already running` and exits before
  touching the terminal.
- Leaving (ruled 2026-11-22): quitting while Convert or Sync device runs waits for the file in hand
  and the saved "stopped" line before the app closes; quitting while an eject or a tools install is
  in progress asks first and waits for it. A hang-up or terminate signal (the terminal closed)
  takes the same path as quitting with everything stopped, so no tool is left running.
- `state.toml` and `review.toml` each have one writer: the task layer's runner. Tasks send it
  changes; no task writes either file itself, so two tasks can never overwrite each other's
  changes. Each write is atomic (a temp file in the same folder, then a rename), so a crash never
  leaves a half-written file.
- `~/.config/sdl/state.toml`: the ticked Spotify lists, the Convert choices, per device its
  name and choices, the last device (so its card names it after a restart), the "never download" songs, and each function's last-run line with its
  time, its outcome (done / needs attention / failed / stopped) and its attention items.
- Song downloads run `download_jobs` at a time (`futures` `buffer_unordered`).
- `pending_enrichment.toml` (F6) is written by the runner alone, like the two files above: a
  download task sends it the song to queue, the enrichment worker the song to remove.
- Nothing outlasts the app (ruled 2026-11-24): the enrichment worker is aborted when the runner
  is dropped, and it starts no outside tool.
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
- A function's card names the library it will act on when it is not the owner's own.
- On a device, songs go to `<device>/Music/<same relative path>` with no library level: a device
  holds one person's music. Syncing two libraries to the same device merges them there, and the
  form's note says so when the device was last synced from a different library.
- **Not:** no moving or copying of songs between libraries by hand, no deleting or renaming of
  libraries in the app, no merged view across libraries, no per-library settings, no library level
  on devices.

## F13. Tools check
- At start the app looks on `PATH` (`which`) for the three tools its functions run: `yt-dlp`,
  `ffmpeg` and `deno`. When any is missing, the first row of the home screen names them and offers `i`, which
  runs `mise install` for the missing ones (`yt-dlp`, `conda:ffmpeg`, `deno`) and checks again.
- The row then says how it ended: nothing when all are found; that they were installed but are
  not on `PATH` yet, with how to start the app so they are (`mise run tui`); or what mise said when
  it failed.
- **Not:** no download or update of tools by the app itself, no change to any mise config, no
  check while a function runs.

## Shared helpers (ruled 2026-11-22)
One pathway per behaviour. Each helper below is the only place its behaviour is written; the call
sites named use it and their own copies are deleted. What the app does is unchanged, except the two
fixes named under H2 and H5. Each helper's interface is written out at the top of its file
(`sdl-core/src/{process,landing,run}.rs`, `sdl-library/src/selection.rs`).

- **H1. Process runner** (`sdl-core::process`): one function runs an outside tool and returns what
  it printed. `tokio::process::Command` with stdin closed, output piped, `.process_group(0)` and
  `.kill_on_drop(true)`, plus a guard that kills the whole group (`rustix`) when the run is dropped,
  so a stopped task leaves no helper behind. One error type: the tool was not found (naming it), it
  failed (the last `ERROR:` line of stderr, else the last non-empty line of stderr, else the last
  non-empty line of stdout, which is where `diskutil` writes its reason, else the exit code), or
  an I/O error. Used for `yt-dlp`, `ffmpeg`, `diskutil` and `mise`; `sdl-youtube/src/ytdlp.rs` and
  the spawn code in the converter, `sdl-devices` and the binary go. The tools check (F13) looks
  along `PATH` with the `which` crate and moves out of the binary into `sdl-tasks`.
- **H2. Landing** (`sdl-core::landing`): one function puts a finished temp file at its place and
  never replaces a file. Two modes: the library (`tempfile`'s `persist_noclobber`) and a device or
  Convert destination (check the place is free, then rename: FAT32 refuses the first, os error
  45). Both answer "written" or "already there"; "already there" is never an error. One function
  writes a small text file whole (`tempfile`: a temp file in the same folder, synced, then renamed
  over the place; a failed write leaves no temp file), used for
  `state.toml`, `review.toml` and every playlist file. Replaces `progress::place`, `copy_in` in
  `sdl-devices`, the landing inside `convert_file`, the body of `storage::save` and the three
  `std::fs::write` calls in `playlists`. **Fix:** in a device sync that converts, a song whose place
  is taken mid-run counts as skipped, not failed.
- **H3. Selection** (`sdl-library::selection`): one function turns "Whole library and these
  playlists" into the library's files, each once: the whole library first (F7, walked with
  `walkdir` at any depth, dot names skipped, links not followed), then each ticked playlist's songs in list order. A
  playlist line counts when it is three plain path components, none starting with a dot, and a
  file with its stem exists in any format. Used by the device plan and the Convert plan; both
  plans scan on a blocking thread.
- **H4. Run driver** (`sdl-core::run`): one outcome type, one tally and one driver for every
  function's run: jobs run a set number at a time (`futures` `buffer_unordered`), `Counts` are sent
  at the start and after every job, an `Attention` for every held or failed one. A job's error
  ends the run: no further job starts and the ones in hand are dropped. A cooperative stop
  (`tokio_util::sync::CancellationToken`, which replaces `StopFlag`) lets the jobs in hand finish
  and starts no other. Replaces `progress::drive` and the loops in `sdl-library::convert::run` and
  `sdl-devices::run`. What ends a run is decided where the error is typed (H5: a full
  destination, a missing tool), never by a loop reading error text. Outcomes known before the run
  (a link that could not be read) are counted first.
- **H5. Converter result** (`sdl-library::converter`): `convert_file` runs `ffmpeg` through H1 and
  lands through H2. Its result is typed: written, already there, the destination is full, or this
  file failed (with the reason); a missing `ffmpeg` is its own error. "Full" is recognised in this
  one place (the I/O error kind, or ffmpeg's "No space left on device"). **Fix:** Convert onto a
  full disk ends the run with a plain message, as Sync device does.
- **Removed with this work:** `Scene.running`, `running_in` and `Kind::Running` in
  `sdl/src/screen/effects.rs`; `RowView.last` and `LastView` in `sdl/src/app/rows.rs`;
  `App::shows_running_panel` is renamed for what it asks (whether a function runs).
- **Order of work**, each step passing the gate by itself: removals; H1; H2; H3; H5; H4.
- **New dependencies:** `tokio-util`, `which`; `walkdir` is also used by
  `sdl-core`.
- **Not:** no change to what any function does or shows beyond the two fixes; no new formats; no
  `ffmpeg-sidecar`, `process-wrap`, `cap-std` or playlist crate (looked at 2026-11-22: none kills a
  process group on drop, checks containment more simply than the three-component rule, or reads
  plain `.m3u8` music playlists while maintained).

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
- Toolchain: fenix stable. The global cargo config (`~/.config/cargo/config.toml`) supplies the
  profiles, the rustflags, the lint levels (inside `build.rustflags`, so every build enforces
  them) and the clippy config. The repo's `.cargo/config.toml` and `.cargo/clippy/clippy.toml`
  are a copy of it without the paths that name one machine, so a clone stands alone; the two
  are kept the same. No `rustfmt.toml`, no `[lints]` table, no other overrides.
- Gate: `cargo fmt --all`, `cargo lint`, and
  `mise x -- cargo test --workspace --all-targets --all-features --locked` (tests use the real
  `ffmpeg`, which is on `PATH` only through mise). `cargo lint` is an alias in the cargo config
  that runs clippy over the whole workspace; no lint flag is typed on a command line.
- No `#[allow]` or `#[expect]` anywhere. Tests return `Result` and use `?`.
- Tests cover pure logic and file operations in temp directories. No invented API fixtures.

## mise
`.mise/config.toml` holds the tools (yt-dlp, ffmpeg, deno) and one task, `tui`, that runs
the release binary with them on `PATH`. cargo builds into `$SHIP_ROOT/outputs/cargo/target` (the
fleet cargo config), so the task runs `$SHIP_ROOT/outputs/cargo/target/release/sdl`.
