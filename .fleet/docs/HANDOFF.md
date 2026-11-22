# Handoff: Spotify-DL (`sdl`)

Read first, end to end: `.agents/AGENTS.md` (rulings), `.fleet/docs/SPEC-tui.md` (the build contract,
F1 to F13), `.fleet/docs/BACKLOG.md` (tasks waiting on a condition).

## Where it stands (2026-11-22)
- `sdl` is a music library manager: eight crates, one Rust TUI. Everything up to the keyboard-focus and several-links
  work is committed and pushed to GitHub; the docs for the shared-helpers build are not yet.
- The gate passes: `cargo fmt`, `cargo clippy` with the Gen.v2 flags, 481 tests run as
  `mise x -- cargo test --workspace --all-features --locked`.
- Release binary: `$SHIP_ROOT/outputs/cargo/target/release/sdl`. cargo's output folder left the repo
  on 2026-11-21 (fleet cargo config in `~/.config/cargo`); `mise run tui` runs that path. The repo's own
  `target/` (9.7 GB) is stale and can be deleted on Yazid's word.
- Reviewed by agents on 2026-11-22 but **never run by an agent against real tools**: libraries (F12), the card
  home screen (F1), the tools check (F13). Yazid is testing by hand in a Rex tab.
- Yazid's own state: `~/Music/Prem/` is a friend's library with two songs; `~/Music/Me/` does not exist
  yet; the old flow's files are in `~/Music-old`; `client_id` in `~/.config/sdl/config.toml` is empty.

## Open in the code, in order

### 1. The Wi-Fi fault (provisionally resolved, 2026-11-22)
On 2026-11-21 at 19:40 Yazid ran Download link, saw his whole Wi-Fi connection suffer, pressed Stop
(the run was saved as "stopped after 0"), and the connection recovered only when the TUI was killed.
- The fix: Stop killed yt-dlp but not the programs yt-dlp starts (deno, ffmpeg). yt-dlp now leads
  its own process group and Stop kills the whole group (now `sdl-core/src/process.rs`, with a test
  that fails without the fix). Every yt-dlp call goes through that one function.
- Confirmed 2026-11-22 with the real yt-dlp (2026.10.26) on a YouTube Music link: its `deno` helper
  runs in yt-dlp's process group, and killing the group left no yt-dlp, deno or ffmpeg process.
- Convert and Sync device stop differently by design (spec F10): the `ffmpeg` conversions in hand
  finish, then the run stops. They use no network.
- The cause of the fault itself was never measured. Yazid's ruling: closed unless it happens again.
  If it does, record the app's process tree, its connections (`lsof -i`) and the bytes on `en0`
  before and after Stop, with the link that was pasted, before naming a cause.

### 2. The pre-release build (shared helpers plus the review fixes, 2026-11-22; waiting for Yazid's hands-on test)
- Shared helpers built to the spec's "Shared helpers" section, then reviewed by five Opus agents
  (one per area), who fixed their own findings. Gate after the fixes: fmt, clippy with the Gen.v2
  flags, 481 tests pass, 2 ignored (`sdl-devices/src/reality.rs`). Release binary rebuilt 19:43.
- Not committed. Not run by an agent against real tools, a real device, a full disk or Spotify.
- Fixed in the review: quit waits for Convert and Sync device, and asks during an eject or tools
  install; hang-up and terminate signals take the quit path; one copy of `sdl` at a time
  (`~/.config/sdl/lock`); unreadable Spotify entries are counted on the summary line; two lists
  never share a playlist file within a run; the library is every audio file at any depth, links
  not followed; whole-file saves use `tempfile` (`atomic-write-file` removed); Review refreshes
  while another task holds songs; the sign-in listener no longer misses the code; a full disk
  during a tag write ends Convert; a device sync waits for copies in hand.
- Waiting for Yazid, from the review:
  - Two writers on one device (Convert to a card while Sync device runs to it) are not refused.
  - A list's playlist file is only kept apart within one run; same-named playlists pasted in
    separate Download link runs, or a Spotify list and a YouTube playlist of one name, still
    share a file. Closing it needs a stored record of which list owns which file.
  - Any-depth library: an untagged file brought in from a card's `Music` folder lands as
    `Unsorted/Music/…` and a later sync copies it back to the card as `Music/Unsorted/Music/…`.
  - Not fixed, small: one song can land in two formats when two functions take it at the same
    moment; a Review correction to a playlist can be undone by a sync writing that playlist at
    that moment; places that differ only in letter case count as two songs.
- Waiting for a measurement:
  - yt-dlp's real answers (`sdl-youtube/src/link.rs`): several artists arriving as one "A, B";
    items of a YouTube Music playlist link matched, not downloaded directly; a song link with
    `&list=` taking the whole list; `-f bestaudio` failing where only combined streams exist.
  - On a disk image: whether `.fseventsd` returns at unmount after clean-up removed it, and
    whether `._` files stay for names with accents (`sdl-devices/src/eject.rs`).
- The quit question still reads "Quit and stop it?" when what runs is an eject it will wait for.

### 3. Small loose ends (each under about 50 lines)
- A new library whose name is already taken: the message shows in the form's "missing" lines, not
  under the name field. `sdl-form`'s `Field::validate` takes a plain `fn(&str)` and cannot know the
  existing libraries; it needs a settable message per field (`sdl-form/src/field.rs`, used from
  `sdl/src/dialogs/libraries.rs`).
- The home screen's footer says "Enter run" while the chosen card says "Enter sync" (or "paste a
  link", "review", "convert"): one wording.
- The Sync device card shows its purpose line when a last run exists but no device is remembered.
- The test for "Stop at the instant a run starts" does not reliably fail on the old bug.

### 4. Decisions waiting for Yazid
- Whether `~/Music/Prem` is how he wants friends' libraries named, and whether to create his own
  library by moving anything into `~/Music/Me/`.
- How `sdl` is installed and started (proposed: `cargo install` into `$SHIP_ROOT/outputs/cargo/bin`
  and yt-dlp, ffmpeg, deno in mise's global config), after the full live run.
- Deleting the stale `target/` folder in the repo.
- The 36 agent decisions listed in chat on 2026-11-21 are parked until after live testing.

## The full live run (final gate before a release build)
1. Yazid adds the Spotify client ID to `~/.config/sdl/config.toml`.
2. `mise run tui`. Sync Spotify: sign in, tick one small playlist, Continue, Start.
3. Quit and start again: Sync Spotify must not open the browser.
4. Download link: one YouTube Music song, a playlist, and one link for a friend's library.
5. Review: resolve a held song.
6. Sync device to the Disc, eject, then on the Disc update the library and import playlists.
7. Change the playlist, sync again, and see how the Disc shows it (expected: a "(2)" copy).
Never run live so far: Spotify sign-in, token refresh and Keychain rotation; MusicBrainz at volume;
a real Disc importing playlists; a real drive eject.

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
- VCS is jj. A hook refuses commit messages over 20 words, and it blocks the whole shell command the
  commit is in, so commit in a command of its own. Described commits are immutable: fixes go in new
  changes.
- Driving the real binary: never activate the Sync Spotify card (on 2026-11-20 an agent opened
  Yazid's browser on Spotify's sign-in page), keep `client_id` blank in any test config, use a temp
  HOME, and check `diskutil list external` before opening Sync device.
- Sizing the process to the change (Yazid, 2026-11-21): small fixes are one builder and a gate run
  by the orchestrator; one feature is one or two builders on a short shared brief; contracts, waves
  and Opus gates are for rebuilds only. Opus reviews took 60 to 70 minutes each; ask before running
  one.
- Diagnosis: check the surface Yazid names first, measure before naming a cause, and say plainly
  when a cause is not found.
