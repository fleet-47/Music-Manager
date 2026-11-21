# Handoff: Spotify-DL (`sdl`)

Read first, end to end: `.agents/AGENTS.md` (rulings), `.fleet/docs/SPEC-tui.md` (the build contract,
F1 to F13), `.fleet/docs/BACKLOG.md` (tasks waiting on a condition).

## Where it stands (2026-11-21, evening)
- `sdl` is a music library manager: eight crates, one Rust TUI. Everything is committed in jj.
- The gate passes: `cargo fmt`, `cargo clippy` with the Gen.v2 flags, 435 tests run as
  `mise x -- cargo test --workspace --all-features --locked`.
- Release binary: `$SHIP_ROOT/outputs/cargo/target/release/sdl`. cargo's output folder left the repo
  on 2026-11-21 (fleet cargo config in `~/.config/cargo`); `mise run tui` runs that path. The repo's own
  `target/` (9.7 GB) is stale and can be deleted on Yazid's word.
- Built today and **never reviewed or run by an agent against real tools**: libraries (F12), the card
  home screen (F1), the tools check (F13). Yazid is testing by hand in a Rex tab.
- Yazid's own state: `~/Music/Prem/` is a friend's library with two songs; `~/Music/Me/` does not exist
  yet; the old flow's files are in `~/Music-old`; `client_id` in `~/.config/sdl/config.toml` is empty.

## Open in the code, in order

### 1. The Wi-Fi fault (not found)
On 2026-11-21 at 19:40 Yazid ran Download link, saw his whole Wi-Fi connection suffer, pressed Stop
(the run was saved as "stopped after 0"), and the connection recovered only when the TUI was killed.
- Ruled out with evidence: unlimited parallel downloads (capped at `download_jobs`, default 4, in
  `sdl-tasks/src/tasks/{link,spotify,review}.rs`); more than one yt-dlp helper (all calls go through
  `sdl-youtube/src/ytdlp.rs`); a retry loop in our code or a background task in `innertube-rs`; this
  yt-dlp build running as two processes (measured: one).
- Fixed since, because it was real whether or not it was the cause: Stop killed yt-dlp but not the
  programs yt-dlp starts (deno, ffmpeg). yt-dlp now leads its own process group and the whole group
  is killed (`ytdlp.rs`, with a test that fails without the fix).
- **Not known:** what was on the network during that run. The app keeps no log.
- Next step: get from Yazid the link he pasted and whether other devices lost Wi-Fi too. Reproduce
  with that link in a temp HOME while recording, each second, the app's process tree, its open
  connections (`lsof -i`) and the bytes on `en0`, before and after Stop. Do not name a cause without
  that measurement.

### 2. Bulk links in Download link (wanted by Yazid, no blocker)
- Today the form takes one link. Yazid asked for several at once.
- Shape: the Link field takes several links separated by spaces or line breaks; each must pass the
  existing checks (http or https, not Spotify); the task lists each link in turn and runs all their
  items as one run with one total; each playlist link still writes its own playlist file.
- Watch for: a pasted line break is dropped today, which would join two links into one (it must
  become a separator); the field's message must say which link is wrong; the "For me" choice applies
  to the whole batch.
- Needs a spec change in F5 before code.

### 3. Review of today's unreviewed code
An Opus red team of the libraries change and the home-screen rebuild was planned and put on hold by
Yazid. Ask before sending one. If it runs: scope it to F1, F12 and F13, give it a short core brief
(crates, commands, rules) and the exact files, and skip device runs unless the change touches them.

### 4. Small loose ends (each under about 50 lines)
- A new library whose name is already taken: the message shows in the form's "missing" lines, not
  under the name field. `sdl-form`'s `Field::validate` takes a plain `fn(&str)` and cannot know the
  existing libraries; it needs a settable message per field (`sdl-form/src/field.rs`, used from
  `sdl/src/dialogs/libraries.rs`).
- Dead code from the old running panel: `Scene.running` and `running_in` in
  `sdl/src/screen/effects.rs` (always `None` now), `RowView.last` and `LastView` in
  `sdl/src/app/rows.rs`, and the name `App::shows_running_panel`.
- The home screen's footer says "Enter run" while the chosen card says "Enter sync" (or "paste a
  link", "review", "convert"): one wording.
- The Sync device card shows its purpose line when a last run exists but no device is remembered.
- The tools check looks along `PATH` with a few lines of our own code; the `which` crate does this.
- The test for "Stop at the instant a run starts" does not reliably fail on the old bug.

### 5. Decisions waiting for Yazid
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
