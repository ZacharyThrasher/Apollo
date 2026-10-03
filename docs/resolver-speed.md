# Resolver speed: how a song starts quickly

Design record: `docs/adr/0004-warm-resolver-workers.md`.

## Where the time went (per track, cold `yt-dlp`)
1. Start the PyInstaller exe (bootloader + Python child).
2. New YouTube session, TLS handshakes, watch page fetch.
3. Download and parse the player JS, solve challenges in Deno (largest part).
4. Pick the format, print JSON.
Warm workers remove 1-3 for every track after the first and let the preprocessed player be reused;
the URL cache and prefetch remove the wait entirely for tracks resolved ahead of time.

## What the app does now
| Situation | What happens |
|---|---|
| Queue/playlist/album loaded | head + next 2 (play order) resolved in the background |
| Pointer rests on a track row for 250 ms | that track is resolved speculatively |
| Click play | cached URL is used (no resolve), else a warm worker resolves it |
| Next track | the engine's preload makes it near-instant |
| Warm workers unavailable | old process-per-track path, reason shown in diagnostics |

## Measuring on the PC
1. Close Apollo. Rebuild with `scripts\package.ps1` (it now also builds `apollo-resolve-bench.exe`).
2. Run `C:\Apollo\apollo-resolve-bench.exe` (uses Apollo's stored session; Liked Music tracks).
3. It prints a SUMMARY and saves `%LOCALAPPDATA%\Apollo\app\bench-resolve.txt`:
   - [1] legacy per-process (the BEFORE number)
   - [2] warm: first click, then steady state
   - [3] cached URL
   - [4] prefetch then click
   - [5] cached URL to libmpv open and first audio (null output)
4. In the app, every start appends a line to `%LOCALAPPDATA%\Apollo\app\latency.log`:
   `t=<ms> kind=.. resolve_ms=.. open_ms=.. audio_ms=.. total_ms=..` (`resolve_ms=-` means a
   preloaded track was adopted; durations only, no ids or URLs). The resolver adds its own lines to the
   same file (source: warm/cache/cold, worker time, per-phase milliseconds).

## Switches
- `APOLLO_RESOLVER=legacy`: always use a process per track (A/B comparison, or if workers misbehave).
- Tunables live in `ResolverConfig::production()` (`WarmConfig`, `CacheConfig`).

## Measured on the owner's PC (main with the warm workers, real account, 2026-10-03)
| Click to playable stream URL | Median |
|---|---|
| Before: one yt-dlp per track | 5.1 s |
| First click after launch (starts a worker) | 4.6 s |
| Cold click, warm worker | 2.5 to 3.7 s |
| URL already held, or prefetched then clicked | 0.00 s |
| Then libmpv: file open / audio moving | 24 ms / about 0.3 s |

Where a cold 3 s goes (worker phases): watch-page download about 1.0 s (this is also where yt-dlp
learns the account is Premium, which makes it add the `web_creator` client), the extra client
lookups about 0.4 s, the music player API 0.7 to 1.1 s, JS solve about 0.15 s.

## Round 2: trimming a cold click
`WarmConfig::youtube_args` hands `--extractor-args youtube:...` to the workers. Skipping the page
and the initial-data request (`player_skip=webpage,initial_data`) removes the ~1 s page step;
the cost is that yt-dlp no longer sees the Premium marker, so it asks `web_embedded`,
`tv_downgraded` and `web_music` instead of `web_creator`, `tv_downgraded`, `web_music`. The bench
section [6] runs each setting on a fresh worker and prints which client served the stream
(`client=` in the worker lines: `c=` of the URL) so the choice is made on data, not on guesses.
Home songs now also resolve ahead when the pointer rests on them.

## Why not call the player API directly from Rust ("what the browser does")?
It would still pay the one player-API round trip (0.7 to 1.1 s here) and save the JS solve
process (about 0.15 s) and any yt-dlp bookkeeping: an estimated 0.3 to 0.4 s over a trimmed
worker. It would also make us own the client quirks yt-dlp tracks (visitor data, signature
timestamp, PO-token rules) and drop its multi-client fallback. A web client turning SABR-only
would break both paths and is not bypassed in either (spec). Revisit only if cold clicks stay
above roughly 1.5 s after the trim; if built, keep the workers as the fallback.
