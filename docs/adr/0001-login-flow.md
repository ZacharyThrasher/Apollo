# ADR 0001 — Login flow (draft, pending M0 spike)

**Status:** proposed. Supersedes the "manual cookie import only" default in BUILD_SPEC AUTH-03
for the owner's own machine, if the M0 spike confirms it works.

## Context

The spec's default sign-in is a manual, scoped cookie import. The owner wants login to be much
easier and is fine entering credentials in-app (personal use, own account).

A native GPUI password form that posts to Google is not viable: Google's sign-in uses bot detection,
2FA/passkey and device challenges, and disallows embedded user agents for OAuth. A typed password
cannot satisfy those.

## Decision (to be validated)

1. **Primary:** an in-app "Sign in" action opens a WebView2 window on Google's normal sign-in for
   music.youtube.com. The owner types credentials there like in any browser (2FA/passkeys work).
   On success the app reads the YouTube cookies from the WebView2 cookie manager and stores them
   through the existing vault path (SEC-01). No password ever touches app code.
2. **Fallback:** yt-dlp-style `cookies-from-browser` for Firefox, then manual import (original AUTH-03).

Invariants unchanged: credentials only in the OS vault, never in logs/argv/DB, no anonymous fallback,
identity binding and validation after capture (AUTH-04).

## Risks

- Google may refuse sign-in inside WebView2 ("this browser may not be secure"). M0 step 5 tests this.
- Session cookies captured this way may rotate differently from a normal browser; needs the TEST-15 rotation test.
- Chrome/Edge cookie decryption (app-bound encryption) rules out `cookies-from-browser` for those browsers.
