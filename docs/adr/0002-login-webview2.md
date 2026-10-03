# ADR 0002 — In-app sign-in via WebView2 cookie capture

Status: **accepted by the owner on 2026-10-03** as the primary login, with Firefox cookie import and manual import kept as fallbacks. Owner-requested deviation from AUTH-03; evidence positive on one run (below). Departs from BUILD_SPEC AUTH-03,
which defers an embedded login helper to its own ADR; this is that ADR.

## Context
The owner wants login that is much easier than manual cookie import and accepts entering
credentials in-app. A native password form cannot work: Google blocks it, and blocks
embedded-browser OAuth flows. The spec forbids the app handling Google passwords, codes or 2FA.

## Proposal
Open a WebView2 window on Google's own sign-in page for YouTube Music. The owner signs in
normally (passkeys and 2FA work as in Edge). The app then reads cookies from the WebView2 cookie
manager (includes HttpOnly) and validates them with an authenticated InnerTube call. The app
never sees the password.

## Test plan (`spikes/m0-login`)
1. Does Google let sign-in complete in WebView2, or show a "browser may not be secure" page?
2. Do captured cookies pass an authenticated `account/account_menu` call (logged-in flag)?
3. Can the same cookies feed yt-dlp (`--cookies`) for authenticated resolution (M0 step 6)?
4. Fallback: `yt-dlp --cookies-from-browser firefox`, then manual cookie-jar import.

## Constraints regardless of outcome
- Cookie values never printed, logged, put in argv, committed, or shown to the model.
- Persist only through the OS vault / authenticated encryption (SEC-01), not this spike.
- WebView2 profile lives in `%LOCALAPPDATA%\Apollo`, outside the repo.

## Outcome (2026-10-02, owner-run)
- Google let sign-in complete inside WebView2 (default WebView2 user agent, no automation flags,
  no remote debug port). A passkey-enrollment prompt appeared and the owner got past it.
- The WebView2 cookie manager returned 23 cookies, including the HttpOnly account cookies; no manual import.
- Authenticated `account/account_menu` returned HTTP 200 with the logged-in flag and an account name.
- The same session works with yt-dlp (`--cookies` from an exported file): 10/10 tracks (see baseline).
- Fallback also works: `yt-dlp --cookies-from-browser firefox` (3/3 tracks).

## Caveats
- One successful run on one account/day. Google can change its embedded-browser policy; keep manual
  cookie import (AUTH-03) and Firefox import as fallbacks.
- This spike uses a separate tao/wry process. Integrating into GPUI is not proven: do not add a second
  event loop to the GPUI app (OS-01). Options: separate helper executable that hands cookies back over
  a user-scoped pipe, or a WebView2 child window on GPUI's HWND. Decide in M2.
- yt-dlp needs cookies as a Netscape file: SEC-02 private temporary file, deleted after each resolve.
- Cookie lifetime/rotation not measured. Brand-account identity selection not tested.
- InnerTube client version in the validation call was a guess that worked; do not hardcode it.
