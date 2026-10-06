# ADR 0005 — One shared throttle: back off on 429, ration speculation

Status: **accepted (2026-10-05, after a real 429 lockout)**. Amends ADR 0004 (prefetch limits).

## What happened
On 2026-10-05 a long Crate-building session kept the speculative resolver busy for 14 minutes at
about 40 prefetches a minute (the warm pool's ceiling; each resolve is about six YouTube
requests, plus one loudness `player` call). At 16:01 YouTube started answering HTTP 429. Apollo
had no backoff, so it went on: 61 more resolves came back throttled in the next six minutes, the
player bar said "Too many requests", Retry fired straight away, and the account's browser
playback was refused too. yt-dlp hid part of it: when WEB_REMIX is 429'd it quietly falls back to
WEB_CREATOR, so some "successful" resolves in the log were already throttled.

Nothing capped the speculative work: hover and keyboard-selection hints, the two-track lookahead
and Skim all call `prefetch`, and each call was only deduplicated against the cache, never
budgeted.

## Decision
1. **One `Throttle` per process** (`apollo_services_common::throttle`). Any 429 (the resolver,
   an InnerTube request, a loudness lookup) starts a cooldown: 30 s, doubling with each
   consecutive failure, capped at 10 min, or the server's `Retry-After` when that is longer
   (capped at 1 h). Requests already in flight when the first 429 lands do not count as new
   strikes. A success after the cooldown, or 20 quiet minutes, starts over.
2. **Two lanes.** Foreground (a click, Retry) is blocked for the cooldown; background (prefetch,
   preload, loudness, autoplay) for twice that, so the first request after a ban is the user's.
3. **Resolver.** Prefetch is limited to 2 in flight and a token bucket of 3 then one per 6 s
   (10 a minute); playback itself is never rationed. During a cooldown a resolve that needs the
   network fails at once with `Failure::Throttled` (the player-bar text is unchanged); cached
   URLs still play. The warm worker reports a 429 that yt-dlp survived through a fallback client.
4. **Loudness** waits for the background lane and keeps requests at least 1.5 s apart.
5. **InnerTube** reports every 429 and its `Retry-After` through `innertube::set_throttle_hook`
   (the app points it at the shared throttle). It never retried a 429; that is unchanged.
6. **Retry** after "Too many requests" is held until the cooldown ends, then goes ahead once.
   Autoplay's radio refill waits for the background lane too.
7. **Diagnostics** in `latency.log`: `throttled strikes=N wait_s=… background_wait_s=…`,
   `prefetch skipped reason=busy|cooldown|budget n=…` (at most every 30 s), `load held back:
   cooldown`, and `repeat=true` on a prefetch of a track already prefetched within the hour (a
   cache that does not hold, or a trigger that does not stop).

## Consequences
Hover and lookahead hints are dropped, not queued, once the budget is spent, so a click on a
track that was not prefetched can cost the full resolve time again. yt-dlp does not expose
`Retry-After`, so the resolver path backs off by the exponential schedule only.

## Update (2026-10-05, second 429): the trigger was the owner's own cursor
The second lockout happened on a build without the above. The log showed about 40 prefetches a
minute for 20 minutes with the queue full, and minutes with nothing playing. The loop: **C with
"Move to the next song after C, A or K" on** (the default) steps the keyboard cursor to the next
row, and a cursor resting on a row for 250 ms is a preload hint. Collecting songs into the Crate at
one song a second therefore resolved every song the cursor passed (about six requests each, plus a
loudness call), for songs nobody meant to play. Hover is the same trigger by pointer.

What changed:
1. **Preload songs on hover** (Settings > Playback, off by default). Off, hover, a resting cursor
   and page opens start no resolve; the queue's own lookahead (head plus next) and Skim's one-ahead
   stay. A cursor stepped by C, A or K on the Search page no longer counts as resting at all.
2. **Related** waits 600 ms after the track changes (and for the cooldown) before asking, so
   skipping through songs asks once for the song landed on.
3. **Loudness** keeps at most six lookahead lookups waiting; a track about to play is never refused.
4. **Play reporting** sends no tracking lookup while the background lane is blocked.
5. **Pages** (playlist, album, artist) opened again within 30 s of their last live fetch are drawn
   from the cache without a new request; Retry and the Reload verbs always ask.
6. **`yt_request kind=.. video_id=..`** in `latency.log` for every request (InnerTube and resolver),
   so a repeat shows up as the same id again and again.
