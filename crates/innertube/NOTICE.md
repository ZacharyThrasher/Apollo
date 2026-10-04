# Provenance

Vendored from **Limusic** (`SimoHypers/limusic`, `crates/innertube`), revision
`77508d6bc9c323c2749aa6add51045b6e98a506e` (2026-10-03), licensed **GPL-3.0-or-later**
(`LICENSE` in this directory is Limusic's). Apollo is GPL-3.0-or-later and not distributed.
Upstream's `rustfmt.toml` is kept so `cargo fmt` does not rewrite vendored code.

Re-sync by diffing against upstream at a newer revision; keep the list below current.

## Kept (trimmed)

- `transport.rs`: HTTP client, header building, SAPISIDHASH, `Set-Cookie` rotation merge
  (Limusic issue #165: Google rotates `__Secure-*PSIDTS`; dropping the new value kills the login
  hours in), visitorData bootstrap.
- `clients.rs` / `clients.json`: only the `WEB_REMIX` identity.
- `models/{context,metadata,browse}.rs` and `endpoints.rs`: search, suggestions, `next`, account
  menu and identities, home, history, library grids, playlist, album, artist, paging, `rate`.
- Fixtures: account menu/identities, search suggestions.

## Dropped (and why)

| Dropped | Reason |
| --- | --- |
| The `player` endpoint as a stream source, stream clients (`VISIONOS`, `ANDROID_VR`, `IOS`, `TVHTML5*`, `WEB_CREATOR`), upstream `models/player.rs` | Audio is resolved by yt-dlp; no cipher/PO-token code in the app. Only the loudness read is back, see Local changes |
| `rustypipe_fallback.rs` and the `rustypipe` dependency | Anonymous stream fallback; the spec forbids guest fallback |
| `blocklist.rs`, hide-videos filter | Product features, not part of the adapter; can come back as domain policy |
| Session "healer" (`wait_for_session_heal`, `begin_heal`, `without_healing`, 90 s wait) | App-specific webview healing. A 401/403 now returns `Error::SessionExpired` and raises `session_rejected`; the account model decides what it means |
| `post_upload`, playlist cover and the other `edit_playlist` actions (description, privacy, playlist-wide moves), `feedback`, `subscribe` | Write actions, not needed. Re-vendored in M3: `like/like`, `like/removelike` (`rate`), `edit_playlist` add-video, `playlist/get_add_to_playlist`. Added by Apollo for playlist editing: see Local changes |
| `register_playback`, CPN generation | History write; M4 |
| Lyrics (`models/lyrics.rs`, `IOS_MUSIC` client) | M4 |
| `tests/live_smoke.rs`, `examples/`, `integration-tests` feature | Network tests; ours must run offline |

## Local changes

- `Session` has a hand-written `Debug` that redacts cookie, visitorData and dataSyncId.
- Transport errors drop the request URL (`reqwest::Error::without_url`) before logging or returning.
- `SongItem::occurrence_id`: `playlistSetVideoId` read from every row, editable or not, so paged
  listings can deduplicate on the occurrence instead of the video.
- `PlaylistPage` / `PlaylistContinuation` / `AlbumPage` `skipped`: rows the response carried that
  did not become a track (unknown renderer, or a known one with no usable video). Unknown
  renderers in the track shelf are counted instead of vanishing.
- `Error::SessionExpired` message is neutral; user-facing copy belongs to the UI layer.
- `Error::Refused`: the service answered 200 but its edit status was not `STATUS_SUCCEEDED`.
  Writes are idempotent (add uses `DEDUPE_OPTION_SKIP`) because the transport retries POSTs.
- `add_to_playlist_options` / `parse_add_to_playlist`: the sheet of editable playlists with
  `containsSelectedVideos`. The fixture `add_to_playlist.json` is hand-written from memory of the
  response shape, not a recording; verify against the live account in the Windows check.

- Playlist editing writes (Apollo additions, not from Limusic): `create_playlist` (`playlist/create`,
  private, empty description), `delete_playlist` (`playlist/delete`), `rename_playlist`,
  `remove_from_playlist` and `move_in_playlist` (`browse/edit_playlist` with
  `ACTION_SET_PLAYLIST_NAME`, `ACTION_REMOVE_VIDEO` and `ACTION_MOVE_VIDEO_BEFORE`), and
  `save_playlist` (`like/like` and `like/removelike` with `target.playlistId`, the way a playlist
  or an album's audio playlist `OLAK5uy_…` is saved to the library). Every one goes through
  `post_once` (a single attempt): the transport's three retries for timeouts are right for reads
  and for idempotent writes, but a retried create would make a second playlist, so a timeout
  here is an unknown outcome that `apollo-app`'s `library::edits` settles by reading the library
  or the playlist back. The request bodies are unit-tested offline; the response shapes
  (`playlistId` in the create answer, `STATUS_SUCCEEDED` on the edits) are from memory of the
  service, not recordings, and are verified only in the signed-in Windows check (against a
  scratch playlist named "Apollo test").

- `InnerTube::player_loudness` and `models/player.rs` (Zach approved one metadata-only `player`
  call per track for volume normalization, 2026-10-03): the plain WEB_REMIX `player` request
  (`videoId`, `contentCheckOk`/`racyCheckOk`, no signature timestamp, no PO token) answered with
  `playerConfig.audioConfig.loudnessDb` (and `perceptualLoudnessDb`, diagnostics only), falling back to the first audio adaptive format's
  `loudnessDb`. The
  response's `streamingData` and playback-tracking URLs are dropped unread; Apollo does not report
  plays. This is a new, minimal parser, not upstream's `models/player.rs`. Its fixtures are
  hand-written from memory of the response shape, not recordings: an anonymous `player` request
  answers `UNPLAYABLE` with no `playerConfig`, so the real shape and the session-bound answer are
  verified only in the signed-in Windows check.

## Audit notes

- No credentials are persisted by this crate. Cookie rotation is surfaced through
  `cookie_changed`; the caller writes it to the OS vault.
- Search carries the account only when `record_history` is true (Limusic #203: typeahead used to
  write every keystroke into the account's search history).
- `clients.json` carries a pinned client version and a Firefox user agent; both rot. Treat a wave
  of parse or 4xx failures as "bump the client version" first.
- The `accounts_list` and `account_menu` parsers are fixture-tested only; the real response shape
  for brand/channel accounts is still unverified (M0 gap, TEST-03).
- `HomePage::skipped_shelves` / `Section::skipped`: shelves and items on the home feed that did not
  parse (an unknown `*ShelfRenderer`, an unreadable item, a shelf with no readable items) are
  counted instead of vanishing. The recorded fixture `home_anonymous_recorded.json` is an
  anonymous response; signed-in shelf shapes in tests are hand-built, so verify them on the live account.
