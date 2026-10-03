# Build specification — personal Rust + GPUI YouTube Music player

**Version:** 1.0, research baseline 2026-10-02.  
**Audience:** coding model and human owner.  
**Status:** implementation-ready design with explicit feasibility gates; not a tested application.  
**Companions:** [research and sources](RESEARCH.md), [implementation plan and acceptance tests](IMPLEMENTATION_PLAN.md), [coding-model handoff](CODING_MODEL_HANDOFF.md), [observed dependency baseline](DEPENDENCY_BASELINE.json).

## 1. Product contract

Build a desktop YouTube Music player whose application logic and interface are written in Rust and GPUI. It is for one person's own full YouTube Premium subscription, with no distribution, hosted backend or other users. The application must remain responsive while playing, browsing, seeking and refreshing account data.

This is a native interface, not a browser wrapper. GPUI owns all ordinary application windows and controls. The design permits native libraries and a replaceable external resolver: Rust + GPUI does **not** mean every dependency must be implemented in Rust. It does not permit silently replacing GPUI with Tauri, Electron or a web page.

The user did not specify an OS. Target the coding environment's host desktop OS first, record the OS/architecture and tested toolchain in the build baseline, and keep only genuinely platform-specific services behind adapters. Do not assume macOS, Linux or Windows based on the user's timezone. Do not claim all-platform support after a single-platform build.

The core scope is authenticated music discovery/library access and reliable playback. It is not an extractor research project, a custom audio-codec project, a streaming-service aggregator or an attempt to clone every YouTube Music screen.

### Requirement language

**MUST** is a required invariant or release gate. **SHOULD** is the default design, with deviations recorded in a short architecture decision record (ADR). **MAY** denotes a later extension. Numeric budgets below are proposed engineering targets, not measured performance claims.

External factual findings are linked to source IDs in RESEARCH.md. The remaining behavior is the specification's own product/engineering decision.

## 2. Decisions to adopt

| Area | Default decision | Reason and boundary |
|---|---|---|
| Interface | Upstream GPUI + its matching platform crate, pinned together | Native Rust interface without forking the editor. Framework APIs are pre-1.0. [R09] |
| Components | Small application-owned components first; GPUI Kit only through a coordinated dependency migration | Do not mix GPUI types from incompatible upstream and snapshot packages. [R11] |
| Metadata/account provider | Adapt the minimum necessary portion of Limusic's Rust InnerTube crate | Broad useful endpoint/model boundary already isolated from its UI and player. [R02] |
| Stream resolution | Version-pinned yt-dlp executable, resolution only, with matching EJS and supported Deno runtime | Keep service-specific extraction churn independently replaceable. [R20–R22] |
| Media engine | libmpv through a small libmpv2 adapter | Avoid implementing demux, codecs, resampling, buffering and output drivers in v1. [R03, R23–R24] |
| Concurrency | GPUI main loop, explicitly owned Tokio service runtime, dedicated media event/command handling | No blocking I/O or media work on the UI thread. |
| Persistence | SQLite for non-secret state; OS vault for credential material | One durable authority; no web localStorage mirror. |
| Desktop integration | Souvlaki where it fits; small platform adapter where it does not | Reuse the same playback commands; validate actual event-loop/window integration. [R25] |
| Account scope | One authenticated account and selected YouTube identity | No anonymous product mode, account switching or tier matrix in v1. |
| Delivery | Local source build, explicit dependency updates | No installer matrix, auto-updater service, telemetry or account server. |

### Alternative decisions, not simultaneous implementations

Sonora's `ytmusic-rs` is a relevant later in-process resolver candidate, not a second resolver to build immediately. Its inspected client includes anonymous operation, persistence options and an optional browser token host; those behaviors require review before adoption. Do not inherit Sonora's GPUI fork or unrelated service/DRM stack merely to get its YouTube adapter. [R13–R14]

`ytmapi-rs` is an alternative when deliberately narrowing to its supported endpoints. Its documented gaps make it less suitable as the default for the whole proposed scope. Do not keep both it and a Limusic-derived parser stack in production without an explicit need. [R12]

The coding model may replace a default after the feasibility gate, but must state the observed failure or measurable benefit, the changed dependency/security implications, and the tests that remain satisfied. Cosmetic preference alone does not justify a new audio engine or extractor.

## 3. Service, subscription and risk boundaries

### AUTH-01 — Premium-only product assumption

Require a valid authenticated session and the owner's declaration that the selected account has full YouTube Premium or YouTube Music Premium. Do not support logged-out listening. Settings, help, local diagnostics and sign-in remain usable without a session; content browsing and playback are gated.

Maintain three independent concepts:

| Concept | States | Interpretation |
|---|---|---|
| Session | absent, validating, valid, expired, temporarily unreachable | Can the service authenticate this session? |
| Identity | selected, ambiguous, changed | Which account/channel's library and actions are in use? |
| Premium evidence | owner-declared, observed-positive, observed-negative, unknown | What is actually known about the assumed membership? |

There is no documented public entitlement endpoint established by this research. Never invent one, use channel subscription/membership endpoints as a substitute, or infer Premium from an absent ad or a particular bitrate. An unavailable undocumented signal is not proof of a non-Premium account. [R15, R18–R19]

**Default gate:** valid session + confirmed selected identity + owner declaration. Display “Premium assumed for this account” unless a maintained account signal actually supports a stronger statement. A reliable negative signal blocks new service actions and explains why. A transient inability to refresh evidence retains the declaration, not a fabricated verification badge. Do not build anti-tamper licensing for a private application.

### AUTH-02 — No silent access-policy fallback

Metadata requests use the selected authenticated context. The resolver must use a tested recipe that retains account authentication for stream selection, not merely receive cookies while silently selecting a guest-only client. Generic public assets/bootstrap requests may be credential-free when appropriate; that is not permission to play through an anonymous fallback.

On account, membership, region, age, DRM or access restrictions, show a typed error and an explicit “Open in YouTube Music” action. Never change accounts, forge eligibility, use an alternate public proxy, strip DRM or change regions to make a track play.

### RISK-01 — Informed unofficial-client scope

The official APIs and embedded player do not provide a sanctioned equivalent of this audio-first architecture. Personal use and a paid subscription do not make unofficial access authorized or eliminate account risk. Store a local acknowledgement of this limitation; do not present a legal-compliance guarantee. [R15–R17, R20]

There is no offline-download/export feature in this specification. Transient media buffering needed for playback is permitted; a durable playable music cache is not. Do not implement ad-blocking, downloading, social relay, credential sharing or playback circumvention as incidental “conveniences.”

Private reuse of GPL sources is viable without public release of private modifications, but retain notices and provenance. Reassess obligations if distribution ever becomes a goal. [R04, R07, R27]

## 4. Scope and functional surface

### First usable version: required

**Account and recovery.** Explicit local session import; active identity display; Premium assumption acknowledgement; logout; reconnect; safe diagnostics. No Google password input in GPUI.

**Discovery and library.** Search songs/albums/artists/playlists, paginated result lists, album and playlist detail pages, artist summary/album navigation, library playlists and liked songs. Preserve unavailable entries visibly. Open supported YouTube/YouTube Music track and playlist links without substituting another recording.

**Playback.** Play/pause, stop, next, previous, seek, volume/mute, queue editing, shuffle, repeat off/one/all, actual playing-track metadata, progress and buffering state, persisted queue and paused restore. One active playback owner.

**Basic account writes.** Like/unlike, add to an existing editable playlist, remove a specific occurrence from an editable playlist. Gate writes by returned capabilities, reconcile after success, and never claim success on timeout. Use a disposable private playlist for live acceptance tests.

**Desktop quality.** Keyboard operation, working text input/IME, accessible names and focus, supported media keys/now-playing integration on the primary host, sane shutdown, offline/error states, and a diagnostics panel without credentials.

### Follow-on release: valuable, not allowed to delay the first usable player

Home/recommendation shelves; robust radio continuation/autoplay; full playlist create/rename/description/reorder/delete; saved albums and artist follows; service-provided lyrics; service-history display; verified service-history reporting; tested near-gapless network transitions; output-device preference; optional tray/background operation and mini-player.

### Explicitly deferred

Offline media downloads, arbitrary media export, music videos, podcasts/live streams, uploading/deleting the owner's uploads, local-file library management, multiple service providers/accounts, crossfade, equalizers, pitch/time processing, external lyric scraping, lyrics translation, Discord/Last.fm, social listening rooms, auto-updating binaries, auto-start and public distribution.

These are scope decisions, not claims that the features are inherently impossible. User uploads may later be supported as a distinct source type after testing their identity and permission requirements.

## 5. Architecture and ownership

```text
GPUI views + actions + UI state
                │ typed commands / immutable snapshots
                ▼
Application coordinator (sole queue/playback/account authority)
      ├── Account service ── OS vault / ephemeral session
      ├── YouTube metadata adapter ── authenticated HTTP
      ├── Stream resolver adapter ── yt-dlp + JS runtime
      ├── Media engine adapter ── libmpv ── audio device
      ├── Store ── SQLite / metadata + artwork cache
      └── Desktop adapter ── media keys / now playing / lifecycle
```

Use one workspace, not a network of microservices. A suitable starting layout is five crates: `domain` (pure types/state rules), `services` (provider/resolver/storage/account), `media` (libmpv wrapper), `desktop` (OS services), and `app` (coordinator and GPUI). Fewer crates with these module boundaries are acceptable. Do not create a package per screen.

### ARCH-01 — Dependency direction

Domain types must not depend on GPUI, InnerTube JSON, yt-dlp output structures or mpv types. The UI must not know cookies, private endpoint names, executable command arguments or signed stream URLs. Provider structs convert into domain models at the boundary; unknown service fields stay out of presentation logic.

The coordinator is the sole authority for queue order, selected item, user playback intent and account epoch. The media engine is authoritative for actual decoder/output state and media time. UI views and OS integrations project these states; they do not run independent players.

### ARCH-02 — Runtime bridge

Initialize a dedicated, bounded Tokio runtime for service I/O. Do not assume GPUI's executor supplies a Tokio reactor. No `block_on`, synchronous SQL, HTTP, process wait, image decoding or mpv event wait in render/input handlers. Use GPUI's supported update mechanisms to apply results on its owning thread. Never send a GPUI context/entity into arbitrary worker threads.

Commands are ordered and bounded. Control completions/errors must not be dropped; frequent progress/buffering snapshots are coalesced. Use explicit cancellation for search, page loads, stream resolution and preload. Store subscription/task handles for their intended lifetime and dispose them when their entity/session closes.

### ARCH-03 — Correlation identifiers

Every asynchronous job/event must carry enough identity to reject stale work: account epoch, relevant request generation, queue-entry ID, media-load ID and, where necessary, queue revision. Tokens have different scopes; do not overload one global counter for everything.

Changing a search query invalidates earlier search results. Changing the account invalidates all private requests and caches. Skipping invalidates the previous active load. Changing the upcoming queue invalidates only affected preloads. Late completion must neither start sound nor overwrite a newer view/state.

## 6. Domain model and service contracts

The following are conceptual contracts, not copy-ready Rust API signatures. Choose concrete Rust types consistent with the pinned dependencies.

| Model | Required content / invariant |
|---|---|
| Track | Opaque source video ID; title; artist references; optional album reference; optional duration; artwork candidates; explicit/availability flags when known; source kind. |
| QueueEntry | Unique local instance ID plus Track reference; origin/context; insertion/rank metadata. Two occurrences of the same video are distinct entries. |
| Album / artist / playlist | Opaque browse/playlist IDs; metadata; capabilities; paginated children. A playlist occurrence may have an additional server occurrence token. |
| Page | Items, optional continuation, source identity/query, partial-warning information. Never persist a cursor as globally valid across accounts or query revisions. |
| AccountContext | Selected account/channel identifiers, locale, credential reference and epoch; no user-facing raw cookie content. |
| StreamLease | Expected track ID, resolved media URL, constrained per-resource headers, protocol/container/codec, optional bitrate/sample rate/duration, expiry if known, identity/quality binding. Secret-bearing and memory-only. |
| PlaybackSnapshot | Queue entry/load identity; requested intent; actual engine phase; media position/duration; seekability; buffering; volume; recoverable error. |
| ProviderCapabilities | Available operations and item-level editability. Missing capability produces an honest disabled/unsupported action. |

Separate `VideoId`, `BrowseId`, `PlaylistId`, `PlaylistItemId` and `QueueEntryId` concepts; do not key everything by display name or URL. Artist and album names are not identifiers. Unknown values remain optional rather than zero/empty placeholders that imply certainty.

### Provider operations

The metadata adapter supports session validation/account identity, search with pagination, track/album/artist/playlist detail, library pages, rating and specific playlist mutations. Later operations add home, radio, history and lyrics. Reads and writes have separate retry policies. Unsupported operations must return `UnsupportedCapability`, not empty success.

The resolver accepts a track identity, current authenticated context, quality policy and cancellation token. It returns a validated StreamLease or a typed failure. It never edits a playlist, updates history or changes the queue.

The media engine accepts validated media leases and commands; emits load/state/position/end/error events; and supports dropping/replacing a preload. It never obtains credentials, calls metadata endpoints, extracts a YouTube watch page or decides the next logical queue entry.

## 7. Authentication and secret handling

### AUTH-03 — First-version sign-in

Use the owner's ordinary browser for Google sign-in. Provide an explicit, local-only session importer into the application's credential store. Prefer a scoped cookie-jar import preserving origin/path/expiry attributes. Import only the required YouTube account cookies from the user-selected profile/export, not an entire browser's cookies.

A raw Cookie-header paste may be supported as an advanced alternative, but it lacks full cookie attributes; treat it as an ephemeral seed with that limitation visible. Never fabricate a long-lived authenticated session just by assigning a future expiry. Do not claim opening the browser automatically gives the native app the browser's cookies.

Do not request Google passwords, recovery codes or two-factor secrets. Do not ask the user to paste session data into an AI conversation. No stealth browser-profile scanning or remote debug port. An embedded login/browser helper is a later ADR with its own compatibility and security tests, not the assumed MVP login. Google OAuth embedded-user-agent restrictions and yt-dlp's current cookie requirements make a generic “just use OAuth in a webview” plan incorrect. [R20, R28]

### AUTH-04 — Validate and bind

After import, validate through an authenticated account-specific operation and show the selected identity before enabling content actions. A public search result is not validation. Brand/channel identity selection must be explicit when the account is ambiguous; do not infer the library owner solely from email or an account index.

Keep a fresh validation result within the active session. Revalidate on startup/import/reconnect, suspicious authorization errors and periodically during long sessions. An initial 30-minute refresh interval is a tunable policy, not a claimed cookie lifetime. Deduplicate concurrent refreshes. A network failure is not automatically logout; a reliable expired-session response blocks new authenticated work and asks for re-import.

### SEC-01 — Storage

Store small credentials directly in the OS vault when practical. For a larger cookie jar, use a vetted authenticated-encryption library with a random key in the vault and ciphertext in the application's private directory. Avoid custom cryptographic constructions. If the store is unavailable or locked, offer memory-only mode; never silently downgrade to plaintext durable storage.

Prevent credentials from appearing in logs, panic messages, tracing spans, SQLite, diagnostics exports, source control, command-line arguments or crash-report attachments. Secret-bearing models must have redacted formatting. Cover both raw and URL-encoded variants in tests. Artwork URLs and ordinary metadata may also expose private information; diagnostics exclude them by default.

### SEC-02 — Resolver credential handoff

Prefer a narrowly scoped local handoff rather than secrets in argv/environment. A CLI that requires a cookie file may use a private, short-lived file restricted to the current user: restrictive directory/file modes on Unix and appropriate ACLs on Windows, symlink-safe creation, deletion on completion/cancel, and stale-file cleanup on startup.

This is an explicit temporary plaintext exception, not encrypted-at-rest storage. File deletion is not guaranteed forensic erasure, especially on journaled/SSD filesystems. Document the tradeoff. Do not put the file in a shared predictable directory.

The account service owns the canonical jar. Give each resolver request an isolated snapshot; merge valid cookie changes only if the account epoch still matches, and avoid an older request overwriting a newer rotation. When metadata and resolver refresh semantics disagree, invalidate/re-import rather than silently mixing identities. Test that rotated cookies survive restart where the provider supports it.

### SEC-03 — Logout

Stop playback, cancel/reap active resolver jobs, drop leases and in-memory cookies, delete the vault entry/encrypted session, and invalidate private caches/queue state for that account. Do not leave the former account's library visible after logout. Allow the owner to retain only explicitly non-private settings. In-flight writes already accepted by the remote service cannot be undone by canceling locally; report reconciliation needs honestly.

## 8. Metadata adapter requirements

### DATA-01 — Controlled reuse

Vendor/adapt only the required Limusic InnerTube files into an isolated subtree with source revision, license and local patch notes. Audit automatic recovery, client fallbacks, cookies, logging and persistence before enabling them. Do not port Tauri commands or UI-owned state wholesale. Do not copy credentials/API secrets from another app.

### DATA-02 — Parsing and pagination

Use semantic IDs and endpoint/capability data where available, not English tab names or fragile display-text matching. Keep locale explicit. Unknown renderer variants must not panic or blank the entire page: skip unsupported optional elements, keep supported content, and report a sanitized parser warning.

Distinguish an actually empty library from a failed/unknown parse. Cap response sizes/depth and deduplicate overlapping pages. Stop on repeated continuation tokens. Preserve original album/playlist sequence; never deduplicate intentional duplicate playlist occurrences by video ID.

Cache keys include account identity, locale, endpoint/query and schema version. Implement bounded stale-while-revalidate for metadata; private caches never cross identity epochs. Cache staleness is visible when the network is unavailable.

### DATA-03 — Remote mutation semantics

Optimistic likes may be shown as pending and rolled back on known failure. Playlist changes require the server's occurrence identity where applicable. After a successful mutation, refresh the affected page/capability and reconcile.

A timeout after sending a non-idempotent write is an unknown outcome, not proof of failure. Read back before retrying to avoid duplicate additions or deletes. Do not replay an offline mutation queue in v1. Destructive later actions require explicit confirmation with the target name/count.

### DATA-04 — External links

Parse URLs with a real URL parser and exact host rules. Support ordinary YouTube/YouTube Music watch/playlist and short-link forms; resolve ambiguity through the provider. Never pass arbitrary pasted text as shell arguments or open local/network media paths through this entry point. Keep track identity fixed; do not replace an unavailable album recording with a similarly titled video.

## 9. Stream-resolution service

### RES-01 — Reproducible helper environment

Use an explicitly configured, version-pinned yt-dlp executable and supported JS runtime. Record executable paths, versions, hashes and EJS packaging/source. Match the EJS version to the resolver rather than independently installing arbitrary “latest” components. Do not depend on user-wide yt-dlp configuration, arbitrary plugins, shell aliases, automatic updates or runtime remote-component downloads. [R22]

Launch with an argument array, never a shell command string. The controlled invocation must request metadata/stream resolution without downloading media, processing a playlist, writing thumbnails/subtitles or running postprocessors. Verify supported flags against the installed version rather than copying an untested command from this document.

Read machine-readable output through bounded pipes, isolate diagnostic output, enforce cancellation/timeouts, and terminate/reap the whole owned process group or Windows job as appropriate. Keep secrets and signed URLs out of surfaced stderr. Process separation improves failure isolation but is **not** an OS security sandbox by itself. Preserve the JS runtime's supported restrictions; do not broaden its filesystem/network privileges for convenience.

### RES-02 — Account-aware selection

The first spike must establish a cookie-compatible, authenticated client selection with the real account and selected identity. Start by testing without a PO-token browser service, because current upstream guidance reports the Premium GVS exemption. Do not hardcode an assumption that the exemption or a particular client remains valid forever. [R21]

Keep resolver policy outside the UI and record it alongside the dependency baseline. Never use an anonymous-only fallback to salvage a failed Premium session. Report “authentication path not verified” if the adapter cannot establish the required behavior; do not call mere presence of a cookie-file argument proof.

Do not put fragile private client constants in dozens of call sites. When all allowed authenticated paths fail, emit a service-compatibility error and keep the application usable. Adding a browser attestation service later requires an explicit ADR and a new security/resource budget, not an invisible fallback.

### RES-03 — Validate the returned media

Require the expected source track ID, an allowed HTTPS media URL, supported non-DRM transport, audio presence, and usable required headers. Reject mismatched IDs, arbitrary local files, unsupported URL schemes and untrusted hosts. Use parsed domain boundaries rather than substring checks. Manifest and redirect resources require the same scrutiny.

A resolver may return formats that libmpv cannot consume as ordinary HTTP media; in particular, do not assume a SABR-only result is a direct audio URL. The initial supported transport is an ordinary authenticated-account-derived HTTPS audio resource. Additional manifest transports require their own tests. DRM-only/unsupported results are explicit failures, not prompts to strip protection.

Scope headers per lease/resource. Never forward the entire account Cookie/Authorization set to every CDN host or redirect. Preserve only headers the selected transport legitimately needs, with origin/redirect restrictions. If the binding cannot isolate headers per file safely, the build gate must resolve that limitation before shipping; a narrowly scoped authenticated local media broker is a possible later ADR, not a generic open proxy.

### RES-04 — Quality and expiry

Default quality is “best available audio for this authenticated account.” Preserve the original codec; no unnecessary transcoding, upsampling or video download. Rank appropriate audio formats using the resolver's maintained policy and observed metadata, not one universal numeric bitrate comparison across codecs.

Display codec/container and bitrate/sample rate only when actually known. Separate desired quality from actual selected quality. Do not label an estimate as exact, promise 256 kbps for every track, or claim lossless/bit-perfect output. [R18]

Stream leases are memory-only. Store track IDs, never signed URLs, in the queue database. Use a reported expiry when available with a safety margin; unknown expiry permits immediate use but not indefinite cache reuse. Re-resolve after restart. A speculative next-track resolution must be invalidated by identity/quality/queue changes and refreshed before use when stale.

## 10. Playback coordinator and queue

### PLAY-01 — State machine

Expose at least: idle, resolving, opening, buffering, playing, paused, seeking and failed. Retain user intent separately from actual engine state. “The user requested play” is not evidence audio started. Successful loading does not override a pause entered while loading.

Each load has a unique ID. Commands and completion events carry the corresponding queue-entry/load IDs. A late completion from a skipped track is discarded and cannot seize audio output. An error from a speculative next track must not stop the currently audible track. An audio-device failure is handled differently from a media failure.

The coordinator owns logical advancement. Where the engine later auto-advances a preloaded playlist entry, treat that event as confirmation of one transition rather than triggering another `next`. EOF/error/preload notifications may arrive in different orders; transition once per logical load.

### QUEUE-01 — Explicit queue semantics

Maintain played history, current entry, manual upcoming entries, source-context upcoming entries, and later radio entries. Each occurrence has its own queue-entry ID, even when several entries share a video ID.

“Play now” replaces the current context only after any required user confirmation; “Play next” inserts immediately after the current entry, preserving order within a selected batch. A later Play-next batch precedes an earlier batch. “Add to queue” appends to the manual segment, ahead of untouched context/radio items. Show these semantics consistently.

Shuffle changes only the unplayed source-context order, not played/current/manual items. Preserve a canonical order and seed so toggling shuffle off is deterministic. Explicit drag placement is authoritative: an item moved into the manual segment leaves the shuffled pool. The active entry itself cannot be moved into the future; removing its queue row marks it removed from future cycles but does not stop the audible track unless Stop is requested.

Previous seeks to the beginning when position exceeds three seconds; otherwise it returns to the preceding played entry when one exists. At the beginning of history it seeks to zero. A user-issued Next overrides repeat-one. Automatic EOF honors repeat-one. Repeat-all cycles the explicit context plus manual additions, not an ever-expanding radio stream; suspend radio refill while repeat-all is active.

Persist queue instance IDs, ordering, context, shuffle/repeat settings and the last known position. Restore **paused**, validate the account, and obtain a new stream on resume. Never auto-start audio on application launch.

### PLAY-02 — Seek, volume and end handling

Scrubbing previews a position locally; commit a bounded seek on release/keyboard action. Preserve pause/play intent across the seek. Clamp to known duration and respect non-seekable content. Cancel obsolete seek completions. Report decoder-confirmed position rather than blindly displaying the requested position forever.

Default volume is conservative; persist the owner's choice. Mute remembers the prior volume. Disable boosts and sound processing in v1. A future loudness feature must explicitly avoid double application and distinguish attenuation from clipping-prone gain.

On unavailable items encountered during automatic progression, skip at most three consecutive failures, show the skipped entries/reasons, then stop and surface recovery. Do not rapidly retry an entire unavailable playlist. A directly requested failed track stays selected with an error instead of unexpectedly playing something else.

### PLAY-03 — Preload and gapless progression

For the first usable version, sequential playback and correct cancellation are required; gapless is not claimed. Permit next-track resolution only for the immediate next entry and only when useful. Active playback requests take priority over speculative work.

The follow-on gapless implementation should keep one libmpv instance and its output stream open where possible, with one-track lookahead. Investigate actual decoder/network prebuffer behavior; merely appending a URL or enabling `gapless-audio` is insufficient. Crossfade is a separate feature and must not be used to disguise a gapless failure. [R23]

Before promoting this feature, pass both owned local fixture tests and real-account network transition tests. Document format-switch limitations and the difference between sample-contiguous local output and best-effort network transitions. Never resolve the entire playlist or cache whole albums merely to claim instant skips.

## 11. Media engine and native dependency integration

### MEDIA-01 — Small, deterministic wrapper

Disable user mpv configuration/scripts, external downloader hooks and video output for this app-owned instance using supported options verified against the pinned native library. Only play URLs validated by the resolver. Do not allow a configuration file or playlist to cause unexpected filesystem/network access or launch another extractor.

Manage the library lifecycle on a dedicated media owner/thread arrangement supported by the binding. The event pump must not call GPUI directly. Preserve typed engine errors where possible; do not label every generic load failure “YouTube blocked you.” Do not hold application locks while blocking in FFI or waiting for callbacks.

Keep HTTP/demux buffering bounded and memory-backed in the default configuration. No persistent audio cache. The active stream plus at most one lookahead is the only allowed speculative media footprint. Monitor total process/native/GPU/helper memory, not only Rust allocator counters.

### MEDIA-02 — Output device and lifecycle

Use the current system default output device initially. Pause conservatively on output-device loss/disconnect or an ambiguous resume transition; never unexpectedly blast audio through a newly selected device. Offer a visible resume/retry action. Treat loss of the output device as a device event, not an instruction to skip songs.

Sleep/resume tests must cover stale URLs and device changes. Seeking/reopening after resume must preserve user intent and last actual position. Do not reconnect forever in the background after the owner has paused or exited.

Record native mpv version/build options, available AAC/Opus support, runtime library path and binding version. Private use removes the installer matrix, not the need to resolve native libraries correctly. Do not globally disable OS security protections to make a binary run.

## 12. Resilience and network discipline

### NET-01 — Bounded concurrency and retries

Initial limits: four concurrent metadata reads; one active resolver plus one immediate-next resolver; serialized account refresh; serialized conflicting writes per resource. Deduplicate identical in-flight reads. These are tunable defaults, not service-published limits.

Use finite timeouts with cancellation. Initial interactive metadata deadline: 15 seconds; resolver deadline: 30 seconds. Keep the UI responsive and provide cancel/retry. Respect `Retry-After`. Read retries use capped exponential backoff with jitter and at most two automatic retries. Never retry non-idempotent writes blindly.

On likely expired media authorization, invalidate and re-resolve once for the same track, then reopen near the last known position while preserving intent. Do not interpret every HTTP 403 as an expired URL: account/access/region/client failures have different remedies. Stop automatic recovery after the bounded attempt and show the classified failure.

After repeated service-wide failures, open a circuit breaker and suppress prefetch/refill. Allow an explicit retry and recovery after backoff. Rate-limit errors, challenges and authorization failures must not trigger brute-force client rotation.

### NET-02 — Failure vocabulary

At minimum distinguish expired session, ambiguous identity, negative Premium evidence, restricted/unavailable track, service throttling, temporary network failure, resolver/tool missing, resolver protocol incompatibility, unsupported media/DRM, expired lease, output-device failure, storage failure and unknown outcome of a write.

Failures include a safe user-facing explanation and one appropriate recovery action. Keep lower-level detail in redacted local diagnostics. An unavailable optional shelf, lyric or integration does not interrupt music.

## 13. Persistence, caches and privacy

### STORE-01 — Single authority

Use schema-versioned SQLite with transactional migrations for settings, account references, entity metadata, queue entries, queue state and optional local listening events. Use WAL/busy handling appropriate to the selected library, with database work off the UI thread. Back up before destructive migration and fail safely on unsupported newer schemas.

Persist structural queue changes transactionally. Checkpoint position every five seconds and on pause/exit rather than every progress tick. A crash can lose a few seconds of position, not corrupt the queue or replay an old account.

Public artwork and metadata caches are bounded and independently clearable. Initial budgets: 256 MiB disk artwork, 64 MiB decoded artwork/textures and a bounded metadata store. Keep image dimensions/response sizes capped. Actual GPU consumption must be measured because it may not match a CPU cache count.

Never store secrets, media leases or playable audio in SQLite. Private metadata has account-specific keys and purge behavior. Cache a failed request only for a short, typed interval; do not convert a parser failure into a long-lived empty library.

### PRIV-01 — No outbound telemetry

No analytics, crash upload, listening telemetry to the app author, third-party lyric requests or social integrations by default. Google requests necessary for the selected service remain part of playback/account operation and should be explained separately.

Local history is not YouTube account history. Label them distinctly. Service-history reporting is a later capability that must be verified with actual listens; prefetch, failed loads and seeks must never count as completed plays. Do not invent playback beacons or assert that a track was recorded remotely when the operation is unsupported/unconfirmed.

Diagnostic export is manual, previews included fields, and excludes browsing queries, track titles, account names, cookies and signed URLs by default. Version/build information, error categories and anonymous timing counters are usually sufficient.

## 14. GPUI functional interface requirements

This section defines behavior, not visual styling. Palette, typography, spacing, illustration, component aesthetics and brand treatment belong to the style model.

### UI-01 — Required surfaces

The shell contains navigation/history controls, search access, content area and a persistent player area. Core routes are account/setup, search, library, album, artist and playlist. Queue and settings are independently accessible. Later home/lyrics/history routes reuse the same navigation and loading/error patterns.

Every network view has distinct initial-loading, cached-refreshing, empty, partial, error and disconnected states. “No songs” must not be the generic error screen. The player distinguishes resolving/opening/buffering/paused/error rather than freezing a spinner without explanation.

Preserve navigation and scroll position where appropriate. Paginated lists and large queues must use GPUI's appropriate virtualization primitives. Stable row identities must use entity/occurrence IDs rather than mutable list indices. Artwork loading must not rerender or remeasure the whole library every time an image arrives.

### UI-02 — Actions and input

Define semantic actions consumed by both keyboard and pointer controls: search, back/forward, play/pause, next/previous, seek, queue insertion/removal, like, volume and mute. Scope Space and arrow bindings so they do not steal text-entry, IME or slider interaction. Provide platform-appropriate primary modifiers and a discoverable shortcut reference.

Do not hand-roll a minimal ASCII text box and call it done. Validate selection, clipboard, Unicode, composed input/IME, focus restoration and context menus on the primary OS. Adopt a proven compatible input component or pinned GPUI example as necessary, without importing an incompatible component graph. [R09, R11, R29]

Keep high-frequency player progress updates localized. Derive visual interpolation from a recent media-time observation plus monotonic elapsed time only while actually playing; resynchronize after pause/seek/buffer/end events. Never use wall-clock passage as the source of audio truth.

### UI-03 — Accessibility

All interactive controls need accessible names, roles, state and keyboard focus; sliders expose value/range; icon-only buttons have names. Loading/error updates must be understandable without relying on color or animation. Respect reduced motion and maintain a complete keyboard path.

Use framework accessibility support where available, then test it on the real host assistive technology. Dependency presence or custom drawing is not proof of usable screen-reader behavior. Unsupported platform semantics must be listed as a release limitation, not hidden behind “native.”

### UI-04 — Style handoff contract

Expose semantic tokens for background/surface/text/emphasis/status, spacing, radius, type roles and motion. Make components accept domain view-models and state variants. The style model may change appearance and layout, but must not change queue semantics, security policy, loading/error meaning, keyboard behavior or the coordinator's state ownership.

Provide fixtures for long titles, multiple artists, missing artwork, unknown duration, explicit flags, mixed scripts, unavailable tracks, duplicate playlist entries and every playback state. Do not fetch real account data just to preview a component.

## 15. Desktop behavior

### OS-01 — Media integration

Route media key and OS now-playing commands into the same coordinator actions as the UI. Publish actual state, current metadata, duration/position and supported commands. Ignore obsolete callbacks for a previous load. Test hardware/media-overlay commands, not merely a mock adapter. [R25]

Keep the platform event-loop/window handle ownership compatible with GPUI. Do not add a second winit/Tauri event loop to satisfy a library example. Fall back to a small native adapter only when the shared abstraction demonstrably cannot integrate.

### OS-02 — Windows, closing and shutdown

Default window close exits the application and stops audio. A later explicit “keep playing when closed” option may enable tray/background behavior; it must provide an obvious Quit path. A missing Linux tray host must not make the app impossible to reopen.

Use single-instance behavior where practical so two launches do not unexpectedly create two audible players. A second launch may send a validated open-link action to the first instance; local IPC must be user-scoped and restricted to a small command schema.

On shutdown, reject new work, checkpoint state, stop audio, cancel and reap resolver/runtime jobs, clean temporary secrets, then exit. Impose bounded shutdown rather than waiting indefinitely for network requests.

## 16. Proposed quality budgets

Measure on the recorded primary host with release builds; include native libraries and child processes. These targets are starting acceptance goals, not source-derived performance promises.

| Metric | Initial target / required measurement |
|---|---|
| Local interaction | p95 input-to-state acknowledgement under 100 ms; no long synchronous UI-thread I/O. |
| Rendering | Smooth 60 Hz-class interaction where the host supports it; profile frames above 16.7 ms rather than claiming universal 60 fps. |
| Lists | Search/library fixtures with 10,000 rows remain navigable through virtualization. |
| Progress | Media observations about 4 Hz; animation localized and disabled/throttled when not visible. |
| Playback startup | Record resolve, connect/buffer and first-audio times separately; target p95 under 3 seconds on the documented warm-session/network test, with failures reported rather than hidden. |
| Stability | Two-hour playback/navigation soak without crash, deadlock, stuck playback ownership or continuing memory growth after warm-up. |
| Idle | No whole-window continuous redraw while paused; child resolver exits when idle. Record CPU/GPU rather than publishing an invented memory promise. |
| Restart | Queue/order/settings preserved; no automatic sound; no stale signed URL reuse. |
| Secret hygiene | Zero credential/signed-URL matches in automated canary scans of logs, DB, export and child argv. |

A missed latency target caused by the service/network is recorded with a trace, not “fixed” by increasing aggressive prefetch or bypassing authentication. Correctness/security gates take precedence over cosmetic benchmark numbers.

## 17. Validation and release gates

The [implementation plan](IMPLEMENTATION_PLAN.md) supplies executable acceptance scenarios. Unit/property tests cover pure state, parsers, URL policy and retries. Integration tests use sanitized fixtures and controlled local media/HTTP servers. Manual local account tests use the owner's consent and never run automatically in shared CI.

No live credentials, private playlists or raw authenticated responses belong in fixtures or screenshots. Redact data before committing and preserve enough structural variation to test parsers. Successful tests on public videos do not prove Premium/account behavior.

Before declaring the first usable version complete, demonstrate authenticated identity binding, ordinary track playback and seeking, correct queue/race handling, account writes with reconciliation, secure storage/logout, native input/media controls and dependency reproducibility. Home/radio/lyrics and gapless may remain disabled with explicit unsupported status until their follow-on gates pass.

## 18. Maintenance contract

Record every vendored source revision and modification. Keep dependency updates deliberate and reviewable. Run parser fixtures, account smoke tests, secret scans and media tests before updating the local installed build. Retain the previous known-working resolver/runtime combination for rollback; do not auto-download an unreviewed executable during playback.

When YouTube changes behavior, first classify whether the break is authentication, parsing, stream resolution, media transport or output. Repair the responsible adapter; do not refactor the UI or introduce multiple hidden fallback engines. Maintain sanitized compatibility notes including test date, selected client policy and known limitations, never credentials.

The owner must expect occasional compatibility maintenance. This document does not certify permanent service behavior, exemption from account enforcement, perfect gapless output or universal accessibility. It does define how to discover and report those limits without corrupting state or misleading the user.
