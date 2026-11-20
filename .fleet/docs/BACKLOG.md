# Backlog

Tasks waiting on a condition. Each names when it runs.

## Remove pre-existing macOS `._` files from devices (one time, by hand)
- **When:** after the build is complete and its beta test has passed, on the next device sync.
- **What:** with the device mounted, list every `._*` file on it that `sdl`'s eject clean-up leaves behind, review the list with Yazid, and delete the macOS ones by hand. Done once per device.
- **Why:** eject clean-up removes a `._X` file only when `sdl` wrote `X` in that sync and the file carries the AppleDouble signature, so it can never delete a player's own files. `._` files left by earlier copies are outside that rule and stay until this task.

## Delete the old-flow music files (by hand)
- **When:** after the new build has synced Liked Songs into the `Artist/Album/Title` layout and Yazid has confirmed the library and a device sync work.
- **What:** delete `~/Music-old` (the old flow's `Spotify/`, `Youtube/` and `Soundcloud/` folders: 186 MP3s and a few link downloads), with Yazid, by hand.
- **Why:** they are kept as a fallback until the new approach is proven; `sdl` has no function that moves or deletes them.

## Raise playlist re-import with Disco
- **When:** after the first real device sync shows the behaviour on the Disc.
- **What:** raise upstream (zmd22/diskos-disco) that importing a changed `.m3u8` creates "<name> (2)" instead of updating the playlist, and ask for an update-in-place option.
- **Why:** every sync that changes a playlist leaves an extra copy on the Disc, which has to be deleted there by hand.
