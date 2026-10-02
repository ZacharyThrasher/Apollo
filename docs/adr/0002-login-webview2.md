# ADR 0002 — In-app sign-in via WebView2 cookie capture

Status: **proposed — pending the owner-run test** (M0 step 5). Departs from BUILD_SPEC AUTH-03,
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

## Outcome
_Pending._
