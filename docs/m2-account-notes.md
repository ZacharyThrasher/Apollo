# M2 account service — behaviour and limits

Crates: `crates/account` (`apollo-account`), `crates/login-helper` (WebView2 sign-in process),
shared primitives in `crates/services-common`.

## The three concepts (AUTH-01) are kept apart

| Concept | States | Notes |
|---|---|---|
| Session | Absent, Validating, Valid, Expired, Unreachable | `Unreachable` also covers "answered in a shape we do not understand"; `Problem::ServiceChanged` says which. A network failure is **not** a logout. |
| Identity | None, Selected, Ambiguous, Changed | More than one channel = the owner must choose. A previously chosen identity that vanishes becomes `Changed` and blocks until re-confirmed. |
| Premium evidence | OwnerDeclared, Undeclared | Nothing else exists: no documented entitlement endpoint. Label is "Premium assumed for this account", never "verified". |

`AccountView::can_use_content()` needs all three. The resolver gets a snapshot only for a
**confirmed session with a selected identity** (`SessionProvider::snapshot`): confirmed means the
service has said yes at least once for the current jar. This is the answer to the M0 finding that
yt-dlp resolves with no or fake cookies: **the gate is the account's own validation, never the
resolver's success.**

A confirmed session keeps working while it is being re-checked (`Validating`) and while the service
is unreachable; only the service saying "logged out" (or logout) ends it (AUTH-04). A jar that was
never confirmed (pending import, restored from disk but not yet validated) is not usable, and the
resolver reports that as `Failure::Network` ("unconfirmed", try again) rather than `SessionExpired`
("sign in again"), via `SessionProvider::unavailable_reason`. After an offline start the app must
call `revalidate()` again when the network returns.

Two counters: `epoch` changes whenever what a snapshot may contain changes (import, restore,
logout, identity choice, **cookie rotation**) and guards snapshots and rotation merges; `generation`
changes only when the jar is replaced or destroyed (import, restore, logout) and guards in-flight
validations. Reason: real yt-dlp rewrites the cookie file on essentially every run (seen in the M2
real-tool test), so rotations are frequent and must not discard a re-check that is in flight.

## Validation (AUTH-04)

`InnerTubeValidator`: `account/account_menu` with the SAPISIDHASH-family `Authorization` header and
the cookie header, then `account/accounts_list` for the channel list.

- 200 and `logged_in=1` -> valid. 200 and `logged_in=0`, or 401/403 -> Expired.
- 5xx/429/timeouts -> Unreachable (says nothing about the session).
- 200 in an unknown shape, or logged in but no identity to bind -> Unrecognized (never "valid").
- A failing `accounts_list` does not invalidate the session; only the active identity is then known.
- **Verified against the real service in M0 only for `account_menu`** (200, logged-in flag, header).
  The multi-channel parser (`accountItem`, `pageId`) is checked against *synthetic* fixtures only:
  owner-run test with a brand-account login is still needed (TEST-03).

## Storage (SEC-01)

Cookie jar -> JSON -> **XChaCha20-Poly1305** (vetted AEAD, 24-byte random nonce per save, AAD
`apollo/cookie-jar/v1`) -> `session.v1` in a user-only directory (protected DACL). The 32-byte key
lives in **Windows Credential Manager** (generic credential, per machine, readable by this user).
Credential Manager blobs are limited to 2.5 KB, which is why the vault holds the key and not the jar.
Atomic replace on save. Selected identity is stored inside the encrypted payload.

- Vault unavailable on save: nothing durable is written (`StoreError::VaultUnavailable`); the view
  says `NotPersisted(reason)`; the owner may `accept_memory_only()`. **No plaintext fallback exists**
  (test: directory stays empty).
- Vault unavailable on restart with a ciphertext present: reported, file **kept** (a locked vault is
  temporary); restores once the vault is back.
- Tampered/truncated file or missing key: removed and reported (`DiscardedUnreadable`).
- Known limit: AEAD detects modification but not **rollback** to an older valid ciphertext; an old
  jar would simply fail validation if it expired.

## Rotation (SEC-02, TEST-15)

The resolver offers rewritten cookies back as `offer_rotation(snapshot_epoch, text)`. The service
merges only if that epoch is still current and the session is Valid; a merge bumps the epoch, so a
second run that started from the same snapshot is refused. Merge rules: update known cookies by
(name, domain, path), add in-scope ones, **never remove**, never import out-of-scope domains.

## Logout (SEC-03, TEST-04)

Order: registered hooks (stop playback, cancel resolver jobs, clear leases) -> epoch bump (in-flight
validations and resolver snapshots become stale) -> drop and zero the jar -> delete vault key and
ciphertext -> notify. Tested: hooks see the snapshot still present (they run first); a validation
that finishes after logout is discarded (`Superseded`) and cannot log the user back in; a late
rotation after logout is refused. Writes already accepted by YouTube cannot be undone locally.

## Sign-in (ADR 0002 / 0003)

`apollo-login-helper` (wry/tao, separate process): the owner signs in on Google's own page inside
WebView2; the helper waits for `SAPISID`/`__Secure-3PAPISID` on music.youtube.com and prints the
cookies as one JSON line on **stdout, the anonymous pipe only the parent holds**. No file, no named
pipe, no argv, no env. The parent runs it in a Job Object with a timeout and cancel, caps its output
at 1 MiB, ignores its stderr, uses a fresh private WebView2 profile per attempt and deletes it
afterwards. The jar is untrusted until `AccountService::import` validates it.

Fallbacks: Netscape cookie file, bare `Cookie:` header (flagged `ephemeral_seed`, session-only, no
invented lifetime), and **Firefox for a profile the owner picks** (`profiles.ini` lists them; the
`[Install...]` default beats a stale legacy `Default=1`, which was a real bug found on this machine).
Firefox import copies `cookies.sqlite` and its WAL into a private temp dir, reads there (including
rows that exist only in the WAL), and wipes the copy; the browser's own files are never modified.
Only YouTube/Google-scope cookies are kept.

## Tests

57 tests: jar (redaction, scoping, Netscape round trip, header seed, rotation merge), AEAD store
(round trip, no plaintext on disk, tamper, wrong key, missing key, unavailable vault, delete),
identity validation (single/multiple channels, 401/403/429/5xx/timeouts, unrecognized shapes,
redaction), service (gating, ambiguous identity + persistence, logout order and races, concurrent
refresh dedupe, restart/offline/expired, locked vault, rotation epochs, identity change, canary scan),
login handoff with a fake helper (pipe-only transport, crash, user close, timeout, bad/oversized
output, cancel kills the tree, profile removal, canary scan), Firefox import on synthetic and WAL
profiles, and the **real** Credential Manager and ACL checks.

## Real-account evidence (2026-10-03, `crates/m2-probe`, run on the owner's PC)

| Run | Result |
|---|---|
| `--source firefox --silent` | 22/22 PASS |
| `--source firefox` (audible, WASAPI, ~30% volume) | 22/22 PASS; the output-device guard did not misfire |
| `--source login` (real WebView2 helper, owner signed in) | 22/22 PASS (25 cookies captured through the anonymous pipe) |

Each run: session validated by the service, stored encrypted, Liked Music listing (131 entries),
4 varied tracks resolve and play with no PO-token service (first audio about 5 s), ~4 Hz position,
exact seek, EOF auto-advance, explicit Next, pause/resume, 20 rapid Next (one owner, no leaked
yt-dlp processes), then logout while playing (audio stops, vault key and file deleted) and a play
attempt afterwards fails as `SessionExpired` without starting a process.

Bugs the real runs found (all fixed, with regression tests):
1. **Cookie header**: a Firefox jar holds SAPISID/SID/... under both `.google.com` and `.youtube.com`;
   sending all of them to music.youtube.com made the service answer "logged out". Only
   host-matching, unexpired cookies are sent now.
2. **WebView2 profile left behind**: after a real sign-in the throwaway profile directory survived
   because WebView2 child processes still held files when it was deleted. Removal now retries for up
   to 15 s, stale `login-*` profiles are swept (recursively) at the next start, and a test cancels
   the real helper and checks nothing is left.
3. **Firefox default profile**: `[Install]` section beats a stale legacy `Default=1`.

Weak spot in the probe: "pause holds position" paused a track right after it started (0 ms to
0 ms); the following "resume continues" check does show real progress.

## not_run / limits

- Brand/channel selection against a real multi-channel account (TEST-03). The single-account
  `accounts_list` response did parse (1 identity) in the real runs.
- Cookie lifetime/rotation observed over days; periodic revalidation uses a 30-minute policy value
  that is not a claimed cookie lifetime.
- Premium status is owner-declared only.
- Linux/macOS: only jar/store/identity/service build there; vault, login and Firefox import are
  Windows-only. Not compiled on Linux in this session (no Linux target installed): **not_run**.
