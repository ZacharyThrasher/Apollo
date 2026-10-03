# Research dossier — personal Rust + GPUI YouTube Music player

Research date: **2026-10-02**. Scope: public primary-source documentation and selected source files, not a complete security audit. No Premium credentials were supplied, no authenticated playback was attempted, and no proposed dependency combination was compiled. The specifications in this bundle are engineering decisions, not claims of tested compatibility.

## Findings that determine the architecture

**The requested application is technically plausible, but it is an unofficial client.** The public YouTube Data API reference does not document a native audio-stream playback SDK or a general YouTube Premium entitlement endpoint. YouTube API policies also constrain audio separation and hidden/background players. Therefore an official Data API + hidden iframe is not a supported equivalent of the requested native audio player. Personal use and payment do not remove service-terms/account risks. [R15–R17]

**Premium may substantially simplify resolution.** The current yt-dlp PO-token guide specifically says Premium subscribers do not need a GVS token. This is a current upstream observation about streaming-server requests, not an official lifetime guarantee or an exemption from every JavaScript challenge. Treat it as the first hypothesis to test with the actual account. [R21–R22]

**Native UI does not require a native implementation of every protocol.** The recommended product code is Rust with a GPUI interface. A separately replaceable yt-dlp executable and its supported JavaScript runtime handle resolution; libmpv handles decoding/output. This avoids coupling UI development to a new extractor and decoder implementation. It does introduce non-Rust/runtime dependencies and subprocess maintenance.

**The supplied applications are references, not interchangeable libraries.** Limusic has particularly useful Rust boundaries; Zuno's metadata layer is largely TypeScript; GPUI is not the renderer in either. Sonora, found through the GPUI ecosystem, is an additional directly relevant native reference. Read the source-level cautions below before reusing anything.

## Reference audit

| Reference | Observed evidence | Recommended use | Caveat |
|---|---|---|---|
| Limusic | Tauri/Svelte UI; separate Rust InnerTube and libmpv crates. | First-choice metadata adapter seed; playback/event lifecycle reference. | Stream/cipher/attestation logic is outside its pure metadata crate. Do not import its entire application. |
| Zuno | Tauri/React; Rust audio and TypeScript YouTube data source; several playback paths. | Queue, state ownership, account storage and failure-case reference. | Architecture document says 1.2.1 while inspected Cargo says 1.4.0. Treat prose as potentially stale. |
| Tono | Author describes a federated music client/server project. | Product reference only, pending source access. | Supplied Codeberg repository/API/raw file could not be retrieved in this research environment. Stack and license not verified. |
| Zed/GPUI | Entity/Render/action model, integrated executor, separate platform initialization; pre-1.0 APIs. | Framework source, examples, testing and lifecycle idioms. | Pin the entire compatible dependency graph; not a Zed fork. |
| awesome-gpui | Curated project/component directory. | Discovery of relevant native application examples. | A listing is not a compatibility, quality or maintenance guarantee. |
| Sonora | GPUI player workspace includes a dedicated Rust YouTube Music dependency. | Native-player architecture and later in-process resolver investigation. | Own GPUI fork and multi-service scope; copying the whole app brings unnecessary complexity. |
| ytmapi-rs / Youtui | Rust metadata API with documented coverage table. | Alternative for a deliberately narrower library-centric client. | Home/related not planned; account info and some other endpoints absent in inspected README. |
| Hummingbird | GPUI native music-player project. | Native library UI/media-integration patterns. | Does not establish YouTube authentication or resolution correctness. |

## Source ledger

All links below are primary sources. “Observed” refers to the particular file/page inspected, not to a successful local build. Live URLs may change after this dossier.

### R01 — Limusic product description
Source: [Limusic README](https://github.com/SimoHypers/limusic/blob/75e55dc8cf9d34660a6816d7be533ddf52af6dd0/README.md).

The README describes Tauri 2, SvelteKit, Rust InnerTube calls, libmpv and a hidden system webview for challenge execution. It advertises numerous features, including account/library actions and gapless playback. Those are upstream claims; this research did not run the application. “No bundled browser runtime” must not be read as “uses no webview.”

### R02 — Limusic metadata boundary
Source: [`crates/innertube/src/lib.rs`](https://github.com/SimoHypers/limusic/blob/75e55dc8cf9d34660a6816d7be533ddf52af6dd0/crates/innertube/src/lib.rs).

The module explicitly separates transport/models/endpoints from Tauri, mpv and webviews. It exports home/search/library-related models, account identity and session types. It explicitly excludes the later cipher/PO-token/WEB_REMIX streaming implementation. Recommendation: adapt the metadata boundary, not assume this crate alone solves playback.

### R03 — Limusic playback boundary
Source: [`crates/player/src/lib.rs`](https://github.com/SimoHypers/limusic/blob/75e55dc8cf9d34660a6816d7be533ddf52af6dd0/crates/player/src/lib.rs), first 200 lines inspected.

The wrapper accepts resolved media, not YouTube IDs. Its events distinguish active-file errors, lookahead errors and audio-device loss. It also guards asynchronous preload/fade completion with generations. Those distinctions inform the proposed coordinator tests; they do not establish a verified gapless guarantee.

### R04 — Limusic manifest/license
Source: [workspace Cargo.toml](https://github.com/SimoHypers/limusic/blob/75e55dc8cf9d34660a6816d7be533ddf52af6dd0/Cargo.toml).

Observed version: `1.1.0-rc.7`; license field: `GPL-3.0-or-later`. Preserve original notices and provenance in any vendored subtree. The retrieved source reference is `75e55dc8cf9d34660a6816d7be533ddf52af6dd0`.

### R05 — Zuno architecture
Source: [`docs/architecture.md`](https://github.com/noFAYZ/zuno/blob/main/docs/architecture.md).

Inspected prose describes Tauri/React, a TypeScript YouTube Music data source, Rust HTTP/session handling, multiple audio paths, and a hidden-iframe fallback. Its useful architectural lessons include one audible owner and distinguishing manual queue entries from automatically generated entries. Do not copy its multiple persistent state authorities into a native Rust application.

### R06 — Zuno actual audio dependencies
Source: [`src-tauri/Cargo.toml`](https://github.com/noFAYZ/zuno/blob/09b4acbbae8ed14b9c8f9daaaca041bbb6aa9c1b/src-tauri/Cargo.toml).

Observed application version: `1.4.0`. Manifest includes rodio, Symphonia and a separate libopus binding, plus platform-specific media control work. This supports treating a custom Rust decoder stack as a separate engineering project, not a trivial substitution for libmpv. Reference: `09b4acbbae8ed14b9c8f9daaaca041bbb6aa9c1b`.

### R07 — Zuno license
Source: [LICENSE](https://github.com/noFAYZ/zuno/blob/09b4acbbae8ed14b9c8f9daaaca041bbb6aa9c1b/LICENSE), opening section inspected.

Repository license file is Apache License 2.0. Transitive dependency and asset licenses still need separate review before reuse or redistribution.

### R08 — Tono access limitation
Sources: [supplied repository](https://codeberg.org/tono-fm/client), [author's site](https://mattia.id/).

The author describes Tono.Fm as a federated/decentralized music client and server. Repository page, API and raw-file attempts failed here. No claim is made about implementation language, framework, license, current endpoint support, or buildability. This is a research gap, not evidence of an abandoned or broken project.

### R09 — GPUI framework and license
Sources: [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [GPUI Cargo.toml](https://github.com/zed-industries/zed/blob/main/crates/gpui/Cargo.toml).

GPUI is pre-1.0. Current README uses `gpui_platform::application()` and describes platform feature requirements, Entity/Render/actions, executor integration and test support. Inspected GPUI manifest says `0.2.2`, Apache-2.0. That manifest version does not prove crates.io has identical APIs. Observed Zed main reference: `f37989fbdf840b308892af66516c2b9c3ef3853a`; use it as a candidate, not a certified baseline.

### R10 — GPUI ecosystem directory
Source: [awesome-gpui](https://github.com/zed-industries/awesome-gpui).

Useful for finding components and application examples, including music players. Assess each project's actual manifest, source and license rather than relying on the listing alone.

### R11 — GPUI Kit compatibility trap
Sources: [GPUI Kit site](https://gpui-kit.com/), [current workspace manifest](https://github.com/longbridge/gpui-kit/blob/main/Cargo.toml).

The inspected site advertises `0.6.0`; current main manifest uses workspace version `0.7.0` and exact `gpui-pre`/platform snapshot `=0.3.7`. Comments explain why broad version requirements broke builds. These are observations of different release surfaces, not a recommended mixed dependency set. Adopt Kit only as a coordinated graph, not alongside incompatible upstream GPUI types.

### R12 — ytmapi-rs coverage
Sources: [Youtui README](https://github.com/nick42d/youtui/blob/main/README.md), [ytmapi-rs docs](https://docs.rs/ytmapi-rs/latest/ytmapi_rs/).

Rust API based on Tokio/Reqwest. The README explicitly documents missing/not-planned home, related and account endpoints, with significant library/search/write support elsewhere. It is not the default here because the proposed application includes account identity and a later home/radio experience. It remains a credible alternative if requirements narrow.

### R13 — Sonora native-player reference
Sources: [Sonora repository](https://github.com/sonorahq/sonora), [workspace manifest](https://github.com/sonorahq/sonora/blob/main/Cargo.toml).

Observed manifest: `0.42.0`, GPL-3.0-or-later. It uses an application-specific GPUI fork, and pins `sonorahq/ytmusic-rs` at `f213c0b`. Its broader workspace includes several services and audio infrastructure. This establishes relevant source to study, not a reason to inherit every subsystem.

### R14 — Sonora's standalone YouTube Music crate
Sources: [Cargo.toml](https://github.com/sonorahq/ytmusic-rs/blob/f213c0b/Cargo.toml), [`src/client.rs`](https://github.com/sonorahq/ytmusic-rs/blob/f213c0b/src/client.rs), [source directory](https://github.com/sonorahq/ytmusic-rs/tree/f213c0b/src).

Observed version `0.1.0`, GPL-3.0-or-later; dependencies include Reqwest, cookie storage and rquickjs. Inspected client exposes authenticated and anonymous paths, page/account selection, optional persistence, resolution caching and an optional browser-hosted token minter. These require policy/storage changes for this specification. A later native-resolver experiment can evaluate it behind the same interface; current research did not compare its live reliability with yt-dlp.

### R15 — Public YouTube Data API scope
Source: [official API reference](https://developers.google.com/youtube/v3/docs/).

The documented resource model does not provide the required native music-stream playback SDK or a general Premium subscription check. Channel subscriptions and channel memberships must not be confused with YouTube Premium. This is a finding about public documentation reviewed, not a claim that no internal entitlement signal exists.

### R16 — Official API restrictions
Sources: [YouTube API developer policies](https://developers.google.com/youtube/terms/developer-policies), [policy guide](https://developers.google.com/youtube/terms/developer-policies-guide).

Policies restrict separating audio and using hidden/background players, among other behaviors. Consequently a hidden iframe is not an official-compliance shortcut for the requested audio-first application. These API policies and the general service terms have different scopes; neither should be paraphrased as a blanket statement about criminal legality.

### R17 — YouTube service terms
Source: [YouTube Terms of Service](https://www.youtube.com/static?template=terms).

The terms condition access/use on the service's permissions and restrict certain automated access and interference with restrictions. Personal noncommercial use is not blanket authorization for any third-party access method. The project should acknowledge service/account risk without claiming that payment makes the implementation approved.

### R18 — Audio quality
Source: [YouTube Music audio quality help](https://support.google.com/youtubemusic/answer/9076559?hl=en).

Official help describes high quality as up to 256 kbps AAC/Opus. Do not translate this into a guarantee that every requested track exposes that format, or label lossy formats as lossless. The proposed interface reports actual observed codec and format metadata.

### R19 — Premium tier distinction
Sources: [YouTube Premium benefits](https://support.google.com/youtube/answer/6305537?hl=en), [Premium Lite help](https://support.google.com/youtube/answer/15968883?hl=en).

Full YouTube Premium includes YouTube Music Premium. Premium Lite is not an equivalent entitlement for this project. Use the user's stated full Premium subscription as the product assumption; do not build billing/account-tier inference from ambiguous labels.

### R20 — yt-dlp authentication and account risk
Source: [yt-dlp YouTube extractor notes](https://github.com/yt-dlp/yt-dlp/wiki/Extractors#youtube).

Current upstream guidance uses cookies and states OAuth login no longer works for yt-dlp. It warns account use can cause temporary or permanent account bans and describes cookie rotation. This does not imply all metadata OAuth integrations are impossible; it does mean metadata OAuth must not be assumed to authenticate this resolver.

### R21 — Premium and Proof-of-Origin tokens
Source: [yt-dlp PO Token Guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide), page dated July 12, 2026 when inspected.

The guide distinguishes GVS, player and subtitle token contexts and specifically reports a Premium exemption for GVS tokens. Enforcement depends on client/session and changes. The specification tests the exemption rather than building a browser token service preemptively; it does not claim a blanket or permanent exemption.

### R22 — JavaScript challenge runtime
Source: [yt-dlp EJS guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS).

Current YouTube support uses external challenge scripts and a supported JavaScript runtime. Deno is the upstream recommendation. Installation requirements depend on the yt-dlp packaging method. Keep executable, EJS and runtime versions compatible; an official packaged executable can include EJS without eliminating the runtime dependency.

### R23 — mpv playback constraints
Source: [mpv stable manual](https://mpv.io/manual/stable/), particularly `gapless-audio`.

Keeping the output device open helps transitions but does not remove network buffering delays. Output-format changes also matter. “Enable gapless” is therefore not an acceptance test. The proposed tests separately measure local decoder/output transitions and real network playback.

### R24 — Rust libmpv wrapper
Source: [libmpv2 documentation](https://docs.rs/libmpv2/latest/libmpv2/).

Candidate binding for the media-engine adapter. Linkage, runtime library discovery, available codecs, headers and the actual mpv build must be validated on the chosen host. No specific wrapper/native-library combination has been certified by this research.

### R25 — Desktop media controls
Source: [Souvlaki documentation](https://docs.rs/souvlaki/latest/souvlaki/).

Candidate common media-control layer. Platform integration still matters: Windows needs the window handle; macOS requires event-loop integration; Linux uses media-session services. Do not introduce a second GUI event loop or assume successful compilation proves hardware media keys work.

### R26 — Credential storage
Source: [keyring documentation](https://docs.rs/keyring/latest/keyring/).

Candidate OS credential-store abstraction. Runtime store availability must be tested, including a locked or absent Linux secret service. Memory-only storage is the specified fallback, not silently writing plaintext credentials.

### R27 — GPL private modification
Source: [GNU GPL FAQ](https://www.gnu.org/licenses/gpl-faq.en.html#GPLRequireSourcePostedPublic).

The GNU FAQ confirms private modifications do not require public source release. This supports private reuse of the GPL references here; it does not erase attribution/provenance needs or decide obligations for a later distributed build.

### R28 — Google authentication UX constraints
Sources: [Google OAuth native-app documentation](https://developers.google.com/identity/protocols/oauth2/native-app), [OAuth policies](https://developers.google.com/identity/protocols/oauth2/policies).

Google disallows embedded user agents for OAuth authorization. Cookie-based access to unofficial endpoints is a separate mechanism, not a sanctioned OAuth alternative. Do not promise a seamless GPUI-embedded Google login. The first version uses user-controlled browser sign-in and an explicit local session-import workflow.

### R29 — GPUI official examples
Source: [GPUI examples](https://gpui.rs/examples/).

Use examples appropriate to the exact pinned framework revision. A GPU-rendered native interface does not automatically provide all widgets, accessible semantics, platform text-input behavior or a Tokio reactor for arbitrary futures.

### R30 — Hummingbird
Source: [Hummingbird repository](https://github.com/hummingbird-player/hummingbird).

Another native GPUI music-player reference, useful for library and desktop integration patterns. Its project claims and source are not a substitute for testing this application's authenticated playback path.

## Residual uncertainties

The primary OS, hardware and distribution environment were not specified. The build plan therefore targets the coding environment's host OS first and requires a recorded decision. Cross-platform release quality is not assumed.

The actual account's selected YouTube identity, cookie longevity, membership signals, returned media transport/codec, premium quality availability and GVS behavior remain untested. SABR-only or DRM-only results must be treated as unsupported unless a later explicit implementation is validated. No automatic fallback may silently change the product's account or access policy.

Dependency versions in observed manifests are evidence, not a lockfile for the proposed app. The coding model must produce a reproducible, tested baseline before expanding the UI. Accessibility, performance, gapless playback and native media integration all require host-level testing.
