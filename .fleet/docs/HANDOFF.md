# Handoff: Spotify-DL (`sdl`)

Read first, end to end: `.agents/AGENTS.md` (rulings), `.fleet/docs/SPEC-tui.md` (the build contract,
F1 to F13), `.fleet/docs/BACKLOG.md` (tasks waiting on a condition).

## Where it stands (2026-11-24)
- `sdl` is a music library manager: eight crates, one Rust TUI. `main` on GitHub is at `51a1abbe`.
  The work of 2026-11-24 is committed locally and **not pushed**: Yazid asked for the commits, not
  a push.
- The gate passes: `cargo fmt --all`, `cargo lint`, and
  `mise x -- cargo test --workspace --all-targets --all-features --locked` (523 tests, 2 ignored in
  `sdl-devices/src/reality.rs`).
- The release binary at `$SHIP_ROOT/outputs/cargo/target/release/sdl` was built on 2026-11-24
  from the committed code, after Yazid's Liked Songs sync finished (1,881 songs in `~/Music/Me`).
  It has not been started yet.
- **IPv6 is off on this Mac's Wi-Fi** (`networksetup -setv6off Wi-Fi`, 2026-11-23, with Yazid's
  approval). The new binary no longer needs that; turn it back on with
  `networksetup -setv6automatic Wi-Fi` after asking Yazid.

## What moved on 2026-11-23 and 24
- Fault found: MusicBrainz's IPv6 address did not answer from Yazid's network, and `ureq` (inside
  `musicbrainz_rs`) has no connect timeout, so every request waited the system's 75 seconds before
  trying IPv4. All four download slots sat on finished songs; a sync fell to about 1 song a minute.
- Fixed and pushed: MusicBrainz addresses are tried IPv4 first, IPv6 as the fallback.
- Downloads and enrichment are fully split. A download tags the song with the source's fields
  and cover, lands it, and queues it in `~/.config/sdl/pending_enrichment.toml`. A background
  worker (`crates/sdl-tasks/src/enrich.rs`) adds genre, track total and disc total to the file in
  the library at MusicBrainz's pace. It shows nothing on screen and ends with the app.
- The queue is the list of songs missing a genre: a song stays until its file has one. When
  MusicBrainz has none (or the song has no ISRC) it is kept, marked as asked, and left alone.
  `Runner::ask_enrichment_again` makes asked songs wait again and `Runner::enrichment_counts`
  counts both kinds: the connection point for an enrichment command. Nothing calls them yet.
- MusicBrainz requests carry the User-Agent
  `sdl/<version> ( https://github.com/fleet-47/Music-Manager )`.
- Measured: MusicBrainz's ISRC lookup refuses `genres` and `release-groups` (its documentation
  says otherwise), so a song still costs a search and a recording lookup.

## Open, in order

### 1. First live run of the split (nothing below has run in the real app)
With the new release binary: sync a few new songs and check that they land at once without a
genre, that `pending_enrichment.toml` fills, that songs with a genre leave it and the rest stay
marked `asked = true`, that the files gain genre and totals and still play, and that quitting
mid-queue and starting again carries on. The worker's wake-up, the 60-second retry after a failed
request and its end at quit are covered only by unit tests of their parts. The new MusicBrainz
client (IPv4 first, new User-Agent) has made no real request.
- Not in the queue: songs that were in the library before 2026-11-24 and have no genre. Putting
  them in needs a scan of the library and a way to identify a song from its file (the queue
  identifies a song by its Spotify or video ID, which is not in the tags). Not designed.

### 2. Screen and interaction pass (asked for by Yazid, 2026-11-24)
Assess and adjust the TUI's UI and UX to fit the split, and to close the gaps the first live
runs showed. Known so far:
- Enrichment gets a card of its own with its command on it (Yazid, 2026-11-24): songs missing a
  genre are shown there, not as a note on a download, and the card's command asks again
  (`Runner::ask_enrichment_again`; counts from `Runner::enrichment_counts`). To be designed after
  the overall UI approach is settled, not before.
- Sync device gives no warning that songs copied now keep no genre on the device.
- The time estimate is vague and swings: it is the whole run's average, worded "about N min".
- The last-run line no longer says how many songs have no genre.
- Loose ends from before (item 6 below).
Events are tied to one of the five functions and the worker is not one, so its line on screen
needs a small design first.

### 3. Download pacing (ruled 2026-11-23, not designed or built)
Stay a guest on YouTube; run as many downloads as go through and pull back on YouTube's
rate-limit error ("This content isn't available, try again later"), then try more again.
`download_jobs` is still a fixed 4. The yt-dlp wiki gives about 300 videos an hour for a guest;
runs here went past that (339 in an hour on 2026-11-22, about 17 a minute on 2026-11-24).

### 4. Review candidates look wrong (seen 2026-11-23, not investigated)
The first held song in `review.toml` ("Bad, Bad Leroy Brown", Jim Croce) lists five unrelated
candidates. SUSPECTED: the candidates saved come from the ISRC search, not the artist-and-title
search. 143 songs were held on 2026-11-23; some may be matchable. Read
`crates/sdl-youtube/src/matching.rs` before naming a cause.

### 5. Test the 2026-11-23 device work on the Disc (nothing below has run on real hardware)
After a new release build: Sync device to the Disc and check the clean-up left no `.Trashes`,
`.fseventsd`, `.Spotlight-V100` or new `._` files; let the eject question count down; eject in Finder
during a countdown (expected: no error popup); eject without unplugging, open Sync device and mount
the "(not mounted)" card from the list, in both directions.
- Not confirmed on a real card: that an unmounted FAT volume reports the same UUID before and after
  mounting (remembered per-device settings depend on it); the `EFI` filter (taken from the man page);
  whether a card with no partition table is offered (the code does not list whole disks); whether an
  external macOS drive's system volumes show up as "(not mounted)".
- If the card stays mounted after a sync, macOS can put `.fseventsd` or `.Spotlight-V100` back before
  a Finder eject.
- The same-drive check and the countdown restart have not been reviewed by a second reader.
- `clean_up_ends_a_failed_run_too` (`sdl-tasks/src/tasks/device.rs`) hands the error to the wrap-up
  step directly: no real run failure can be produced in a test without `unsafe`.

### 6. Small loose ends (each under about 50 lines)
- A new library whose name is already taken: the message shows in the form's "missing" lines, not
  under the name field (`sdl-form/src/field.rs`, used from `sdl/src/dialogs/libraries.rs`).
- The home screen's footer says "Enter run" while the chosen card says "Enter sync" (or "paste a
  link", "review", "convert"): one wording.
- The Sync device card shows its purpose line when a last run exists but no device is remembered.
- The test for "Stop at the instant a run starts" does not reliably fail on the old bug.
- The quit question reads "Quit and stop it?" when what runs is an eject it will wait for.
- No connect timeout on MusicBrainz: if IPv4 is the dead route and IPv6 the live one, the
  75-second wait returns. Offered to Yazid on 2026-11-23, not ruled.

### 7. Still open from the 2026-11-22 pre-release review
- Two writers on one device (Convert to a card while Sync device runs to it) are not refused.
- A list's playlist file is only kept apart within one run; same-named playlists from separate runs
  still share a file. Closing it needs a stored record of which list owns which file.
- An untagged file brought in from a card's `Music` folder lands as `Unsorted/Music/…` and a later
  sync copies it back as `Music/Unsorted/Music/…`.
- Small: one song can land in two formats when two functions take it at the same moment; a Review
  correction to a playlist can be undone by a sync writing that playlist at that moment; places that
  differ only in letter case count as two songs.
- Waiting for a measurement of yt-dlp's real answers (`sdl-youtube/src/link.rs`): several artists
  arriving as one "A, B"; items of a YouTube Music playlist link; a song link with `&list=`;
  `-f bestaudio` where only combined streams exist.
- The Wi-Fi fault of 2026-11-21 is closed unless it happens again. If it does, record the app's
  process tree, its connections (`lsof -i`) and the bytes on `en0` before and after Stop, with the
  link that was pasted, before naming a cause.

### 8. Opus on the Disc (Disco 3.6.1): covers and Now Playing
Measured with Disco's own scanner on a FAT32 image: title, artist, album, album artist and track are
read correctly from the library's Opus files. The faults are in Disco:
- Covers: Disco's art decoder reads MP3, FLAC, M4A and WAV only (`ui/tools/diskos_artdec.c:14` in
  `Snowsky-DAC/disco-v3.6.1/src`), so no cover inside an Opus file is shown.
- Now Playing: Yazid sees the file name with `.opus` as the title and no album. The stock player
  sends the bare file name for Opus; Disco's swap to the tag title (`ui/ipc.c:113`) is not taking
  effect on his Disc and no swap exists for album. The cause is not found; it needs the Disc's
  `song.db`.
- Yazid is open to patching Disco and keeping the patch as stacked jj changes. It needs a jj clone
  of `zmd22/diskos-disco`, Disco's MIPS cross-build, and a way for `disco:update` to sign a local
  build. Not started; waiting for Yazid to open it as its own thread.
- Without a patch, "Format on device: M4A" gets covers at the cost of a re-encode.

### 9. Decisions waiting for Yazid
- Push the commits of 2026-11-24 to GitHub.
- MusicBrainz request count: keep exact genres (two requests a song plus one per album, the
  current code) or take the genre from the search's top tag (one request, less exact). YouTube's
  limit sits below MusicBrainz's either way.
- Whether the repo's `.cargo/config.toml` should be a literal copy of the global one.
- How `sdl` is installed and started, after the full live run.
- The 36 agent decisions listed in chat on 2026-11-21 are parked until after live testing.

## The full live run (final gate before a release build)
Done live by Yazid: Spotify sign-in, Liked Songs syncs, one device sync and a Finder eject.
Still to do: quit and restart without the browser opening; Download link (one YouTube Music song, a
playlist, a link for a friend's library); Review a held song; on the Disc, update the library and
import playlists; change a playlist, sync again and see how the Disc shows it (expected: a "(2)"
copy); items 1 and 5 above.

## Carried forward (not part of this build)
- **Global secret-provisioning task** (requested 2026-11-17): a global mise task that either creates a
  new Bitwarden item or takes an existing item name, and provisions it for the current project
  (reference in `.fnox.toml` plus the fnox sync cache). Findings: mise's global tasks dir here is
  `~/.config/mise/tasks/` and global tasks cannot list `secrets`; mise has no secret-provisioning
  command; fnox cannot add a vault reference and its Bitwarden provider is read-only, so creating
  items needs `bw create item`; one task process can run
  `BW_SESSION="$(bw unlock --raw)" fnox sync …; bw lock` without exporting the session. Unanswered:
  plain file or fleet flake (`bridge/`); task name. Requirement (2026-11-18): two pathways, select
  an existing Bitwarden secret, or generate one stored locally and in Bitwarden, both zero-trust.
  **Open question for Yazid: what "zero trust" means here.** Proposed reading, not confirmed: secret
  values never appear in agent context, command arguments, shell history, exported environment
  variables or plaintext files; a generated value is created inside the task process and piped
  straight to Bitwarden and to fnox's Secure Enclave cache; the Bitwarden session key lives only
  inside the one task process, which locks the vault on exit.
- `sdl` itself uses no secret: PKCE sign-in with the client ID only, refresh token in the Keychain.

## Working on this repo
- Toolchain: fenix stable at `/nix/store/hpkyhn8x0bjzr735qf5nh2xwng5wqdrp-rust-stable-2026-10-15/bin`
  (not on `PATH`). No nix commands without approval. Tests need `mise x --` (real ffmpeg).
- Never type lint flags on a cargo command; the gate is `cargo lint`, which runs `--locked`. After
  a change to a `Cargo.toml`, `cargo metadata --offline --format-version 1` brings `Cargo.lock` in
  line.
- Never run `cargo build --release` or touch `~/Music` and `~/.config/sdl` while an `sdl` is running.
  Reading them is fine.
- VCS is jj; a hook blocks `git` commands, so use `jj` and `gh`. A hook refuses commit messages over
  20 words, and it blocks the whole shell command the commit is in, so commit in a command of its
  own. Described commits are immutable: fixes go in new changes.
- Driving the real binary: never activate the Sync Spotify card (on 2026-11-20 an agent opened
  Yazid's browser on Spotify's sign-in page), use a temp HOME, and check `diskutil list external`
  before opening Sync device. No agent runs a mutating `diskutil` or `hdiutil` verb on a real drive.
- Sizing the process to the change (Yazid, 2026-11-21): small fixes are one builder and a gate run
  by the orchestrator; one feature is one or two builders on a short shared brief; contracts, waves
  and Opus gates are for rebuilds only. Builders that touch the same files run one after another.
- Before a change that touches several helpers, Yazid wants a short architectural review first:
  what exists for each step, what is added, what is reused, what is removed, which tests are bound
  to the old behaviour (2026-11-23).
- Diagnosis: check the surface Yazid names first, measure before naming a cause, and say plainly
  when a cause is not found.
