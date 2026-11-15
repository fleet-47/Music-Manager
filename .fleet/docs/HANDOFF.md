# Handoff

Re-entry: read `.agents/AGENTS.md` (rulings, layout, tasks, facts) and `.fleet/docs/SPEC-tasks-ux.md`, then this file.

## What moved (2026-11-15 → 2026-11-18)
- Toolchain and all tasks live in `.mise/config.toml` (mise secrets via fnox → Bitwarden item `Spotify-DL`). `mise tasks validate` passes; the `mise run` picker shows `download`, `sync`, `snowsky-sync`.
- `download` (yt-dlp path): working; two SoundCloud downloads verified.
- `snowsky-sync`: working. First `--resync` done, card unmount cleanup verified on a read-only remount (no dot-folders left), Disco picked the files up cleanly.
- Liked Songs: 186 of 2,117 downloaded to `~/Music/Spotify/Saved tracks/` before the dev app hit Spotify's dev-mode limit (24 h lockout, ends ~2026-11-18 19:05).
- Spotify pathway restructured to two steps (`spotify:list` with secrets → credential-free `spotdl download lists/spotify.spotdl`). One-song test of the credential-free client passed during the lockout, with full tags.
- fnox sync cache added to close the `BW_SESSION` gap: `github:remko/age-plugin-se` is a project tool; the global `sync-age` provider (Secure Enclave key) already exists; `.fnox.local.toml` is git-ignored.

## Open
1. **Yazid: one-time fnox sync** (not yet run; no cache file exists). In your own Terminal, in the repo: `export BW_SESSION="$(mise x -- bw unlock --raw)"` then `mise x -- fnox sync --provider sync-age --local-file`. Verify from a fresh tab: `mise x -- fnox check --all` should prompt Touch ID and pass with no `BW_SESSION`.
2. **Re-attempt `mise run sync`** after the lockout ends (~19:05 on 2026-11-18). Note per tool whether live progress shows (spotDL here; yt-dlp via `download`; rclone via `snowsky-sync --dry-run`) and add `raw = true` only where it doesn't (SPEC "Open").
3. **Red-team the Spotify call volume before the next large run.** Yazid's question: can everything be pulled from Spotify once and reused? Established facts (spotDL 4.6.0 source):
   - `spotify:list`: about one call per 50 Liked Songs (`types/saved.py:44-56`), plus playlist listings for `all-user-playlists`.
   - The download step then refetches 3 calls per song (track, artist, album) through spotDL's no-credentials client, because list entries lack `genres` and `disc_count` (`download/downloader.py:476-494` → `types/song.py:86-102`).
   - That refetch runs before the file-exists skip (`downloader.py:494` vs `:564`), so every `sync` repeats it for songs already on disk: about 6,400 public calls per Liked Songs run.
   - To evaluate (unverified): avoiding the refetch altogether; spotDL `--use-cache-file` (stores Spotify responses under `~/.spotdl/.spotify_cache`); rate limits of the no-credentials client; whether the saved list could carry full metadata once.
4. **Global secret-provisioning task** (requested 2026-11-17): a global mise task that either creates a new Bitwarden item, or takes an existing item name, and provisions it for the current project (reference in `.fnox.toml` + fnox sync cache). Findings so far:
   - mise global tasks dir on this Mac is `~/.config/mise/tasks/` (`Fleet/Config` → `~/.config`); `~/.mise/tasks/` is not read globally. Global tasks cannot list `secrets`.
   - mise has no secret-provisioning command (`mise secrets` has only `ls`; `[bootstrap.secrets]` is for bootstrap templates).
   - fnox has no command to add a vault reference; its Bitwarden provider is read-only (`fnox set` writes only to writable providers). Creating items needs Bitwarden's documented `bw create item` pipeline.
   - A single task process can run `BW_SESSION="$(bw unlock --raw)" fnox sync …; bw lock` without exporting the session.
   - Still unanswered: whether the task lives as a plain file or in the fleet flake (`bridge/`); task name; recommendation pending a full mise/fnox doc review.
   - Requirement stated 2026-11-18: two pathways, (a) select an existing Bitwarden secret, (b) generate a secret that is stored locally and in Bitwarden, both with zero-trust generation and handling. The fnox sync of item 1 is to be done by this task, not by the manual `export BW_SESSION` steps.
   - **Open question for Yazid: what "zero trust" means here.** Proposed reading, not yet confirmed: secret values never appear in agent context, command arguments, shell history, exported environment variables or plaintext files; a generated value is created inside the task process and piped straight to Bitwarden and to fnox's Secure Enclave cache; the select pathway writes only the reference and the encrypted cache; the Bitwarden session key lives only inside the one task process, which locks the vault on exit.
5. **Card capacity:** Liked Songs is about 10 GB (about 4.6 MB/song); the card has 7.4 GB free. `snowsky-sync` will fail partway once the library outgrows it. Decision needed: bigger card, or a subset sync.
6. **Not yet tested:** `snowsky:unmount` after Ctrl+C during `snowsky-sync` (on hold by Yazid's choice); recovery from interrupted runs (on hold).

## Carried forward
- Nothing is committed to jj yet.
- Leftovers on the card: two `bisync/*.lst-dry` files from the first dry run; macOS `._*` files (hidden files, not folders, so they don't affect Disco).
- spotDL credentials appear in the process list while `spotify:list` runs (spotDL takes them only as flags).
