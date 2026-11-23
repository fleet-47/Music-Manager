# Handoff: Spotify-DL (`sdl`)

Read first, end to end: `.agents/AGENTS.md` (rulings), `.fleet/docs/SPEC-tui.md` (the build contract,
F1 to F13), `.fleet/docs/BACKLOG.md` (tasks waiting on a condition).

## Where it stands (2026-11-23)
- `sdl` is a music library manager: eight crates, one Rust TUI. Everything is committed and pushed to
  GitHub except this handoff (`main` at `88cd248c` plus one local commit).
- The gate passes: `cargo fmt --all`, `cargo lint`, and
  `mise x -- cargo test --workspace --all-targets --all-features --locked` (509 tests, 2 ignored in
  `sdl-devices/src/reality.rs`).
- **The release binary is older than the code.** `$SHIP_ROOT/outputs/cargo/target/release/sdl` was
  built 2026-11-22 19:43 and holds none of the 2026-11-23 work. It was not rebuilt because Yazid's
  Liked Songs sync was running on it. Build a new one (`cargo build --release --locked`) once no
  `sdl` is running.
- Yazid ran the app live on 2026-11-22 and 23: Sync Spotify on Liked Songs (still running at close,
  959 songs in `~/Music/Me`, all Opus) and one Sync device to the Disc. A folder `~/Music/Music`
  exists and was not looked at.

## What moved on 2026-11-23
- Clean-up is its own step at the end of every Sync device run; eject is `diskutil unmount` only.
- The "Eject <device>?" question counts down from 30 seconds and ejects by itself; a covered
  question starts its countdown again when it is back on top.
- Clean-up and eject act only on the same drive (volume UUID, checked inside `sdl-devices`).
- Sync device lists plugged-in drives that are not mounted and mounts the chosen one at Continue.
- Lint levels moved into the global cargo config's `build.rustflags`, so every build on this Mac
  enforces them; the lint gate is `cargo lint`. The repo carries its own copy in `.cargo/`. Any other
  local Rust project with a warning now fails to build until the warning is fixed.
- Reviewed once by an Opus agent reading the code; its confirmed findings are fixed. The same-drive
  check and the countdown restart were written after that review and have not been reviewed.

## Open, in order

### 1. Test the 2026-11-23 device work on the Disc (nothing below has run on real hardware)
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
- `clean_up_ends_a_failed_run_too` (`sdl-tasks/src/tasks/device.rs`) hands the error to the wrap-up
  step directly: no real run failure can be produced in a test without `unsafe`.

### 2. Opus on the Disc (Disco 3.6.1): covers and Now Playing
Measured with Disco's own scanner on a FAT32 image: title, artist, album, album artist and track are
read correctly from the library's Opus files. The faults are in Disco:
- Covers: Disco's art decoder reads MP3, FLAC, M4A and WAV only (`ui/tools/diskos_artdec.c:14` in
  `Snowsky-DAC/disco-v3.6.1/src`), so no cover inside an Opus file is shown.
- Now Playing: Yazid sees the file name with `.opus` as the title and no album. The stock player
  sends the bare file name for Opus; Disco's swap to the tag title (`ui/ipc.c:113`) is not taking
  effect on his Disc and no swap exists for album. The cause is not found; it needs the Disc's
  `song.db`.
- Yazid is open to patching Disco and keeping the patch as stacked jj changes. Feasible: an Ogg
  picture reader in `diskos_artdec.c` and an album and title lookup in `ipc.c`. It needs a jj clone
  of `zmd22/diskos-disco` (`engineering/diskOS` tracks `b0hemia/diskOS`, which the Disc does not
  run), Disco's MIPS cross-build, and a way for `disco:update` to sign a local build. Not started;
  waiting for Yazid to open it as its own thread.
- Without a patch, keeping Opus is still the best audio quality; "Format on device: M4A" gets covers
  at the cost of a re-encode.
- Disco is current: v3.6.1 is the latest release and upstream `main` is four commits ahead, none in
  the scanner, covers or Now Playing.

### 3. Still open from the 2026-11-22 pre-release review
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

### 4. Small loose ends (each under about 50 lines)
- A new library whose name is already taken: the message shows in the form's "missing" lines, not
  under the name field (`sdl-form/src/field.rs`, used from `sdl/src/dialogs/libraries.rs`).
- The home screen's footer says "Enter run" while the chosen card says "Enter sync" (or "paste a
  link", "review", "convert"): one wording.
- The Sync device card shows its purpose line when a last run exists but no device is remembered.
- The test for "Stop at the instant a run starts" does not reliably fail on the old bug.
- The quit question reads "Quit and stop it?" when what runs is an eject it will wait for.

### 5. Decisions waiting for Yazid
- Whether the repo's `.cargo/config.toml` should be a literal copy of the global one. It leaves out
  the target folder, the install root and the linker and SDK paths, which name this Mac.
- How `sdl` is installed and started, after the full live run.
- The 36 agent decisions listed in chat on 2026-11-21 are parked until after live testing.

## The full live run (final gate before a release build)
Done live by Yazid: Spotify sign-in, a Liked Songs sync, one device sync and a Finder eject.
Still to do: quit and restart without the browser opening; Download link (one YouTube Music song, a
playlist, a link for a friend's library); Review a held song; on the Disc, update the library and
import playlists; change a playlist, sync again and see how the Disc shows it (expected: a "(2)"
copy); item 1 above.

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
- Never type lint flags on a cargo command; the gate is `cargo lint`. Cargo joins lists from the
  global and the repo's config files, so the `lint` alias is a string in both, and rustflags appear
  twice on this Mac (harmless).
- Never run `cargo build --release` or touch `~/Music` and `~/.config/sdl` while an `sdl` is running.
- VCS is jj; a hook blocks `git` commands, so use `jj` and `gh`. A hook refuses commit messages over
  20 words, and it blocks the whole shell command the commit is in, so commit in a command of its
  own. Described commits are immutable: fixes go in new changes.
- Driving the real binary: never activate the Sync Spotify card (on 2026-11-20 an agent opened
  Yazid's browser on Spotify's sign-in page), use a temp HOME, and check `diskutil list external`
  before opening Sync device. No agent runs a mutating `diskutil` or `hdiutil` verb on a real drive.
- Sizing the process to the change (Yazid, 2026-11-21): small fixes are one builder and a gate run
  by the orchestrator; one feature is one or two builders on a short shared brief; contracts, waves
  and Opus gates are for rebuilds only. Builders that touch the same files run one after another.
  An Opus read-only review of a three-change diff took about 8 minutes on 2026-11-23.
- Diagnosis: check the surface Yazid names first, measure before naming a cause, and say plainly
  when a cause is not found.
