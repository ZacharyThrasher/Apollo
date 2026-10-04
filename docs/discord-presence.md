# Discord Rich Presence

Settings > Discord > **Show what I'm playing** (off by default). When on, Discord shows
"Listening to Apollo" with the title, artist, album art, a progress bar and one button,
"Play on YouTube Music". Code: `crates/app/src/discord/`.

## What goes over the wire

- Only the Discord client's local IPC pipe: `\\?\pipe\discord-ipc-0` to `-9` on Windows (a Unix
  socket of the same name on dev machines). Apollo opens no network connection for this.
- Discord itself downloads the cover from the https thumbnail URL in the activity (the same
  Google/YouTube image URL the player already uses, at 720 px). No assets are uploaded to the
  Discord application.
- Frame format: little-endian opcode and length, then JSON. Handshake `{"v":1,"client_id":...}`,
  then `SET_ACTIVITY` with `type: 2` (Listening). `details` is the title, `state` the artist,
  `assets.large_text` the album, `timestamps.start/end` (Unix seconds) make the progress bar.

## Behaviour

| Event | Presence |
|---|---|
| Track starts, resumes, or the position jumps by more than 1.5 s | new activity |
| Pause, stop, queue ends | cleared |
| Setting turned off | cleared, pipe closed, no more connection attempts |
| Quit | cleared (waits up to 0.5 s) |
| Discord not running | retried every 30 s while something is playing; silent |
| Discord closes mid-song | reconnect on the same 30 s rhythm |

Updates are spaced at least 4 s apart (Discord allows 5 per 20 s); only the latest wanted state
is sent when several pile up.

## The Discord application ID

`APPLICATION_ID` in `discord/mod.rs` is empty until the owner creates the app, which keeps the
feature inert (the switch still exists). Create it at <https://discord.com/developers/applications>:
New Application, name it **Apollo** (the name is what shows after "Listening to"), and copy the
Application ID from General Information. No bot, OAuth or secrets are involved; the ID is public.
`APOLLO_DISCORD_APP_ID` in the environment overrides the constant for a try-out without a rebuild.

Discord only shows the button to *other* people, not in your own profile.
