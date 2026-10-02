# ADR 0001 — Stack confirmed for Milestone 0 (Windows 11)

Status: accepted for M0 on this host. Not a claim about other OSes.

## Decision
Keep the spec's default stack, with these concrete choices:

- **UI:** `gpui` 0.2.2 + `gpui_platform` 0.1.0 from Zed git rev `f37989fbdf84…` (compiled unchanged).
  Mirror Zed's `async-task` patch in our workspace manifest. No GPUI Kit, no crates.io `gpui` snapshot.
- **Audio:** libmpv via `libmpv2` 6.0.0 + a prebuilt shinchiro mpv-dev DLL; `mpv.lib` generated locally.
- **Runtime:** app-owned Tokio runtime on its own thread; GPUI side awaits an mpsc `recv()`
  (runtime-agnostic), so no executor mixing.
- **OS media:** Souvlaki 0.8.3 using the HWND GPUI exposes through `raw-window-handle`; no second event loop.

## Evidence
See `docs/build-baseline.md` (results table).

## Consequences / open items
- GPUI is pre-1.0; any rev bump must repeat the M0 checks.
- IME untested (no IME installed).
- The `windows` crate appears in four versions; watch binary size and link time.
- yt-dlp/Deno/resolution path: **not yet evidenced** (step 6).
