# Local stats

What Apollo itself played, kept on this machine and never sent anywhere. It feeds the Library's
**Stats** tab and Home's **On repeat** and **Forgotten favourites** shelves. Visual decisions:
`docs/design/DIRECTION.md` "Stats and Home personalization decisions".

## The play log

* Table `plays` in `apollo.db` (schema migration 5): one row per play that counted, with the
  account key, when it counted (unix seconds), the track (video id, title, artists as JSON,
  album title and id, duration), the cover address the app showed for it, and how long it was
  listened to. No URLs of streams, no cookies, nothing about the session.
* Plays are filed under the account the page cache serves (the chosen channel). Another account
  never sees them; signing out keeps them on disk (they are not a cache). While nobody is chosen
  nothing is recorded.
* At most 200 000 rows per account; the oldest go first.
* Writes happen on one worker thread (`stats::PlayLog`), in order, off the UI thread. Reads open
  the file on a background thread.

## What counts as a play (`stats/tally.rs`, pure)

* Audio must be flowing. Listening time is the time the player spends in the playing state,
  not the position: seeking does not make a play, pausing stops the clock, a gap of more than
  5 s between two looks (the PC slept) counts for 5 s.
* The play counts at **30 s** of listening, or **half the track** for a track shorter than a
  minute. Skipping through songs counts nothing.
* A pause or leaving the track settles the listening time of a play that counted.
  Repeat-one and replaying a track count each pass.
* It is independent of Settings > Playback > Add plays to history (that switch is about telling
  YouTube Music; this log is local) and of Home > Personal shelves (that only shows or hides).

## What the screens do with it (`stats/report.rs`, pure)

* Stats: per track plays in the period (Last 4 weeks, Last 6 months, All time), summed for
  Tracks played and Listening time; artists and albums are credited with the plays of their
  tracks (every credited artist gets the full count; an artist or album without an id is matched
  by name); ten rows each.
* On repeat: tracks with at least 2 plays in the last 30 days, most played first, 20 at most.
  Forgotten favourites: tracks with at least 5 plays whose latest play is 60 or more days ago,
  most played first, 20 at most. A shelf with fewer than 6 tracks is not shown.
* Home works the shelves out (in the background) at launch, whenever the account is chosen and
  every time Home is opened; they are not part of the page cache.
