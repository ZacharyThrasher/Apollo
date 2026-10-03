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
| `player` endpoint, stream clients (`VISIONOS`, `ANDROID_VR`, `IOS`, `TVHTML5*`, `WEB_CREATOR`), `models/player.rs` | Audio is resolved by yt-dlp; no cipher/PO-token code in the app |
| `rustypipe_fallback.rs` and the `rustypipe` dependency | Anonymous stream fallback; the spec forbids guest fallback |
| `blocklist.rs`, hide-videos filter | Product features, not part of the adapter; can come back as domain policy |
| Session "healer" (`wait_for_session_heal`, `begin_heal`, `without_healing`, 90 s wait) | App-specific webview healing. A 401/403 now returns `Error::SessionExpired` and raises `session_rejected`; the account model decides what it means |
| `post_upload`, playlist cover/edit/create/delete, `feedback`, `subscribe` | Write actions; re-vendor in M3 when needed |
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

## Audit notes

- No credentials are persisted by this crate. Cookie rotation is surfaced through
  `cookie_changed`; the caller writes it to the OS vault.
- Search carries the account only when `record_history` is true (Limusic #203: typeahead used to
  write every keystroke into the account's search history).
- `clients.json` carries a pinned client version and a Firefox user agent; both rot. Treat a wave
  of parse or 4xx failures as "bump the client version" first.
- The `accounts_list` and `account_menu` parsers are fixture-tested only; the real response shape
  for brand/channel accounts is still unverified (M0 gap, TEST-03).
