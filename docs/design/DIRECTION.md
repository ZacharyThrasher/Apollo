# Apollo design direction: "Art-led dark"
Owner: project coordinator (taste owner). Chosen by Zach 2026-10-03. Threads implement this; unanswered taste questions go back to the coordinator.

## Principles
1. Art is the only color. Chrome is neutral graphite; the single accent comes from the current album art.
2. Quiet chrome, loud content. Artwork and titles carry the page; controls recede until hovered/focused.
3. Minimal text. Zach's rule: "Will the user ever need/want to read this more than once or twice? If not, it doesn't need to be prominently displayed." No helper text, onboarding copy or explanatory subtitles.
4. Keyboard-first, no layout shift.

## Color (dark only for v1)
Surfaces: bg.base #0E0E10 (with Mica: same at 80% alpha); bg.raised #17171A (player bar, panels); bg.overlay #1F1F23 (menus, popovers, tooltips). state.hover white 6%, state.selected white 8%, state.pressed white 10%. border.subtle white 6%, border.strong white 12%.
Text (must pass WCAG AA 4.5:1 on bg.base and bg.raised): text.primary #F2F2F3; text.secondary #A1A1A8; text.tertiary #808088 (durations, counts; never essential info); text.disabled white 30%.
Accent (dynamic): downscale artwork to 64x64; find the dominant vivid cluster (median-cut or k-means k=5), ignoring near-gray, near-black and near-white pixels; convert to OKLCH; clamp L to 0.68-0.78 and C to 0.08-0.18; keep hue. Fallback (no art or grayscale art): OKLCH(0.72 0.12 275). accent.fg (text/icons on accent) = bg.base. Accent changes crossfade over 400 ms, interpolated in OKLCH (shortest hue path). Used only for: play button fill, progress fill, active nav indicator, focus ring, selected-row marker.
Ambient: now-playing area only, artwork blurred (radius >= 80px) at 12-18% opacity; never behind dense lists.
Semantic: error #FF6B6B, warning #F5C26B, used sparingly. No success color; success is silence.

Contrast note (2026-10-03, theme crate): the original text.tertiary #7C7C84 measured 4.32:1 on bg.raised (4.66:1 on bg.base), below AA. Nudged lighter to #808088 (4.57:1 raised, 4.92:1 base). On bg.overlay text.tertiary is 4.19:1 and text.secondary passes; tertiary text must not be placed on bg.overlay (menus, popovers, tooltips) unless the owner decides otherwise.

## Typography
Segoe UI Variable (Display optical size at >=20px, Text below); fallback Segoe UI. Scale px/weight: caption 11/400, body 13/400, body-strong 13/600, list-title 15/400, section 20/600, page-title 28/600, now-playing-title 40/600. Tabular numerals for times. Single-line ellipsis truncation, full text in a hover tooltip.

## Spacing, shape, icons
4px grid: 4, 8, 12, 16, 24, 32, 48. Radius: 4 small controls, 8 artwork/cards, 12 panels/popovers, full circle for the play button. Icons: Fluent System Icons regular 20px, filled for active/selected. Hit targets >= 32px.

## Layout
Title bar: custom, Mica, 32px drag region, native-style caption buttons.
Nav rail (left): 64px icons-only collapsed / 220px expanded; Home, Search, Library; tooltips when collapsed.
Main content: scrollable; page title 28px, sections 20px.
Player bar (bottom): 80px on bg.raised. Left: 56px art + title/artist. Center: shuffle, previous, play/pause (40px accent circle), next, repeat; progress bar below with times in text.tertiary. Right: volume, queue toggle.
Queue: right panel 320px, toggled from player bar, slides in 200 ms.
Now Playing (expanded): art min(50vh, 480px), ambient glow, 40px title, artist in text.secondary, transport below.

## Motion
Hover/press 120 ms, panels 200 ms, accent crossfade 400 ms; ease-out cubic. If Windows "Animation effects" is off, transitions are instant.

## States and copy
Loading: skeleton blocks at white 4% with subtle shimmer; spinners only inside buttons; reserve final size so nothing shifts.
Empty: one short line + one action (e.g. "No liked songs yet" [Search]).
Errors: what happened in <= 6 words + one action (e.g. "Couldn't load this playlist" [Retry]).
Premium/account status only in Settings > Account ("Premium assumed"), never in main chrome.
Focus ring: 2px accent, 2px offset, keyboard focus only.

## Out of scope for v1
Light theme, user themes, visualizers, animated backgrounds beyond the ambient glow.
