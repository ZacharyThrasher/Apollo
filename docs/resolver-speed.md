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
