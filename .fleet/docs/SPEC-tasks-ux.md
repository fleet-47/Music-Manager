# Spec: task layout and UI/UX

Revised after the Opus red team. Rulings in `.agents/AGENTS.md` bind this spec; where they conflict, AGENTS.md wins.

## Goal
Each task shows the underlying tool's own live output in the user's real terminal (scrollback and copy intact). mise features over shell; no extra launcher tool.

## Decisions
- Tasks stay in `.mise/config.toml`. File tasks were rejected: their script bodies don't render Tera templates (`{{ vars.* }}`, `{% if usage.* %}`).
- Launcher: mise's built-in task picker (`mise run` with no task; no task is aliased `default`). TUIsage rejected: its embedded terminal keeps no scrollback (`execution.rs:82-86`) and captures the mouse.
- Visible tasks: `download`, `sync`, `snowsky-sync`. Everything else is `hide = true`.
- `[settings] task.output = "interleave"`, `task.quiet = true`.
- Spotify two-step: `spotify:list` (secrets, `spotdl save`) then a credential-free `spotdl download lists/spotify.spotdl`. The download steps therefore hold no secrets, so redaction never conflicts with giving them the terminal.
- Tool flags: spotDL `--print-errors`; rclone `--progress`; no spotDL `--archive` (file-exists skip already precedes the YouTube search, `downloader.py:564` vs `:710`); no `gum`.

## Open, to measure in the user's terminal
- `raw = true` is added to a task only if its tool shows no live progress under `interleave` (spotDL, yt-dlp, rclone each checked once).
- Whether `snowsky:unmount` (post-dependency) still runs after Ctrl+C during `snowsky-sync`.

## Acceptance
1. `mise tasks validate` passes.
2. `mise run` opens the picker listing only the three visible tasks.
3. `mise run sync` makes one listing call through the dev app, then downloads with no credentials; folders unchanged (`Spotify/Saved tracks/`, one folder per playlist).
4. Existing behaviour unchanged: destinations, `library` prompt, card mount, cleanup and unmount.
