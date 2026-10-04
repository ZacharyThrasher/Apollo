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
Nav rail (left): 64px icons-only collapsed / 220px expanded; Home, Explore, Search, Library; tooltips when collapsed.
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
8. **Autoplay switch:** right-aligned in the queue header, after shuffle, repeat and clear: a compact 28x16 switch (accent track when on, `border.strong` when off, 12px knob) with the label "Autoplay" in body text.secondary. On by default and remembered. When up next is down to one track (or none) while something plays, a song radio seeded by the last track is appended; repeat-all takes priority. Radio rows look like any other queue row.

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
4. **Scrolling:** a row scrolls sideways with the wheel or a swipe. Previous and next chevrons (32px circles on bg.overlay, border.strong) appear on hover at the row edges, only when there is somewhere to go, and page by a screenful, 200 ms ease-out (at once under reduced motion). The chevrons are not tab stops. Each shelf is a single Tab stop (roving focus): Tab goes shelf to shelf and returns to the item you left; Left and Right move between items inside the shelf and the row follows focus.
5. **Loading:** the shared skeleton pulse (a song grid, then a card row). More shelves load by continuation as the page runs out, and a failed continuation leaves a "Couldn't load more" line with Retry at the foot. Errors are the Library's error row ("Couldn't load Home", or "Sign-in expired") with Retry; an empty feed says "Nothing to show" with Retry; signed out offers a primary Sign in button.
6. **Freshness:** shelves are kept in memory for the session and reload in the background when Home is reopened after 10 minutes. A failed refresh keeps what is shown. A different account drops them. The first batch is also saved on disk (see Instant pages), so launch draws the last feed at once.
7. **Keyboard:** one Tab stop per shelf at the page's tab index, so Tab reaches the player bar quickly. Each shelf is still a list named by its title. Nothing more is planned for accessibility on this personal app (Zach, 2026-10-03).
8. **Approved as built:** the primary Sign in button when signed out, "Nothing to show" with Retry, "Couldn't load more" with Retry, and the 32px circle chevrons on bg.overlay with border.strong.

## Track menu decisions (coordinator brief, 2026-10-03)
Applied to the menu opened by right-click or a row's More button (`ui/track_menu.rs`).
1. **Where:** right-click on any song row or tile (Library and playlist/album/artist lists, Search, Home, the queue panel) and on album and playlist cards. A hovered row also shows a "More" button (Fluent more-horizontal) at its right end, in a 32px slot that is always reserved. Artist cards have no menu.
2. **Surface:** the add-to-playlist popover's: bg.overlay, 8px radius, border.subtle, no scrim. Rows are 32px with a 16px Fluent icon in text.secondary and a body label in text.primary; hover and the keyboard cursor use the list hover tint; groups are split by 1px border.subtle lines. Built choice, not in the brief: the menu is 220px wide with 4px inside padding.
3. **Placement:** at the pointer for a right-click, under the More button with the right edges lined up for the button. It flips to the other side of the pointer or button when it would leave the window and never covers the title bar. Esc or a click outside closes it; Up, Down, Home, End and Enter work it.
4. **Song items, in groups:** Play next, Add to queue; Start radio, Add to playlist…; Like / Remove from Liked; Go to artist, Go to album (each hidden when unknown); Copy link; and in the queue panel Remove from queue (not on the playing row).
5. **Card items:** Play, Shuffle; Add to queue, Start radio; Copy link. Play, Shuffle and Add to queue follow every page of the album or playlist (up to 30 pages); Start radio is seeded from its first song.
5a. **Queue rows:** no permanent More slot. On hover or under the keyboard cursor, More and Remove appear over the duration (which hides), on an opaque bg.raised patch, so the title keeps its full width at rest.
6. **Start radio:** the song plays at once and, when the radio arrives, it replaces everything that was up next. If the radio cannot be fetched the queue is left as it was.
7. **Feedback:** success is silent (no "Added to queue" or "Link copied" toast exists; the existing toast is error-only). Failures use the existing error toast: "Couldn't start radio", "Couldn't load tracks".

## Lyrics decisions (coordinator, 2026-10-03)
Applied to the lyrics pane (`ui/lyrics_pane.rs`, `lyrics.rs`) and the Lyrics button in the player bar. Tokens are in `theme::tokens::lyrics`.
1. **Where:** Now Playing has a "Lyrics" toggle (Fluent icon, filled when on) beside the queue toggle in the player bar. When it is on, lyrics take the right half of Now Playing in place of the queue; the cover, title and transport stay on the left. The choice is remembered (`lyrics_open` in the `settings` table). Pressing the Lyrics button with Now Playing closed opens it. The queue button, pressed while lyrics show, turns them off and opens the queue.
2. **Lines:** Segoe UI Variable 22px semibold, left-aligned, line height 1.3 with 8px above and below each line. The current line is text.primary; every other line is text.secondary at 80% opacity (measured: 5.2:1 on bg.base, so it still passes AA). Never text.tertiary. A line you point at rises to full text.secondary.
3. **Following:** the current line sits a third of the way down the pane. The column moves to it over 200 ms (ease-out), or at once under reduced motion. Clicking a line seeks to it and follows again. A manual scroll pauses following until a line is clicked or 4 s after the last scroll.
4. **Plain lyrics:** static, scrollable text in the same type, all text.secondary; no clicking, no following.
5. **States:** no lyrics is a single centred "No lyrics" in text.secondary (same type as the lines); loading is the shared skeleton pulse (six bars the height of a line, starting at the third). Nothing else is explained.
6. **Data:** LRCLIB only (`/api/get`, then `/api/search` within 2 s of the track's length), synced preferred, plain as the fallback. Looked up when a track becomes current and for the track after it; answers, including "not found", are cached in SQLite by video id (a "not found" is believed for a week).

## Audio toolbox decisions (coordinator brief, 2026-10-04)
Applied to Settings > Audio (`ui/audio_page.rs`, `ui/equalizer_panel.rs`, `ui/choice_menu.rs`, `ui/vslider.rs`) and the speed button in Now Playing. Behaviour is in `docs/audio-toolbox.md`.
1. **Audio section:** directly after Playback (Search follows it). "Output device" is a select row (`SettingsRow::select`): label left, current device and a chevron right. It opens a `choice_menu` popover in the track menu's style (bg.overlay, 8px radius, border.subtle, 32px rows, check on the one in force): "System default", then the devices.
2. **Equalizer row:** a switch (off by default) with an "Edit" text button beside it. Edit opens the editor under the card and reads "Done" while it is open.
3. **Editor:** bg.raised 8px panel. Preset chips (Flat, Bass boost, Vocal, Treble boost) and Reset on top; ten vertical sliders below, +-12 dB, 31 Hz to 16 kHz, each frequency under its slider in text.tertiary. Changes are heard while you drag, with no clicks.
4. **Speed:** a "1.0×" text button in Now Playing's title-line actions. The popover offers 0.75, 1.0, 1.25, 1.5. It resets to 1.0 on each launch and is not saved.
5. **Built choices, not in the brief:** editing a flat, switched-off equalizer turns it on (a slider that changes nothing audible reads as broken); a band's dB value shows above its slider only while pointed at, dragged or focused (the row of sliders stays quiet); a 0 dB hairline sits behind each slider; the device popover is 320px wide and the speed popover 120px; the Edit button turns into "Done" rather than a separate close control; the unplugged device reads "System default" because that is what plays.

## Instant pages (invisible speed work, 2026-10-04)
No visible change: no "refreshing" indicator, no Settings row. A page that was opened before draws from the cache at once and is fetched again behind it; the fresh answer replaces it in place only if it differs (no scroll jump, no flicker). A page opened for the first time keeps the shared skeleton pulse.
1. **What is cached:** Home's first batch, the Library lists (playlists and albums, Artists, History), and every album, artist and playlist page's whole track list, in the `page_cache` table (`crates/app/src/page_cache.rs`), also held in memory for the session. Home is drawn at launch as soon as the account is known, before the session is confirmed.
2. **Account:** pages are filed under the chosen channel. Another account purges the old one's pages; signing out by choice purges everything.
3. **Writes:** a like drops Liked Music's page, an add to a playlist drops that playlist's page and the Library list, and Subscribe drops the artist's page and the Artists list, at the moment of the write and again when it settles. A fetch that began before a drop cannot put its older answer back. Playlist editing calls `PageCache::invalidate_collection(id)` (browse id or bare `PL...` id); `invalidate_library()` after a playlist or album is created, deleted or saved.
4. **Failures:** a failed refresh behind cached content is silent, except a dead session ("Sign-in expired"), which always replaces the cached page.

## Artist page decisions (coordinator brief, 2026-10-04)
Applied to the artist page (`ui/library/artist.rs`); it replaces the artist-page parts of "Album, artist and playlist decisions" 1, 3 and 4.
1. **Header:** the artist's picture as a wide banner, the service's own crop, centre-cropped to its frame: the page's width by 280px, 8px radius, scrolling with the page. It fades to bg.base (opaque, so the picture never shows through the Mica alpha) over its lower two thirds, with the name over it at 32px semibold (`typography::ARTIST_NAME`), 24px from the left and bottom. No blur: the ambient blur stays in Now Playing. While the page loads the banner is the skeleton color and the name is the one the card carried.
2. **Button row,** 16px under the banner, 8px apart, all 32px high: Play (filled with the accent, label in accent.fg), Shuffle and Radio (outlined, `border.strong`, label in text.primary) and Subscribe, an outlined toggle that reads "Subscribed" with the selected tint when on. Subscribe keeps one width for both labels. Shuffle plays the artist's own shuffle mix (the header's Shuffle button) and falls back to the top songs; Radio plays the artist's radio from its first track; Play plays the top songs. Radio and Shuffle show the in-button spinner while their tracks load. A failed radio raises "Couldn't start radio"; a failed Subscribe is put back with "Couldn't subscribe" or "Couldn't unsubscribe" (the usual error toast).
3. **Sections, in order:** "Top songs" (the page's rows, with a "Show all" text button at the right of the heading when the service has a page of every top song; it opens that playlist over the artist), then a sideways shelf of cards for each of the service's album, single and playlist rows under its own title, then "Fans also like" as a shelf of round artist cards (name only), then "About". Shelves scroll with the wheel or a swipe; every card is a tab stop and keyboard focus scrolls its shelf. No music-video sections.
4. **About:** the bio in body text.primary, clamped to 3 lines and 720px wide, opened and closed by clicking it (Enter when focused); under it one line in text.secondary with the subscriber count and, when the service gives one, the monthly listeners, joined by " · ".
5. **Subscribe is a write to the account:** the button flips at once, the request is sent once, and an unknown outcome (a timeout, a dropped connection, an unreadable reply) is settled by reading the artist page back, never by sending it again. Presses are ignored while one is out, and an answer for a page closed since (an account change closes them all) is dropped.

## Search decisions (coordinator brief, 2026-10-04)
Applied to the popover under the Search field (`ui/search_popover.rs`) and `search.rs`.
1. **Popover:** one anchored popover hangs under the field, as wide as the field, styled like the track menu (bg.overlay, 8px radius, border.subtle, 4px padding, 32px rows with a 16px Fluent search icon in text.secondary). Up to 8 rows. Up, Down, Enter and Esc work it; a click picks a row. No scrim, no helper text.
2. **Recents:** with the field focused and empty it lists up to 10 recent searches. Each has an x (Fluent dismiss) that removes it, and the header has a small "Clear" on the right. Recents are saved when a search is made on purpose (Enter, a picked row, or playing or opening a result), newest first, one copy each, 20 kept. A pasted link is never a recent.
3. **Suggestions:** while typing, the service's completions, asked for after a 150 ms pause (not written to history, so no 2-character minimum), cached per query. The typed text itself and repeats are left out. A failure shows nothing.
4. **Closing:** Esc closes the popover first, then clears the field, then leaves it. Enter, a pick, and a click outside close it; typing or refocusing an empty field brings it back. Down goes to the results once it is closed.
5. **Pasted links:** a music.youtube.com, youtube.com or youtu.be link acts on Enter with no confirmation: a track plays now, a playlist, album or artist opens its page. A link is never searched for (nothing is typed into history). A link that is not one of those says "Can't open that link" with a Clear action.
6. **Hide music videos:** removes the video versions from the Songs group (albums, artists and playlists are untouched). It applies to the answer already on screen, with no new request.
7. **Settings > Search** (after Playback): "Save recent searches" (on) and "Hide music videos" (off). Turning the first off forgets the list. Signing out forgets it too.

## Search finishing decisions (coordinator brief, 2026-10-04)
Applied to the Search page (`ui/search_page.rs`, `ui/search_chips.rs`, `ui/top_result.rs`) and `search.rs`.
1. **Chips:** one row of pills under the field: All, Songs, Albums, Artists, Playlists, Community playlists. Pills are bg.raised, 32px high, body in text.secondary; the active one is filled bg.overlay with text.primary and a 1px accent outline (every pill reserves the outline, so nothing shifts). The row is one tab stop (the active chip); Left and Right walk it and Enter picks (Space stays play/pause). The row shows once there is a query and is gone for a pasted link. The chip survives a new query.
2. **All:** the first row holds the Top result on the left (a 340px bg.raised 8px card) and the top 4 songs on the right under a "Songs" heading; below come Albums, Artists and Playlists as the card grids they were. Each group's heading has a "Show all" text button on its right that switches to that chip. A Top result that is an album, artist or playlist is not listed again in its group. With no Top result the songs fill the row.
3. **Top result card:** art 96px (a circle for an artist), the name at 22px semibold, then "Song / Video / Album / Artist / Playlist \u2022 byline" in text.secondary, then a Play button (accent fill, accent.fg label, 32px pill). The art and text are one control (a song plays, anything else opens its page); Play plays a song, plays an album or playlist through, and plays the listed songs for an artist. Right-click opens the same menu as the cards.
4. **Filtered lists:** Songs is the song rows, the other chips the card grid, with no heading. They page by scrolling: the next page is asked for within 600px of the end (or when the list does not fill the window), under it the shared skeleton pulse (3 rows, or one row of cards); a failed page is "Couldn't load more" with Retry. A page that adds nothing new ends the list.
5. **No Settings rows** were added.

## Explore decisions (coordinator brief, 2026-10-04)
Applied to the Explore route (`ui/explore_page.rs`, `explore.rs`, provider `explore.rs`).
1. **Nav:** "Explore" sits right after Home with the Fluent compass icon (filled when selected). Choosing it while on it closes an open mood page.
2. **Page:** the "Explore" page title, then sections in this order, each hidden when the service sends nothing for it: "New releases", "Top songs", the other shelves (such as "Trending"), "Moods & genres". No subtitles, no country picker; the country is the account's own.
3. **New releases and other shelves:** one horizontal row of the shared cards, 16px apart, with Home's hover chevrons; albums and singles open their page, and Back returns to Explore. One Tab stop per row; Left and Right walk it.
4. **Top songs:** a numbered list of the compact song rows Home uses, 560px wide, rank in text.tertiary (tabular numerals) in a 32px column, twenty rows. "Show all" is a text button under the list: it opens the chart's own playlist page when the service names one, else it opens the rest of the list in place. One Tab stop; Up and Down walk it. Clicking a row plays it with the whole chart as the queue.
5. **Moods & genres:** one wrapping grid of 32px-high pills on bg.raised (full radius, 16px side padding, body label in text.primary, 12px apart, the hover tint fading over 120 ms). One Tab stop; Left and Right walk it. A pill opens a page titled with the mood: Back button, the mood in the page-title style, then the playlists as the shared cards in a wrapping grid. Esc and Alt+Left go back to the pills, scrolled to where they were. Opening a playlist from there and pressing Back returns to the mood page.
6. **States:** the shared skeleton while loading, "Couldn't load Explore" or "Couldn't load playlists" with Retry, "Nothing to show" with Retry (or Back on a mood page); signed out offers the primary Sign in button. The page loads when first opened (not at startup), is kept for the session and refreshes in the background after 10 minutes. A different account drops it.
7. **What the service sends varies:** Explore is built from four answers (explore, charts, new albums and singles, moods) and shows whatever came back. Podcasts and music-video shelves are left out (Apollo plays audio). No new Settings row.

## Related tab decisions (coordinator brief, 2026-10-04)
Applied to Now Playing's right pane (`ui/pane_tabs.rs`, `ui/related_pane.rs`, `related.rs`).
1. **Pane control:** a 48px strip at the top of the right pane with three text segments, Up next · Lyrics · Related. The active one is body-strong in text.primary with a 2px accent underline; the others are body in text.secondary with the list hover tint. It sits over the queue, the lyrics and the related songs alike, and only while Now Playing is open (with it closed the queue keeps its "Queue" title, since there is no pane to switch). Picking Lyrics or Related opens Now Playing, like the player bar's Lyrics button. The segments are three Tab stops at index 12.
2. **In sync with the bar:** the control, the player bar's Lyrics and Queue buttons and Now Playing's right side are one state. Related and Lyrics are exclusive, and either one replaces the queue (which stays open underneath, as lyrics already did). The Queue button, pressed over either, turns it off and shows the queue; the Lyrics button, pressed over Related, swaps to Lyrics. Related has no bar button.
3. **Remembered:** the chosen segment is saved (`related_open` beside `lyrics_open`). No Settings row.
4. **Content:** a shelf of songs is rows like the queue's (56px, 40px art, title over artists, the duration in caption text.tertiary swapped for More under the pointer, right-click opens the track menu; a click plays it with the shelf as the queue). Every other shelf is a heading in body-strong text.secondary over a sideways-scrolling row of compact cards (96px art in the cards' 8px inset, artists as circles; a click opens the album, artist or playlist page and leaves Now Playing). Related is a dense list, so the pane sits on bg.raised with the `border.subtle` left border like the queue panel: no ambient art behind it.
5. **Loading:** fetched only while the Related segment is on screen, then again for every new playing track. The previous shelves stay until the new answer arrives (no skeleton flash between tracks); the first load is the shared skeleton rows. A failure is one line ("Couldn't load related", "Sign-in expired", ...) with Retry; a track with nothing related says "Nothing related".

## Stats and Home personalization decisions (coordinator brief, 2026-10-04)
Applied to the Library's Stats tab (`ui/library/stats.rs`, `library/model/stats.rs`), Home (`home.rs`) and Settings. Everything is computed on this machine from Apollo's own play log (`docs/local-stats.md`); no new network calls.
1. **Home shelves:** "On repeat" (most played in the last 30 days) and "Forgotten favourites" (played a lot, not for 60+ days) are song shelves in Home's own shelf component, right after the first YouTube Music shelf, in that order. A shelf stays hidden until it has 6 tracks (20 at most).
2. **Stats tab:** the last Library tab. At the right end of the tab row a period text button opens the sort popover's twin: Last 4 weeks (default), Last 6 months, All time. Under the tabs, a row of three bg.raised 8px cards (Listening time, Tracks played, Top artist: caption label over a 20px semibold value), then Top tracks, Top artists and Top albums, ten rows each under body-strong text.secondary headings. Rows are the usual 56px rows with the rank in text.secondary where the track number sits; the plays count ("12 plays") sits where a track's duration does, in caption text.tertiary. A track plays the Top tracks list from it; an artist or an album opens its page (an artist or album the log has no id for does nothing and has no menu). An artist's picture is the cover of their most played track, in a circle.
3. **Empty:** no plays in the period shows one line, "Play some music and your stats show up here". No action.
4. **Settings:** a "Home" section right after Playback with one row, "Personal shelves" (switch, on). Off hides the two shelves; the log keeps recording either way.
5. **Not covered by the brief, chosen by the thread:** the plays count on the right of each row; "On repeat" counts a track only after 2 plays in the 30 days (once is not a repeat); "Forgotten favourites" needs 5 plays in all.

## Out of scope for v1
Light theme, user themes, visualizers, animated backgrounds beyond the ambient glow.
