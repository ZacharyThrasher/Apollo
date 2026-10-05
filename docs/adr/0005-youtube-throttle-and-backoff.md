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
