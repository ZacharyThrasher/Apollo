# Personal YouTube Music player — research and build handoff

**Rust + GPUI · own Premium account · no distribution**  
Prepared 2026-10-02.

Start with **[BUILD_SPEC.md](BUILD_SPEC.md)** for architecture and requirements. Give **[CODING_MODEL_HANDOFF.md](CODING_MODEL_HANDOFF.md)** to the coding model together with the other files. **[IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md)** defines staged work and 36 acceptance scenarios. **[RESEARCH.md](RESEARCH.md)** contains reference-project audits, 30 primary-source entries and uncertainties. **[DEPENDENCY_BASELINE.json](DEPENDENCY_BASELINE.json)** records observed revisions and candidate choices without claiming they were compiled together.

## Recommendation

Use a GPUI-native Rust application with a small isolated metadata adapter, a replaceable yt-dlp resolution helper and libmpv playback. Keep credentials and service-specific parsing outside the interface. Start with one account, one queue and one playback engine. Require authenticated operation and a declared Premium account, but do not pretend a public Premium-verification API exists.

The major initial experiment is whether the actual Premium account can reliably resolve and play its available music without a browser PO-token helper. Current upstream documentation suggests that narrower simplification; live validation is still necessary.

## What this bundle is—and is not

This is a source-backed design and implementation handoff, not an application, style design or tested dependency lockfile. Selected repository files and official/upstream documentation were inspected. No user credentials were provided, no account operations were performed, and no GPUI/native-media build was run. The supplied Tono Codeberg source could not be retrieved; its implementation and license are not asserted.

Performance budgets and the test matrix are targets for the coding model. Personal use and Premium do not remove service/account risks. The default plan excludes offline music downloads, public redistribution and anonymous playback.
