# ADR 0004 — Warm resolver workers, in-memory URL cache and speculative resolve

Status: **accepted (owner request 2026-10-03: "as instant/responsive as possible")**. Supersedes
nothing; the per-track process path stays as the automatic fallback and as `APOLLO_RESOLVER=legacy`.

## Context
Starting a playlist, queue or album waited on one `yt-dlp` process per track. Each process pays
for a PyInstaller start, a fresh YouTube session and TLS handshakes, a webpage fetch, and a Deno
re-parse of the player JS (the biggest cost: vanilla `yt-dlp` cannot reuse the preprocessed
player across processes when `--no-cache-dir` is set). The app is personal, so we can spend
memory and a few extra requests to hide that wait.

## Decision
1. **Warm workers.** A small pool of long-lived `yt-dlp` processes (same pinned exe, no changes
   to it) loads one tiny extractor plugin (`crates/resolver/plugin/apollo_serve.py`) that serves
   JSON lines on stdin/stdout. The `YoutubeDL` object, its connections and an **in-memory**
   preprocessed-player cache live across requests. Cookies are sent over the pipe (never
   written to disk); a rotated jar comes back over the pipe and goes through the existing
   epoch-guarded `offer_rotation`.
2. **In-memory URL cache** (`LeaseCache` over the existing `LeaseStore`): reuse a lease only
   while it still has at least 15 min left (5 min cap when the expiry is unknown). Concurrent
   requests for the same video share one resolve (flight dedupe).
3. **Speculative resolve.** The app asks the resolver to `prefetch` the queue head and the next
   two tracks in play order, the first row of a freshly listed page, and the track the pointer
   rests on for 250 ms (debounced). Prefetch is background priority: at most 2 of the 3 workers,
   at most 6 pending (since ADR 0005: 2 pending and 10 a minute, none during a 429 cooldown),
   never ahead of a foreground request.
4. **Fallback.** If the plugin does not load or a worker will not start twice in a row, the pool
   marks itself unusable and the old process-per-track path runs, with the reason in
   diagnostics. `APOLLO_RESOLVER=legacy` forces it.

## Constraints kept
- No session, no process and no cached URL (the gate runs before the cache).
- Signed URLs live in memory only; never logged, never on disk. Cookies never touch disk on the
  warm path.
- `UrlPolicy::validate` still runs on every result; the stale-result rules (`LoadId`) in the
  coordinator are unchanged. Logout clears the cache (`invalidate`).
- Workers run in a Job Object (kill on close) with a cleared environment and shut down
  gracefully so PyInstaller leaves no `_MEI*` directories.

## Relaxations (owner-accepted, personal app)
- A worker keeps the account cookies in process memory between requests (before: a private file
  for the length of one request).
- Up to 3 resident workers, about 100 MB each.
- Extra background requests to YouTube for tracks the user may never play.

## Risks
- YouTube rate limiting from speculation: bounded by the caps above and the 250 ms intent delay.
- The plugin API of the pinned PyInstaller exe is only proven on the Linux build of the same
  yt-dlp version; on Windows the failure mode is the automatic legacy fallback.
- Warm resolves use the same web client, so an expired/rotated session behaves as before.

## Measurements
See `docs/resolver-speed.md` (run `apollo-resolve-bench.exe` on the PC).

## Addendum 2026-10-03: fast clicks, full-quality background work
Measured on the owner's PC (bench section [6], `docs/resolver-speed.md`): skipping yt-dlp's watch-page
and initial-data requests takes a cold click from about 3.3 s to about 1.9 s, but the embedded
client then serves the plain opus stream (itag 251, about 130 kbps) where yt-dlp's defaults get
the Premium opus stream (itag 774, about 256 kbps) from the web music client. So
`WarmConfig::fast_clicks()` (production) uses the skip arguments only when someone is waiting on
the result (a click); everything resolved ahead (queue lookahead, hover, page open) and the
fallback keep the defaults. A click that gets no usable stream with the fast arguments (for
example an embedding-restricted track) is retried once with the defaults. The arguments travel
with each resolve request, so one worker serves both kinds. Skipping client configs, or limiting
the clients, gave no playable format in the measurements and is not used.

Update the same day: the web music client call (0.9 to 1.5 s) was the largest remaining cost on
the fast path, and it only runs because the music URL adds that client. Clicks now resolve the
plain watch URL with the embedded and tv clients only: about 0.8 s cold, measured. Requests
carry `site` (`www` or `music`) and the arguments; the defaults (music URL, all clients) remain
for background work and for the retry.

Decision 2026-10-03 (owner): "always best". Production resolves every request, clicks included, with
yt-dlp's defaults and the Premium stream; `WarmConfig::fast_clicks()` stays in the code as an opt-in
profile (cold click about 0.8 s at about 130 kbps). The bench now searches for settings that keep
the Premium stream and are faster than the defaults.
