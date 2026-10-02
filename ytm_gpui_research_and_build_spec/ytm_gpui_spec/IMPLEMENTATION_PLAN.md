# Implementation plan and acceptance matrix

Normative behavior lives in [BUILD_SPEC.md](BUILD_SPEC.md). Sources and limitations live in [RESEARCH.md](RESEARCH.md). No phase in this plan has been executed as part of the research.

## Build order

### Milestone 0 — Record the host and prove the risky integrations

**Deliverables:** `docs/build-baseline.md`, exact dependency/tool versions and hashes, a lockfile, redacted local probe instructions/results, and a short ADR confirming the selected stack. No designed interface required; a minimal GPUI control window and diagnostic probe are sufficient.

Establish the host OS/architecture, Rust toolchain, compatible GPUI/platform graph, system libraries, native mpv linkage and available codecs. Start with the upstream GPUI revision candidate recorded in DEPENDENCY_BASELINE.json, but change it deliberately if compilation demonstrates incompatibility. Do not copy wildcard dependency examples into the final manifest.

Validate rendering, text/IME input, a native window handle where required, accessibility basics, a background task returning to the UI, libmpv audio output using owned test media, and one actual OS media-control command.

The owner then runs a local account probe. It must import credentials without revealing them to the model, confirm the selected account/library, and test authenticated resolution with the Premium account **without a browser PO-token service first**. Use at least ten normally accessible music tracks, including album tracks, different durations and several artists. Confirm actual returned format/transport and play the full duration of representative tracks; a successful URL lookup alone does not prove sustained playback. Exercise seeking near the beginning, middle and end.

Run a negative case with absent/expired credentials and verify the product does not silently play anonymously. Inspect the permitted resolver policy rather than assuming a cookie argument proves all extraction branches are authenticated. Test the account's channel/brand identity when relevant.

**Exit gate:** a reproducible GPUI/media build and evidence of one viable authenticated account-to-audio path. If owner-run account testing is unavailable, mark it `not_run`; continue only with clearly mock-backed/offline development and do not call playback integration complete. If the stream path fails, record the exact category and compare a narrowly scoped alternative adapter—not a new UI framework or a hidden iframe workaround.

### Milestone 1 — Pure state, fixtures and contracts

**Deliverables:** domain models, coordinator reducer/state machine, mock provider/resolver/media adapters, parser fixtures, queue property tests and cancellation tests.

Implement queue occurrence IDs, ordering/shuffle/repeat, generation handling, user intent vs actual engine state, typed errors, strict external-link parsing, capability checks and retry rules. The test suite must simulate completions arriving in the wrong order before any rich interface is added.

**Exit gate:** TEST-05 through TEST-12, TEST-17 and TEST-24 pass without real network access.

### Milestone 2 — Account, resolver, media and persistence

**Deliverables:** secure importer, account/identity service, metadata adapter minimum, controlled resolver launcher, libmpv adapter, SQLite schema/migrations and paused restore.

Implement selected-identity binding, safe session rotation, temporary resolver credential handling, ephemeral leases and bounded process cleanup. Integrate sequential playback and media progress. Add local diagnostics and canary secret scans immediately, not at the end.

**Exit gate:** TEST-01 through TEST-04, TEST-13 through TEST-16, TEST-18 through TEST-23 and TEST-28 pass in their appropriate environments. No stale track can regain playback after a skip, logout or new load.

### Milestone 3 — First usable GPUI player

**Deliverables:** setup, search, library, album/artist/playlist pages, player controls, queue editor, settings, basic account mutations, keyboard/accessibility and primary-host media integration.

Use functional components and fixtures; leave aesthetics to the style model. Virtualize lists and localize progress updates. Implement honest loading, empty, partial, disconnected and failure states. Include reconciliation for likes and playlist changes and a meaningful reconnect workflow.

**Exit gate:** all mandatory first-version tests pass, plus a two-hour owner-run session. Publish a local release note with tested host, tool versions and actual limitations. Do not include placeholder buttons for unsupported operations.

### Milestone 4 — Service depth and transition quality

**Deliverables:** home shelves, radio/autoplay, playlist management, service lyrics/history where confirmed, output preferences and measured transition improvements.

Add only one feature family at a time. For radio, retain its own continuation/session context, deduplicate generated repeats sensibly while preserving user-created duplicates, and stop/refill conservatively. For history, distinguish local events from confirmed service state. For lyrics, use service-provided content first and present missing/timing-unavailable states honestly.

Implement one-track media lookahead with authoritative transition mapping. Measure local gapless behavior separately from remote buffering and format changes. Do not use crossfade as the gapless test.

**Exit gate:** TEST-31 through TEST-36 pass for each advertised capability. Disabled features remain explicitly unsupported rather than simulated successes.

### Milestone 5 — Optional ergonomics and additional platforms

Only after the core is reliable: tray/background option, mini-player, output-device selection, better startup/resource behavior and a second OS. Each additional OS repeats its full platform, account, device and shutdown tests. Packaging, sharing, offline downloads and new services remain outside this plan unless the owner changes the product scope.

## Acceptance matrix

`U` = unit/property test; `I` = controlled integration; `H` = real host/manual; `A` = owner-run authenticated test. Account tests never put credentials into model context or shared CI.

| ID | Scenario | Required result | Mode |
|---|---|---|---|
| TEST-01 | Import valid account session | Selected identity displayed and private library validates before content actions enable; Premium evidence labeled accurately. | A |
| TEST-02 | Missing/expired/negative-tier session | No silent guest playback; clear recovery; unknown Premium signal is not mislabeled negative. | U/A |
| TEST-03 | Ambiguous channel identity | Explicit selection or safe blocking; writes do not reach a different library. | A |
| TEST-04 | Logout during resolution/playback/write | Playback stops; pending loads cannot start; secrets/cache invalidated; unknown remote-write outcome reported. | I/A |
| TEST-05 | Resolve A slowly, then request B | Late A completion never changes selection or produces audio. | U/I |
| TEST-06 | Twenty rapid next/previous commands | Exactly one current owner; no stale completion, crash, deadlock or leaked child. | U/I/H |
| TEST-07 | Same video appears three times | Distinct queue occurrences, correct removal/reordering and repeat behavior. | U |
| TEST-08 | Shuffle on/off with manual additions | Current/history unchanged; manual priority preserved; canonical source order recoverable. | U |
| TEST-09 | Repeat-one and explicit Next | Automatic EOF repeats; explicit Next advances exactly once. | U/I |
| TEST-10 | Seek while paused, then pause during load | Neither operation unexpectedly resumes sound; actual position reconciles. | U/I/H |
| TEST-11 | Preload error while active track plays | Active playback continues; only stale next lease is invalidated. | U/I |
| TEST-12 | EOF and auto-transition events reorder | One logical transition, never a double skip or duplicate history event. | U/I |
| TEST-13 | Stale/expired media lease | One bounded re-resolution/reopen preserving intent/position; persistent failure is surfaced. | I/A |
| TEST-14 | Wrong ID, invalid scheme/host, unsafe redirect | Lease rejected; no credential leakage or arbitrary local media opened. | U/I |
| TEST-15 | Cookie rotation and concurrent requests | Newer canonical session not overwritten by an older snapshot; correct identity after restart. | I/A |
| TEST-16 | Vault unavailable, child canceled/crashed | Memory-only option or safe stop; private temp cleanup; no durable plaintext fallback. | I/H |
| TEST-17 | Search results complete out of order | Only current query revision updates the view; abandoned work canceled. | U/I |
| TEST-18 | Network interruption, 429, service outage | Finite retries, `Retry-After` respected, circuit breaker suppresses prefetch, UI stays usable. | I |
| TEST-19 | Unsupported/DRM-only/SABR-only format | Specific unsupported error; no fabricated direct URL or alternate-policy bypass. | U/I/A |
| TEST-20 | Output device loss and sleep/resume | No accidental loud resume, no mistaken queue skip; recovery retains actual position/intent. | H |
| TEST-21 | Codec and seeking sample | AAC/Opus samples supported by actual native build; seeking and end detection verified, not merely metadata parsed. | I/A |
| TEST-22 | Crash/restart and schema migration | Queue/order preserved, restore paused, fresh resolution; failed migration does not corrupt state. | I |
| TEST-23 | Secret canary audit | No raw/encoded cookies, authorization or signed URLs in argv, logs, DB, exports or ordinary diagnostics. | I |
| TEST-24 | Unknown renderer, duplicate/repeated cursor | Partial useful page without panic; no infinite pagination; parse failure distinct from empty library. | U/I |
| TEST-25 | Like and playlist add/remove | Correct specific occurrence changed; success verified; timeout triggers reconciliation rather than duplicate retry. | I/A |
| TEST-26 | Ten-thousand-row content | Virtualization works, stable focus/row identity, responsive scrolling and no full-list work every progress tick. | I/H |
| TEST-27 | Keyboard, IME, clipboard, screen reader | Every required action reachable and named; text input shortcuts/IME not intercepted by playback bindings. | H |
| TEST-28 | Resolver timeout and Quit | All owned helper/runtime work canceled/reaped, audio stopped, temp credentials removed within bounded shutdown. | I/H |
| TEST-29 | OS media keys and now playing | Commands affect the sole coordinator; metadata/progress match actual current playback. | H |
| TEST-30 | Two-hour playback/navigation soak | No crash/deadlock/ownership leak; track resource growth, start latency and recovery outcomes. | H/A |
| TEST-31 | Radio continuation/refill | No runaway requests, repeated-cursor loop or loss of manual queue priority; repeat-all policy respected. | U/I/A |
| TEST-32 | Full playlist mutation outcomes | Capability/confirmation correct; reordering/removal use occurrence identity; uncertain results reconciled. | I/A |
| TEST-33 | Lyrics missing, untimed and timed | Honest state, no invented timings, correct seek/scroll binding and no unsolicited third-party requests. | U/I/A |
| TEST-34 | Local vs service listening history | Failed loads/prefetch do not count; service confirmation and unsupported status shown distinctly. | I/A |
| TEST-35 | Owned local gapless fixtures | Measure adjacent decoder/output samples using a controlled fixture; no crossfade; report discontinuity/format limitations. | I/H |
| TEST-36 | Real network album transitions | Measure audible gaps and explain cold fetch/codec/output effects separately; no unconditional “perfect gapless” claim. | A/H |

## Fixtures and test infrastructure

Generate or use owned audio fixtures with known duration and continuous boundary samples. Include AAC/Opus containers actually supported by the chosen engine build; do not assume every codec has the same priming/padding behavior. A controlled local HTTP server may simulate range responses, delay, truncation, redirects, expiry and errors. Production URL restrictions must remain enabled; test-only localhost permission is explicit and cannot leak into production.

Use sanitized structural provider fixtures for search, albums, playlists, unavailable songs, missing metadata, multiple locales, unknown renderers and continuations. Preserve intentional duplicate occurrences. Never check in raw authenticated responses and attempt to redact them only after publication.

Use fake clocks and controllable completion ordering for retry, expiry, progress, generation and shutdown tests. State-machine/property tests should exercise operations in unexpected sequences, not just one linear happy path. Simulate media-device failure independently of network failure.

## Completion reporting

At each milestone, report implemented behavior, exact tests run and their results, tests not run, host/tool versions and remaining restrictions. Distinguish mocked, controlled integration and real-account evidence. A compiled binary is not a playback test; a returned stream URL is not a complete-track test; a screenshots-only review is not an accessibility test.

Security, wrong-account writes and stale playback ownership are release blockers. Unknown live account compatibility blocks claims of a working YouTube player, but not honest progress on pure state, UI fixtures and local media integration. A failure in an optional lyric, radio or tray feature must not prevent the reliable core from being usable.
