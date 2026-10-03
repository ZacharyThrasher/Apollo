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
Accent extraction thresholds (coordinator-approved 2026-10-03): near-gray = OKLCH C < 0.04; near-black = L < 0.18; near-white = L > 0.92. "Dominant" = the most populous vivid cluster (no saturation weighting). If fewer than 1% of the opaque pixels are vivid, use the fallback.
Ambient: now-playing area only, artwork blurred (radius >= 80px) at 12-18% opacity; never behind dense lists.
Semantic: error #FF6B6B, warning #F5C26B, used sparingly. No success color; success is silence.

Contrast notes (2026-10-03, theme crate; coordinator-approved):
- The original text.tertiary #7C7C84 measured 4.32:1 on bg.raised, below AA. Nudged lighter to #808088 (4.57:1 on bg.raised, 4.92:1 on bg.base).
- text.tertiary is never used on bg.overlay (4.19:1, fails AA). Menus, popovers and tooltips use text.secondary as their lowest text tier.
- Mica: worst plausible dark-mode backdrop assumed to be #3A3A3A. bg.base at 80% alpha over it is about #171718; primary/secondary/tertiary measure 16.0 / 7.0 / 4.57:1, so 80% already passes and stays. Final Mica alpha: 80% (`MICA_BASE_ALPHA_PCT`). If a future token change drops a tier below AA there, raise alpha in 5% steps.

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
Hover fades in and out over 120 ms; the press tint is instant (no fade). Panels 200 ms, accent crossfade 400 ms; ease-out cubic. If Windows "Animation effects" is off, transitions are instant.

## States and copy
Loading: skeleton blocks at white 4% with subtle shimmer; spinners only inside buttons; reserve final size so nothing shifts.
Empty: one short line + one action (e.g. "No liked songs yet" [Search]).
Errors: what happened in <= 6 words + one action (e.g. "Couldn't load this playlist" [Retry]).
Premium/account status only in Settings > Account ("Premium assumed"), never in main chrome.
Focus ring: 2px accent, 2px offset, keyboard focus only.

## Shell decisions (coordinator, 2026-10-03)
Applied to the app shell from the first Windows review.
1. **Close button:** hover is native Windows 11 red #C42B1C with a white glyph; pressed is #C42B1C at 90%. The other caption buttons keep the neutral hover and pressed tints.
2. **Player bar side blocks:** the left block (art, title, artist) is 30% of the bar, min 240px, max 360px. The right block (volume, queue) has the same width, so the transport stays centred on the window. The minimum window width follows: 240 + 320 + 240 plus padding and gaps is 864px.
3. **No-art tile:** bg.overlay #1F1F23 with a centred 20px Fluent music-note glyph in text.tertiary. The glyph is decorative, not text, so the "no text.tertiary on bg.overlay" rule does not apply (4.19:1 clears the 3:1 non-text minimum).
4. **Top-edge speckles:** the app draws no 1px border at the window top; the Mica edge is native. Nothing to remove; if the speckles persist they come from DWM compositing.
5. **Tooltip radius** 4px. **Volume default** 70, saved to the `settings` table of the app store (`%LOCALAPPDATA%\Apollo\app\apollo.db`, key `volume_percent`) when a slider change is final, restored on start.
6. **Queue button** toggles only its own state until the queue panel exists.
7. **Player bar errors:** the time row (times and slider) is replaced by a warning icon in `warning`, a line of at most 6 words in text.secondary, and "Retry" as a text button in text.primary with underline on hover. No accent. Retry takes the progress slider's place in the tab order.
8. **Nav:** 3px accent bar, 8% selected background and a filled icon on the selected item. Selected label is body-strong in text.primary; unselected is body in text.secondary. The expand/collapse toggle is icon-only with a tooltip and no visible label.
9. **Sliders:** 4px track. Progress fill is the accent; volume fill is text.secondary. The 12px text.primary thumb shows only on hover, drag or focus. Shuffle and repeat "on" use the filled icon in text.primary; the repeat-one icon carries its own "1" badge.
10. **Page padding:** 32px horizontal, 24px top.
- Next/Previous while paused starts playing (matches YouTube Music). Owner can override.

## Account screen decisions (coordinator, 2026-10-03)
Settings > Account. Other screens reuse these.
1. **Buttons:** secondary actions (Use Firefox, Import file, Paste cookies, Retry, Sign out) are text buttons: body in text.secondary (body-strong in text.primary for Retry-style inline actions), underline on hover, keyboard-only accent focus ring, no fill. The single primary action on a view (Sign in) is the neutral filled button: bg.overlay fill, text.primary label, no accent, fill brightens by the hover tint (120 ms) and the press tint (instant), standard focus ring, 32px high, 16px side padding. At most one primary button per view. Spinners show only inside a button. Implemented as `ui/text_button.rs` (`TextButton`, `.primary()`).
2. **Status dot:** a 6px dot in `warning`, top-right of a nav icon, marks an item that needs the owner (Settings: "Session expired" or "Choose account"). It carries an accessible name; the nav item's label becomes "Settings, <reason>".
3. **Avatars:** the account's channel picture on the 8px-radius tile when the service returns one (Google-hosted https images only); otherwise the no-art tile with the account's initial. A picture that fails to load falls back to the initial silently.
4. **Skeleton:** white 4% blocks at final size. The pulse is low amplitude (opacity 0.85 to 1.0, 1.2 s cycle) and static when the OS reduces motion.
5. **Premium:** the Premium slot shows "Premium" only once the app's own account check has confirmed it, and nothing otherwise. No hedged text ("assumed", "not declared"). Today no check can confirm it (there is no entitlement endpoint), so the slot is empty.

## Queue and Now Playing decisions (coordinator, 2026-10-03)
Applied to the queue panel (`ui/queue_panel.rs`) and Now Playing (`ui/now_playing.rs`).
1. **Panel chrome:** the queue panel is flush to the right edge, filled with bg.raised, with a `border.subtle` left border and no radius or inset. It is 320px and slides in over 200 ms.
2. **Rows:** 56px rows holding a 40px art tile, body title over body artist in text.secondary, and the duration in caption text.tertiary. On hover the duration is replaced by a remove button; the playing row has no remove. The playing row uses the nav's selected-row marker (3px accent bar, 8% selected background, body-strong title). Played rows drop the title to text.secondary. Row content is inset 4px so the focus ring fits between rows.
3. **Labels and copy:** panel title "Queue" in the section style; "History" and "Up next" labels in body-strong text.secondary; empty state "Nothing up next" with a "Search" text button.
4. **Reorder:** drag an up-next row onto another to take its place. The insertion line is 2px tall with rounded ends, in text.secondary (accent stays reserved), spanning the row's content width. Keyboard: Alt+Up/Down on the list cursor.
5. **Clear:** clears up next only, with no confirmation; the playing track keeps playing.
6. **No-art glyph:** 20px on tiles up to 56px; a quarter of the tile size above that.
7. **Now Playing:** opened from the player bar's art/title block, closed with Esc, the chevron, or navigating. Art `min(50vh, 480px)`, title in the 40px style, then one line of artist name(s) and album separated by " · " at 18px regular in text.secondary, 8px below the title, truncating with an ellipsis. The progress row and transport sit below, shared with the player bar. Ambient art is blurred at 15% opacity (inside the 12-18% range) and sits only behind this view.

## Library decisions (coordinator, 2026-10-03)
Applied to the Library page (`ui/library.rs`).
1. **Tabs:** Playlists and Albums use the nav's selected-tint pattern (selected item on the tint, text.primary; others text.secondary). No accent underline; accent stays reserved.
2. **Loading:** skeleton rows at final size use the shared pulse (`ui/skeleton.rs`: opacity 0.85 to 1.0, 1.2 s, static under reduced motion). No separate shimmer on any screen.
3. **Liked Music:** a filled heart in text.secondary on the standard no-art tile, no accent. Everything else without a cover uses the music-note tile.
4. **Playing row:** the nav's selected-row marker (3px accent bar, selected background, body-strong title), the same as the queue panel.
5. **Icons:** Fluent System Icons only, 20px grid, taken from the Fluent set (regular for outlines, filled for the heart). Do not hand-draw icons.

## Album, artist and playlist decisions (coordinator, 2026-10-03)
Applied to the pages opened from Search cards and artist pages (`ui/library.rs`, `ui/card.rs`).
1. **Pages:** an album or playlist page is the Library's detail view. An artist page has the same header with Play and Shuffle acting on its top songs, a "Top songs" list, then a section per row of albums, singles and playlists. Pages stack; Back (Esc, Alt+Left, the back button) closes one at a time, and closing the last one returns to the route it was opened from. The nav keeps that route lit.
2. **Cards:** 144px artwork inside an 8px inset (the hover tint and focus ring sit in the inset), title in body-strong, second line in body text.secondary. Cards wrap in a grid with 16px between cards, and the artwork lines up with the section heading above it.
3. **Shape:** artist artwork is a circle, on Search's artist cards and on the artist page header. It is the one cue that tells an artist from an album at a glance. Albums, singles and playlists stay 8px-radius squares.
4. **Header:** 96px artwork, page-title name, then the card's own second line and a "N songs" line in text.tertiary once every page has loaded. The count is dropped when the second line already says songs or tracks, and its line is held open while loading so nothing shifts. It never sits on bg.overlay.
5. **States:** the shared skeleton while loading; an error is "Couldn't load this artist" (or album, playlist) with Retry; an artist page with nothing on it says "Nothing to show" with Back.
6. **Keyboard:** Search cards and artist-page cards are tab stops; Enter opens the focused card (its own key context, so a list's Enter does not play the selected row instead). Space stays play/pause.

## Like and playlist decisions (coordinator, 2026-10-03)
Applied to the heart and add-to-playlist actions (`ui/row_actions.rs`, `ui/playlist_picker.rs`, `ui/toast.rs`, `ui/queue_panel.rs`).
1. **Heart:** filled Fluent heart in text.primary when liked, outline in text.secondary when not (the shuffle and repeat on/off pair). It sits leftmost in the player bar's right-hand block, and on Now Playing's title line with add-to-playlist beside it. The bar has no add-to-playlist (width).
2. **Rows:** track rows in the Library, Search and the queue panel reserve a 32px slot for the heart so nothing shifts. It shows on hover or under the keyboard cursor and stays once the track is liked. Library and Search rows add an add-to-playlist action beside it; queue rows have the heart only. Keys on a list: `l` likes the selected row, `p` adds it to a playlist (not in the queue).
3. **Playlist picker:** an anchored popover, not a modal. It opens under the button that opened it with the right edges lined up (the `p` key anchors to the selected row's action), above it when there is no room below, and centred only with no anchor. 280px wide, 320px at most with scroll, bg.overlay, 8px radius, border.subtle, 40px rows without art, no scrim. Esc or a click outside closes it.
4. **Rows in the picker:** a playlist that already holds the track shows a check in text.secondary and does nothing when chosen. Adding a duplicate is skipped silently. No playlists: "No playlists yet" with no action. Load failure: "Couldn't load playlists" with Retry.
5. **Errors:** a failed like or add rolls back and leaves a toast above the player bar: warning icon and up to six words on bg.overlay in text.secondary. It stays 4 s or until clicked, and fades in and out over 150 ms (appears and goes at once under reduced motion).

## Home decisions (coordinator, 2026-10-03)
Applied to the Home route (`ui/home_page.rs`, `home.rs`); the feed is YouTube Music's `FEmusic_home` for the signed-in account.
1. **Page:** the "Home" page title, nothing else: no greeting, no explanatory text. Each shelf is a section titled in the same style as the other section headers.
2. **Card shelves:** one horizontal row of the shared cards, 16px apart (artists circles, albums and playlists rounded squares). Clicking a card opens its album, artist or playlist page, and Back returns to Home.
3. **Song shelves** (such as "Quick picks"): a grid of compact song rows (art, title, artist), four rows tall, paging sideways by column; no cards. Clicking a song plays it with that shelf as the queue. A shelf that mixes songs and cards is a card shelf; its songs queue together.
4. **Scrolling:** a row scrolls sideways with the wheel or a swipe. Previous and next chevrons (32px circles on bg.overlay, border.strong) appear on hover at the row edges, only when there is somewhere to go, and page by a screenful, 200 ms ease-out (at once under reduced motion). The chevrons are not tab stops; Left and Right move between items and the row follows focus.
5. **Loading:** the shared skeleton pulse (a song grid, then a card row). More shelves load by continuation as the page runs out, and a failed continuation leaves a "Couldn't load more" line with Retry at the foot. Errors are the Library's error row ("Couldn't load Home", or "Sign-in expired") with Retry; an empty feed says "Nothing to show" with Retry; signed out offers a primary Sign in button.
6. **Freshness:** shelves are kept in memory for the session and reload in the background when Home is reopened after 10 minutes. A failed refresh keeps what is shown. A different account drops them.
7. **Accessibility:** each shelf is a list named by its title; every item is a stop at the page's tab index.

## Out of scope for v1
Light theme, user themes, visualizers, animated backgrounds beyond the ambient glow.
