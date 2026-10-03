# M2 resolver (yt-dlp) — behaviour and limits

Crate: `crates/resolver` (`apollo-resolver`). Implements `apollo_runtime::Resolver`; produces
validated, memory-only leases in the shared `LeaseStore` and reports `Event::Resolved`.

## Flow per request (own thread, at most 2 at a time)

1. **Gate on the account, not on yt-dlp.** `SessionProvider::snapshot()` returns `None` when there
   is no validated session: the resolver publishes `Failure::SessionExpired` and **never starts a
   process** (M0 finding: yt-dlp resolves fine with no or fake cookies, so its success proves
   nothing about the account). Test: `no_session_means_no_process_and_no_guest_fallback`.
2. The snapshot's cookie text goes into a **private directory**: created `CREATE_NEW` with a
   *protected* DACL granting only the current user (tested via `icacls`), file created
   `CREATE_NEW` (never follows a link), contents zeroed and deleted when the run ends, stale
   directories of dead processes swept at startup.
3. yt-dlp runs as an **argument array** inside a **Job Object** (started suspended, assigned, then
   resumed, so no child can escape): `--ignore-config --no-plugin-dirs --no-cache-dir
   --no-remote-components --no-update --skip-download --no-playlist -f bestaudio -J
   --js-runtimes deno:<path> --cookies <private file> <watch url>`. The video id must be exactly
   11 characters of `[A-Za-z0-9_-]`; anything else is refused before it can touch a command line.
   The environment is cleared and rebuilt (no proxy variables, no user profile, TEMP in a scratch
   dir).
4. Pipes are bounded (stdout 8 MiB by default, stderr 64 KiB kept only for classification),
   timeout 30 s, cancellation. Timeout, cancel, output flood and drop of the resolver all
   terminate the whole job; a crash of this process also kills it (KILL_ON_JOB_CLOSE).
5. JSON -> `StreamCandidate` -> `UrlPolicy::production().validate()`. Wrong id, DRM, SABR,
   manifest transports, non-googlevideo hosts, `http://`, no audio: all become
   `Failure::Unsupported` with an internal outcome code; no lease exists. Cookie/Authorization
   headers that yt-dlp reports are dropped by the policy.
6. Cookie rotation: if yt-dlp rewrote the jar, it is offered back via
   `offer_rotation(snapshot_epoch, ...)`. The account service must refuse it when a newer epoch
   exists (tested with a provider that bumps the epoch mid-run).
7. Nothing is logged except redacted outcome lines (`load=3 preload=false outcome=ok ms=4800`).

## Pinned tools (RES-01)

`YtDlpResolver::new` hashes yt-dlp and Deno and refuses to start if they differ from
`PinnedTools::m0_baseline()` (yt-dlp 2026.08.19, Deno 2.9.7; hashes in docs/build-baseline.md).
yt-dlp's bundled EJS (0.8.0) travels with the exe. The check is not atomic with execution
(a same-user attacker could swap the file in between); same-user compromise is out of scope.

## Failure mapping

| yt-dlp says | Failure |
|---|---|
| "Sign in to confirm", "cookies are no longer valid" | SessionExpired |
| HTTP 429 / too many requests | Throttled |
| members-only, Premium-only, region, private, age | Restricted |
| requested format not available, DRM, SABR, solver failures | Unsupported |
| timeouts, DNS, connection errors, HTTP 5xx, our own 30 s timeout | Network |
| anything else | Unsupported ("service compatibility") |

The patterns come from yt-dlp's messages as I know them plus the fake; real messages should be
re-checked in the owner-run account tests. stderr is inspected, never stored or shown.

## Tests (fake yt-dlp, real process machinery)

22 tests: argv safety and strict ids, JSON parsing, failure classes, validated lease, private
short-lived cookie file (exists while running, gone after, never in argv/env), no-session = no
process, policy refusals (TEST-14/19), timeout and cancel and drop each kill the **whole tree**
including a grandchild and clean up (TEST-28), epoch-guarded rotation (TEST-15), concurrency cap,
stdout flood, stale sweep, pins verified against the real M0 binaries, and a canary scan
(TEST-23) over argv, environment, diagnostics, `Debug` output and leftover files.

## Real tool check (no account needed)

`real_ytdlp_through_the_contained_runner_with_placeholder_cookies` (opt-in, `--ignored`): the real
pinned yt-dlp 2026.08.19 + Deno 2.9.7 through this runner (cleared environment, Job Object, private
cookie dir), placeholder cookie values, a public video: resolved to a validated googlevideo lease in
4.4 s; cookie dir empty afterwards. It also showed that **yt-dlp rewrites the cookie file on every
run**, so cookie rotation is the normal case, not an edge case (see account notes: epoch vs
generation). An unconfirmed session fails as `Network`/"session-unconfirmed" without a process.

## not_run / limits

- Real yt-dlp with the owner's real session through this crate: owner-run (`crates/m2-probe`).
- Linux/macOS: only argv/parse build there; the runner is Windows-only.
- The synchronous `Resolver` trait gives no way to report "resolver is shutting down"; `start`
  after drop is silently ignored.
- Per-process memory/CPU limits on the helper are not set (Job Object limits are possible later).
