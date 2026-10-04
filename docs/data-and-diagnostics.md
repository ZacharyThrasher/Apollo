# Data: library export and diagnostics report

Settings > **Data**, the last section (after Keyboard). Two rows.

## Export library

A select row (**JSON** or **CSV**, JSON by default, remembered in `export_format`) with an
**Export…** button. The button opens the Windows save dialog (opens in Documents, proposes
`apollo-library-<date>.<ext>`; a name typed without an extension gets the format's). Nothing
shows on success. While it runs the button reads **Exporting…** and is disabled; a failure shows
beside the button in text.secondary until the next try.

**Read-only.** `export::collect` calls only `LibrarySource` reads (`collections`, `tracks`,
`more_tracks`, `subscribed_artists`); the unit tests use a source that records every write the
trait has and assert none was called. Requests go one at a time with a 150 ms pause; a network or
rate-limit failure is retried once, a dead session stops the export, and a switch of account
(`account_token`) midway writes nothing.

What is exported:

* **Liked songs** (Liked Music), every page followed.
* **Playlists** with their tracks (a song twice in a playlist stays twice; only a row that two
  overlapping pages both carried is dropped). Liked Music is not repeated here.
* **Saved albums**: the entries (title, artist and year line, id, link), not their songs.
* **Subscribed artists** (`FEmusic_library_corpus_artists`, new read in `innertube` and the
  provider): the artists the account follows. Not the Artists tab's list, which is the artists
  behind saved songs.

A list that cannot be read is named in a `problems` array (JSON) or a `problem` row (CSV), the
rest is still written, and the row says **Saved without N items**.

Files are written through `<name>.part` and renamed, so a failed export never replaces a good
file with half of one.

**JSON** (`apollo-library-export`, version 1): `liked_songs`, `playlists[]{id,title,detail,url,
tracks[]}`, `saved_albums[]`, `subscribed_artists[]`, `problems[]`. Tracks carry `video_id`,
`title`, `artists[]`, `album`, `duration_seconds`, `explicit`, `url`.

**CSV**: one table, UTF-8 with a byte-order mark (Excel), CRLF, columns
`section,list,list_id,position,video_id,title,artists,album,duration_seconds,explicit,detail,url`
where `section` is `liked_song`, `playlist_song`, `saved_album`, `subscribed_artist` or
`problem`. A text cell starting with `=`, `+`, `-`, `@`, a tab or a carriage return gets a
leading apostrophe so a spreadsheet cannot run it as a formula; JSON is the exact copy.

## Diagnostics report

A **Create…** button that saves a `.txt`. Sections: App (time, version, backend, OS, architecture,
account state, database schema and size), Tools (yt-dlp, Deno, libmpv client API), Settings
(only the keys in `settings::REPORTED_KEYS`; recent searches, the saved queue and window
positions are never listed), Recent errors (up to 20) and Recent log (up to 200 lines).

The log is a 400-line memory of what the app printed: the crate shadows `eprintln!` (see
`lib.rs`) so every existing call keeps printing and is also kept in `diagnostics::log`. Nothing
is written to disk except the report the owner saves.

**Redaction** (`diagnostics::redact`) runs over the finished text, so a secret that arrives by
any route is still caught: cookie and auth headers (`Cookie`, `Set-Cookie`, `Authorization`,
`X-Goog-*`), `SAPISIDHASH`/`Bearer` tokens, named session cookies (`SAPISID`, `__Secure-*`, …),
Netscape cookie-file lines, JSON secret fields, stream and Google-hosted URLs (whole URL), every
other URL's query and fragment (and the path, except on a short list of hosts), IPv4 and IPv6
addresses, e-mail addresses, the Windows user directory, 32+ character opaque tokens, and by
exact match the account's name, handle and channel id, and the Windows user and machine names.
Markers contain nothing secret-shaped, so redacting twice changes nothing. The tests plant
secrets in every free-text field and assert none survives.
