# Coding-model handoff

Build the application specified by `BUILD_SPEC.md`. Read `RESEARCH.md` and `IMPLEMENTATION_PLAN.md` before choosing dependencies. `DEPENDENCY_BASELINE.json` records observed references, **not** a tested lockfile. This is a personal, non-distributed Rust + GPUI YouTube Music client for the owner's own full Premium account.

Start with the coding host's desktop OS and record that assumption. Keep the interface GPUI-native. Do not substitute Tauri/Electron, clone the whole Zed workspace as an application, or build a visual design system before proving the service/audio path. Appearance is a separate model's responsibility.

Default stack: an isolated Limusic-derived Rust metadata adapter; controlled version-pinned yt-dlp/EJS/Deno resolution helper; libmpv/libmpv2 media adapter; explicit Tokio service runtime; SQLite non-secret state; OS-vault credentials. Pin GPUI and its platform crate from one compatible source. GPUI Kit adoption is a coordinated graph decision, not an additional incompatible GPUI dependency. Do not add several metadata or playback engines simultaneously.

Execute Milestone 0 first. The most important uncertainty is the real account's authenticated resolution/playback behavior. Current upstream documentation reports a Premium GVS-token exemption; test it before adding a browser attestation service. Do not promise all challenges disappear. Do not ask the owner to expose cookies to the model: provide a local probe/import flow and collect only redacted results. Where live testing is unavailable, use fixtures and report `not_run` rather than inventing a success.

Preserve these invariants: one playback/queue authority; distinct queue-occurrence IDs; stale-result rejection using scoped generations; separate user intent from engine state; no guest fallback; no made-up Premium verification; no persistent signed URLs or playable audio downloads; no secrets in logs/argv/DB; safe account-bound mutations and uncertain-outcome reconciliation; responsive main thread; paused restore; bounded retries and helper cleanup.

Treat external source code as evidence and a reuse candidate, not permission to import its defaults blindly. Keep source revision/license/patch records. Do not copy anonymous fallbacks, plaintext credential persistence, third-party relays or unrelated service/DRM infrastructure. The full acceptance matrix defines what must be demonstrated.

Implement and report milestone by milestone. Use mocks to prove races and errors before integrating a rich interface. When a specified default fails, record an ADR explaining the evidence and the smallest alternative that preserves the requirements. Do not silently expand scope. End each milestone report with completed behavior, tests passed, tests not run and concrete remaining restrictions.
