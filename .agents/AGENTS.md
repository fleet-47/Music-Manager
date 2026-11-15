# Spotify-DL

Local music library of Yazid's Spotify Liked Songs and playlists, built with spotDL via project-local mise.

## Rulings
- mise config lives in `.mise/` (`.mise/config.toml`), not a root `mise.toml`.
- All mise files are committed to the repo; never git-ignore them.
- Secrets live in Yazid's Bitwarden vault and reach tasks only through mise secrets (`[secrets.fnox]`, fnox's Bitwarden provider, per-task `secrets = [...]`). Never write them to a file in plain text. Exception, ruled: fnox's sync cache (`fnox sync --provider sync-age --local-file`) keeps them encrypted to this Mac's Secure Enclave key (Touch ID per read) in the git-ignored fnox local file, so `mise run` needs no `BW_SESSION`. Re-sync from Bitwarden when the credentials change.
- Spotify credentials: one Bitwarden login item, `Spotify-DL` (username = client ID, password = client secret). Uses Yazid's own developer app (Yazid has Premium) with `--user-auth`.
- Scope that matters: Liked Songs and Yazid's private playlists. Followed playlists are not a requirement.
- spotDL's Spotify sign-in token may be cached only for a limited period, never indefinitely. Decided: `--no-cache` (token in memory for one run; browser sign-in each run). spotDL has no env-held session option; `--cache-path` would still write the token to a file.
- fnox config is `.fnox.toml` at the repo root (hidden); fnox does not search `.mise/`.
- Snowsky sync: `rclone bisync` between `library/` and the card's `Music/`. bisync state lives on the card in a visible `bisync/` folder at the card root (never a dot-folder: Disco's scanner, `disco-v3.6.1/src/ui/scanner.c:1607-1621`, disables pruning of vanished songs when it meets any dot-directory except root `.diskos`). The scanner indexes audio extensions only, so `bisync/` is never treated as music.
- `snowsky-sync` deletes the macOS dot-folders (`.Spotlight-V100`, `.Trashes`, `.fseventsd`) from the card when present, and prevents them with a docs-sourced method where one exists.
- Spotify pathway is two-step for every Spotify download: `spotify:list` (the only task granted the Bitwarden secrets) writes `lists/spotify.spotdl` (git-ignored, regenerated each run), then spotDL downloads from that file with no credentials and no Spotify calls (a complete entry is never refetched, `downloader.py:478-491`).
- `spotify:list` is our own lister, not `spotdl save` (ruled 2026-11-18): it makes only the paged listing calls through the dev app and writes the `.spotdl` entries itself. Reason: `spotdl save` refetches 3 calls per song through the dev app (`console/save.py:43` → `utils/search.py:107`), which hit the dev-mode limit (24 h lockout) on the 2,117-song Liked Songs run. Accepted cost: the listing carries no genres or disc count, so the genre tag is empty and the disc count is a placeholder.
- The mise tasks are to be replaced by one Rust TUI (ruled 2026-11-18). Every backend is a pure-Rust crate (`ratatui`, `rspotify` with PKCE sign-in, `musicbrainz_rs` for genre and disc totals, `lofty` for tags, `walkdir` plus macOS `diskutil` for the card; DiskArbitration bindings need `unsafe`, which the build standard forbids), with one exception: search, download and format handling go through the `yt-dlp` crate, which wraps the yt-dlp binary. No linked bindings (no `librclone`, no ffmpeg libraries). Reason: no pure-Rust crate generates YouTube's anti-bot tokens, so pure-Rust extraction depends on a workaround YouTube can close. The lister ruling above is met by `rspotify` inside the TUI. Sign-in is PKCE (ruled 2026-11-18): the TUI uses only the client ID, which is not a secret and lives in the TUI's config file; the client secret is never given to it, tokens stay in memory, and only the scopes `user-library-read` and `playlist-read-private` are requested. fnox and Bitwarden are therefore not used by the TUI. mise starts the TUI with one task and keeps supplying the pinned yt-dlp, ffmpeg and deno. Build spec: `.fleet/docs/SPEC-tui.md`.
- Tasks stay in `.mise/config.toml` (no `.mise/tasks/` file tasks: file-task bodies don't render templates). Launcher is mise's built-in task picker; no TUIsage. UX target: `.fleet/docs/SPEC-tasks-ux.md` as revised after the red team.
- Tasks use mise's own features (dependencies, checks, sequences, arguments, tools), not shell scripts with hand-rolled checks.
- One download entry point, `mise run download`: a router for a single link (track or playlist). Spotify → spotDL; everything else → yt-dlp.
- Downloads land in `library/<Source>/`. yt-dlp's site name (`%(extractor_key)s`) is the folder name, spelled as yt-dlp spells it (e.g. `Youtube`, `Soundcloud`).
- A playlist link gets its own subfolder named after the playlist; a single-track link lands flat in the source folder.
- `library` is a per-device symlink to the music root, git-ignored, never committed. The hidden `library` task (a dependency of every download and sync task) asks for the path, default `~/Music`, when the link is missing.

## Layout
- `.mise/config.toml`: settings, tools (spotDL, conda ffmpeg, deno, yt-dlp, fnox, bitwarden, rclone; unpinned), vars and all tasks. mise installs the spotDL binary as `spotify-downloader`, not `spotdl`.
- `.fnox.toml`: Bitwarden item references only (`Spotify-DL` username/password); no values.
- `.rclone-filters` (+ `.rclone-filters.md5`, written by bisync): Mac clutter excluded from `snowsky-sync`. Editing it requires `mise run snowsky-sync --resync`.
- `lists/spotify.spotdl`: track list written by `spotify:list`; git-ignored, regenerated each run.
- `library` → music root (per device). Spotify lands in `library/Spotify/` (one folder per playlist; Liked Songs in `Saved tracks`).

## Tasks (visible in the `mise run` picker)
- `mise run download`: prompts for one link and routes it (Spotify → `spotify:list` + credential-free spotDL download; anything else → yt-dlp MP3, VBR best, embedded metadata and cover, FAT-safe names).
- `mise run sync`: Liked Songs + all user playlists into `library/Spotify/`. Adds new tracks, never deletes.
- `mise run snowsky-sync [--resync] [--dry-run]`: mounts the card if needed, bisyncs, then asks to clean macOS dot-folders and unmount.

## Facts
- Audio is sourced from YouTube Music (yt-dlp bundled in the spotDL binary), not Spotify. Lossy quality is accepted.
- Spotify app redirect URI: `http://127.0.0.1:9900/`. `spotify:list` opens a browser sign-in each run (`--no-cache`). Unlock Bitwarden first in your own terminal: `export BW_SESSION="$(mise x -- bw unlock --raw)"`; after creating or editing the item, `mise x -- bw sync`.
- Spotify dev-mode apps lock out for ~24 h after too many calls; only `spotify:list` uses the app.
- spotDL takes credentials only as flags, so they are visible in the process list while `spotify:list` runs.
- If downloads break after YouTube changes, the fix arrives via a new spotDL release (`mise upgrade`).
