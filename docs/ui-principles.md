# UI principles

Owner-set rules for all interface work.

## Minimal copy

Before adding any text, ask: **will the user ever need or want to read this more than once or twice?**
If not, it does not get prominent placement. That means no explanatory subtitles, no
"how this works" blurbs, no persistent helper text. One-time explanations belong in
a first-run moment, a tooltip, or a quiet secondary line, and should disappear once used.

Applies to: empty states, settings descriptions, error messages (state what happened and
the one action, nothing more), onboarding, and diagnostics (details behind a disclosure).

The functional spec (BUILD_SPEC section 14) still requires honest loading / empty / partial /
error / disconnected states and accessible names. Meeting it takes labels and state, not paragraphs.
