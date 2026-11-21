# Handoff: Spotify-DL (`sdl`)

Read first, end to end: `.agents/AGENTS.md` (rulings), `.fleet/docs/SPEC-tui.md` (the build contract),
`.fleet/docs/BACKLOG.md` (tasks waiting on a condition).

## Where it stands (2026-11-21)
- `sdl` is rebuilt as a music library manager: eight crates, one Rust TUI. The old mise/spotDL flow is
  gone (jj history, change `nplromuk`).
- The gate passes: `cargo fmt`, `cargo clippy` with the Gen.v2 flags, and 386 tests run as
  `mise x -- cargo test` (tests use the real `ffmpeg`). Release binary:
  `$SHIP_ROOT/outputs/cargo/target/release/sdl` (cargo's output folder moved out of the repo on
  2026-11-21; the repo's old `target/` is stale). Everything is committed in jj.
- Proven by agents on this Mac, in temp folders only: Opus download from YouTube Music (stream-copied),
  SoundCloud link matched or held, the review list, playlist files against Disco's reader, all
  conversion presets, device sync both ways on FAT32 disk images, the room refusal, stop and quit
  mid-run, eject clean-up, idle at 0% CPU, and the real Keychain store (throwaway entry, deleted).
- **Never run live:** Spotify sign-in, token refresh and Keychain rotation; MusicBrainz at volume; a
  real Disc importing the playlists; a real drive eject.
- `~/Music` is empty and ready for the new layout. The old flow's files are in `~/Music-old`.
  `~/.config/sdl/config.toml` still has an empty `client_id`.

## Next: the full live run (final gate before a release build)
1. Yazid adds the Spotify client ID to `~/.config/sdl/config.toml`.
2. `mise run tui` from the repo. Sync Spotify: sign in, tick one small playlist, Continue, Start.
3. Quit and start again: Sync Spotify must not open the browser.
4. Download link: one YouTube Music song, then a playlist link.
5. Review: resolve a held song.
6. Sync device to the Disc, eject, then on the Disc update the library and import playlists.
7. Change the playlist, sync again, and see how the Disc shows it (expected: a "(2)" copy).

After a clean run: decide how `sdl` is installed and started (proposed: a global `sdl` command with
yt-dlp, ffmpeg and deno in mise's global config), then the Liked Songs sync (about 46 Spotify calls;
MusicBrainz makes it roughly two hours).

## Open
1. **Running-panel spinner.** The UI/UX gate put a spinner beside every running task, redrawing 10
   times a second for the whole run. Spec F1 gives a spinner only to a task with no known total, and
   Yazid ruled against constant redraws. Proposed: revert to the spec, with the one-second clock as
   the sign of life. Awaiting Yazid.
2. **Pasted Spotify links.** Spotify's API gives a developer-mode app the songs of a playlist only
   when the user owns it or collaborates on it, public or not (reference page "Get Playlist Items";
   February 2026 dev-mode changes). The route is to copy the playlist to one's own account and use
   Sync Spotify. Proposed: say exactly that in Download link's message. Awaiting Yazid.
3. **Agent decisions.** Yazid was given 36 one-line decisions agents took where the spec was silent
   (chat, 2026-11-21). Six were then fixed in the UI/UX gate; none of the rest is flagged yet.
4. **Known small limits:** the same video as a `music.youtube.com` link and a `youtube.com` link counts
   as two songs for "held" and "never"; a Keychain write that fails mid-run is not shown; a list filter
   cannot contain a space; a popup shifts when its "missing" lines disappear.
5. **Card capacity** (carried): the Disc's card had 7.4 GB free against a Liked Songs library of
   several GB. Sync device can now sync by playlist; whether to buy a bigger card is still Yazid's call.

## Carried forward (not part of this build)
- **Global secret-provisioning task** (requested 2026-11-17): a global mise task that either creates a
  new Bitwarden item or takes an existing item name, and provisions it for the current project
  (reference in `.fnox.toml` plus the fnox sync cache). Findings so far:
  - mise's global tasks dir on this Mac is `~/.config/mise/tasks/`; global tasks cannot list `secrets`.
  - mise has no secret-provisioning command; fnox has no command to add a vault reference and its
    Bitwarden provider is read-only. Creating items needs Bitwarden's `bw create item` pipeline.
  - A single task process can run `BW_SESSION="$(bw unlock --raw)" fnox sync …; bw lock` without
    exporting the session.
  - Unanswered: plain file or fleet flake (`bridge/`); task name.
  - Requirement (2026-11-18): two pathways, (a) select an existing Bitwarden secret, (b) generate a
    secret stored locally and in Bitwarden, both with zero-trust generation and handling.
  - **Open question for Yazid: what "zero trust" means here.** Proposed reading, not confirmed: secret
    values never appear in agent context, command arguments, shell history, exported environment
    variables or plaintext files; a generated value is created inside the task process and piped
    straight to Bitwarden and to fnox's Secure Enclave cache; the Bitwarden session key lives only
    inside the one task process, which locks the vault on exit.
- `sdl` itself uses no secret: PKCE sign-in with the client ID only, refresh token in the Keychain.

## For the next agent working on the code
- Toolchain: fenix stable at `/nix/store/hpkyhn8x0bjzr735qf5nh2xwng5wqdrp-rust-stable-2026-10-15/bin`
  (not on `PATH`). No nix commands without approval. VCS is jj; described commits are immutable here,
  so fixes go in new changes.
- When driving the real binary: never activate the Sync Spotify row (on 2026-11-20 an agent did so by
  mistake and Yazid's browser opened Spotify's sign-in page), keep `client_id` blank in any test
  config, and check `diskutil list external` before opening Sync device (Yazid plugs real drives in
  while agents work).
- The build pattern that worked: contracts written and reviewed first, then builders in waves with a
  red team after the first wave, then one unified red team with a device run, then a UI/UX gate, with
  the gate re-run and a commit between each.
