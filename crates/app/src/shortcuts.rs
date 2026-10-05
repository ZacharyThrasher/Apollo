//! Shortcut help: what keys are bound, shown where the eye already is.
//!
//! The keys themselves live in the keymap (`keymap.rs` and each page's `bind_keys`). This module
//! only *reads* them: [`ENTRIES`] names each command once (its label, its group in the cheat
//! sheet, which actions it covers, which tooltip labels belong to it) and the key shown is
//! whatever the live keymap says, so rebinding a key or adding a binding for a listed action
//! changes every tooltip, menu row, badge and the cheat sheet at once.
//!
//! Four things use it:
//!
//! * tooltips end with the key ("Add to Crate  C"), see [`tip_text`];
//! * context-menu rows show the key at their right edge, see [`menu_key`];
//! * the `?` cheat sheet lists every entry by group, see [`sheet`];
//! * holding Ctrl for a moment shows the single-key shortcuts on the buttons that have them
//!   ([`badge_for`]), and the third time a key's command is done with the mouse a one-line tip
//!   names the key ([`note_mouse_use`]), once ever per command.
//!
//! A new key a page binds appears in the cheat sheet's "More" group by itself when it is a
//! plain letter (named after its action). Add an [`Entry`] to give it a proper label, group and
//! tooltip.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{App, Entity, Global, KeyBinding, KeyContext, SharedString};

use crate::keymap::{GlobalKey, GLOBAL_BINDINGS};
use crate::settings::{SettingsModel, GLOBAL_HOTKEYS, KEY_BADGES, SHORTCUT_HINT_STATE, SHOW_SHORTCUT_HINTS};

/// The cheat sheet's groups, in the order they are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Play,
    Collect,
    Navigate,
    Window,
    /// Keys no entry names (see the module docs).
    More,
}

impl Group {
    pub const ALL: [Group; 5] = [
        Group::Play,
        Group::Collect,
        Group::Navigate,
        Group::Window,
        Group::More,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Play => "Play",
            Group::Collect => "Collect",
            Group::Navigate => "Navigate",
            Group::Window => "Window",
            Group::More => "More",
        }
    }
}

/// One command that has keys.
#[derive(Debug)]
pub struct Entry {
    /// Stable: the nudge's memory is kept under it.
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// The short names (after `::`) of the actions that do this, one per page that binds it.
    pub actions: &'static [&'static str],
    /// Which binding is *the* key when there are several (as `Keystroke::unparse` writes it).
    pub prefer: Option<&'static str>,
    /// The cheat sheet lists every binding, not just the main one.
    pub all_keys: bool,
    /// Tooltip labels that mean this command ("Like" and "Unlike" are one command).
    pub tips: &'static [&'static str],
}

const fn entry(
    id: &'static str,
    label: &'static str,
    group: Group,
    actions: &'static [&'static str],
) -> Entry {
    Entry {
        id,
        label,
        group,
        actions,
        prefer: None,
        all_keys: false,
        tips: &[],
    }
}

impl Entry {
    const fn prefer(mut self, keystroke: &'static str) -> Self {
        self.prefer = Some(keystroke);
        self
    }

    const fn all_keys(mut self) -> Self {
        self.all_keys = true;
        self
    }

    const fn tips(mut self, tips: &'static [&'static str]) -> Self {
        self.tips = tips;
        self
    }
}

/// Every command with a key. Order is the cheat sheet's order inside a group.
pub const ENTRIES: &[Entry] = &[
    entry("play-pause", "Play / pause", Group::Play, &["TogglePlay"]).tips(&["Play", "Pause"]),
    entry("next", "Next song", Group::Play, &["NextTrack"])
        .prefer("shift-right")
        .tips(&["Next"]),
    entry("previous", "Previous song", Group::Play, &["PreviousTrack"])
        .prefer("shift-left")
        .tips(&["Previous"]),
    entry("seek", "Seek 5 s", Group::Play, &["SeekBack", "SeekForward"]).all_keys(),
    entry("volume", "Volume 5%", Group::Play, &["VolumeUp", "VolumeDown"]).all_keys(),
    entry("mute", "Mute", Group::Play, &["ToggleMute"]).tips(&["Mute", "Unmute"]),
    entry(
        "like",
        "Like / unlike",
        Group::Collect,
        &["LikeCurrent", "LikeSelected", "LikeRow", "LikeCursor"],
    )
    .tips(&["Like", "Unlike"]),
    entry(
        "crate",
        "Add to Crate",
        Group::Collect,
        &["CrateCurrent", "CrateSelected", "CrateRow", "CrateCursor", "CrateSong"],
    )
    .tips(&["Add to Crate"]),
    entry(
        "playlist",
        "Add to playlist",
        Group::Collect,
        &[
            "AddCurrentToPlaylist",
            "PlaylistSelected",
            "AddRowToPlaylist",
            "PlaylistCursor",
        ],
    )
    .tips(&["Add to playlist"]),
    entry(
        "target",
        "Add to target playlist",
        Group::Collect,
        &[
            "AddCurrentToTarget",
            "TargetSelected",
            "AddRowToTarget",
            "TargetCursor",
        ],
    ),
    entry("remove", "Remove", Group::Collect, &["RemoveCursor"]),
    entry(
        "move",
        "Move up / down",
        Group::Collect,
        &["MoveCursorUp", "MoveCursorDown", "MoveRowUp", "MoveRowDown"],
    )
    .all_keys(),
    entry("crate-top", "Crate: move to top", Group::Collect, &["CrateMoveTop"]),
    entry("crate-bottom", "Crate: move to bottom", Group::Collect, &["CrateMoveBottom"]),
    entry("crate-add", "Crate: add songs by search", Group::Collect, &["CrateFocusAdd"]),
    entry("crate-paste", "Crate: paste links or lines", Group::Collect, &["CratePaste"]),
    entry("undo", "Undo", Group::Collect, &["UndoQueue"]),
    entry("redo", "Redo", Group::Collect, &["RedoQueue"]).all_keys(),
    entry("skim-from", "Skim from here", Group::Play, &["SkimRow"]),
    entry("skim-crate", "While skimming: keep in Crate", Group::Collect, &["SkimCrate"]),
    entry("skim-keep", "Skim: keep this song", Group::Play, &["SkimKeep"]),
    entry("skim-stop", "Skim: stop and go back", Group::Play, &["SkimCancel"]),
    entry(
        "navigate",
        "Move around (lists, cards, sidebar)",
        Group::Navigate,
        &["NavigateUp", "NavigateDown", "NavigateLeft", "NavigateRight"],
    )
    .all_keys(),
    entry("search", "Search", Group::Navigate, &["FocusSearch"])
        .prefer("ctrl-k")
        .all_keys()
        .tips(&["Search"]),
    entry("lyrics", "Lyrics", Group::Navigate, &["ToggleLyrics"]).tips(&["Lyrics"]),
    entry("nav-back", "Back", Group::Navigate, &["GoBack"])
        .prefer("alt-left")
        .all_keys()
        .tips(&["Back"]),
    entry("nav-forward", "Forward", Group::Navigate, &["GoForward"])
        .tips(&["Forward"]),
    entry(
        "back",
        "Close",
        Group::Navigate,
        &["CloseNowPlaying", "Back"],
    )
    .all_keys(),
    entry("help", "This list", Group::Navigate, &["ShowShortcuts"]).prefer("shift-/"),
    entry(
        "crate-panel",
        "Jump into the Crate panel (again: close it)",
        Group::Window,
        &["ToggleCrateDock"],
    )
    .tips(&["Crate panel"]),
    entry("fullscreen", "Fullscreen", Group::Window, &["ToggleFullscreen"]),
];

/// Where a hint is for: the shell (a button in the player bar or Now Playing works wherever
/// focus is, so only the shell's own keys count) or a list (a row's action also has the list's
/// key, which only works with the cursor in that list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Shell,
    Any,
}

pub fn entry_by_id(id: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.id == id)
}

/// The command a tooltip label stands for.
pub fn entry_for_tip(label: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.tips.contains(&label))
}

// ---- reading the keymap ---------------------------------------------------------------------

/// A binding as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// "Ctrl+K", "C", "Shift+→".
    pub text: String,
    /// As `Keystroke::unparse` writes it ("ctrl-k"); what [`Entry::prefer`] compares with.
    pub unparsed: String,
    /// Just a key: no Ctrl, Alt, Shift or Win with it.
    pub single: bool,
}

fn short_name(binding: &KeyBinding) -> &'static str {
    let name = binding.action().name();
    name.rsplit("::").next().unwrap_or(name)
}

fn shell_context() -> [KeyContext; 1] {
    [KeyContext::parse("Shell").expect("static context")]
}

fn shown(binding: &KeyBinding) -> Option<Shown> {
    let strokes = binding.keystrokes();
    if strokes.is_empty() {
        return None;
    }
    let text = strokes
        .iter()
        .map(|k| {
            let m = k.modifiers();
            display(m.control, m.alt, m.shift, m.platform, k.key())
        })
        .collect::<Vec<_>>()
        .join(" ");
    let unparsed = strokes
        .iter()
        .map(|k| k.unparse())
        .collect::<Vec<_>>()
        .join(" ");
    let single = strokes.len() == 1 && {
        let m = strokes[0].modifiers();
        !(m.control || m.alt || m.shift || m.platform)
    };
    Some(Shown {
        text,
        unparsed,
        single,
    })
}

/// "Ctrl+Shift+Z", "→", "?": how a keystroke reads on screen.
pub fn display(control: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
    // Shift+/ is what typing "?" is.
    if shift && key == "/" && !control && !alt && !win {
        return "?".into();
    }
    let mut out = String::new();
    if control {
        out.push_str("Ctrl+");
    }
    if alt {
        out.push_str("Alt+");
    }
    if shift {
        out.push_str("Shift+");
    }
    if win {
        out.push_str("Win+");
    }
    out.push_str(&display_key(key));
    out
}

fn display_key(key: &str) -> String {
    match key {
        "space" => "Space".into(),
        "escape" => "Esc".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "enter" => "Enter".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Del".into(),
        "pageup" => "PgUp".into(),
        "pagedown" => "PgDn".into(),
        "tab" => "Tab".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => {
            let mut chars = k.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        }
    }
}

/// Every binding the keymap has for `entry`, in the order they were bound, without repeats (one
/// key bound in several lists shows once).
pub fn bindings_of(entry: &Entry, scope: Scope, cx: &App) -> Vec<Shown> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let shell = shell_context();
    let mut out: Vec<Shown> = Vec::new();
    for binding in keymap.bindings() {
        if !entry.actions.contains(&short_name(binding)) {
            continue;
        }
        if scope == Scope::Shell {
            let reachable = binding
                .predicate()
                .map_or(true, |p| p.eval(&shell));
            if !reachable {
                continue;
            }
        }
        if let Some(s) = shown(binding) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// The key to show for `entry`: its preferred binding, else the first one bound.
pub fn primary(entry: &Entry, scope: Scope, cx: &App) -> Option<Shown> {
    let all = bindings_of(entry, scope, cx);
    entry
        .prefer
        .and_then(|p| all.iter().find(|s| s.unparsed == p).cloned())
        .or_else(|| all.into_iter().next())
}

// ---- hints that follow the settings ---------------------------------------------------------

/// What the hints need to know that is not in the keymap. Installed by the shell.
pub struct KeyHelp {
    settings: Entity<SettingsModel>,
    ledger: RefCell<HintLedger>,
    badges: Cell<bool>,
    /// Shows a one-line tip (the shell's pill).
    show_tip: Rc<dyn Fn(SharedString, &mut App)>,
}

impl Global for KeyHelp {}

/// Make the hints work for this app: `show_tip` puts a line on screen for a few seconds.
pub fn install(
    settings: Entity<SettingsModel>,
    show_tip: impl Fn(SharedString, &mut App) + 'static,
    cx: &mut App,
) {
    let ledger = HintLedger::parse(&settings.read(cx).get_text(&SHORTCUT_HINT_STATE));
    cx.set_global(KeyHelp {
        settings,
        ledger: RefCell::new(ledger),
        badges: Cell::new(false),
        show_tip: Rc::new(show_tip),
    });
}

/// Settings > Keyboard > Show shortcut hints. On where nothing is installed (tests).
pub fn hints_on(cx: &App) -> bool {
    cx.try_global::<KeyHelp>()
        .map_or(true, |h| h.settings.read(cx).get(&SHOW_SHORTCUT_HINTS))
}

/// Settings > Keyboard > Hold Ctrl for key badges.
pub fn badges_enabled(cx: &App) -> bool {
    cx.try_global::<KeyHelp>()
        .is_some_and(|h| h.settings.read(cx).get(&KEY_BADGES))
}

/// Whether the key badges are on screen now.
pub fn badges_showing(cx: &App) -> bool {
    cx.try_global::<KeyHelp>().is_some_and(|h| h.badges.get())
}

/// Turns the badges on or off; true when that changed what is on screen.
pub fn set_badges(on: bool, cx: &App) -> bool {
    match cx.try_global::<KeyHelp>() {
        Some(h) => h.badges.replace(on) != on,
        None => false,
    }
}

/// A tooltip: `label`, then two spaces and the key when the label is a command that has one.
pub fn tip_text(label: &str, scope: Scope, cx: &App) -> SharedString {
    if hints_on(cx) {
        if let Some(key) = entry_for_tip(label).and_then(|e| primary(e, scope, cx)) {
            return format!("{label}  {}", key.text).into();
        }
    }
    label.to_owned().into()
}

/// The key of a menu row's command (by entry id), when hints are on.
pub fn menu_key(id: &str, cx: &App) -> Option<SharedString> {
    if !hints_on(cx) {
        return None;
    }
    let key = primary(entry_by_id(id)?, Scope::Any, cx)?;
    Some(key.text.into())
}

/// The badge for a button labelled `label`: its key, when the badges are showing and it is a
/// single key.
pub fn badge_for(label: &str, cx: &App) -> Option<SharedString> {
    if !badges_showing(cx) {
        return None;
    }
    let key = primary(entry_for_tip(label)?, Scope::Shell, cx)?;
    key.single.then(|| key.text.into())
}

/// The command was just done with the mouse. The third time (and only then, once ever per
/// command) a tip names its key. Keyboard use never counts.
pub fn note_mouse_use(id: &str, scope: Scope, cx: &mut App) {
    if !hints_on(cx) {
        return;
    }
    let Some(entry) = entry_by_id(id) else { return };
    let Some(key) = primary(entry, scope, cx) else {
        return;
    };
    let Some(help) = cx.try_global::<KeyHelp>() else {
        return;
    };
    let (settings, show_tip) = (help.settings.clone(), help.show_tip.clone());
    let (due, state) = {
        let mut ledger = help.ledger.borrow_mut();
        let due = ledger.note(id);
        (due, ledger.to_text())
    };
    settings.update(cx, |m, cx| m.set_text(&SHORTCUT_HINT_STATE, &state, cx));
    if due {
        show_tip(format!("Tip: press {}", key.text).into(), cx);
    }
}

/// [`note_mouse_use`] for the command a tooltip label stands for.
pub fn note_mouse_use_label(label: &str, scope: Scope, cx: &mut App) {
    if let Some(entry) = entry_for_tip(label) {
        note_mouse_use(entry.id, scope, cx);
    }
}

/// Uses at which the tip shows.
const TIP_AT_USE: u8 = 3;

/// How often each command was done with the mouse, and whether its tip was shown. Kept in the
/// settings table as `id:uses:shown;...`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HintLedger {
    commands: BTreeMap<String, (u8, bool)>,
}

impl HintLedger {
    pub fn parse(text: &str) -> Self {
        let mut commands = BTreeMap::new();
        for part in text.split(';') {
            let mut fields = part.split(':');
            let (Some(id), Some(uses), Some(shown)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if let (false, Ok(uses)) = (id.is_empty(), uses.parse::<u8>()) {
                commands.insert(id.to_owned(), (uses, shown == "1"));
            }
        }
        Self { commands }
    }

    pub fn to_text(&self) -> String {
        self.commands
            .iter()
            .map(|(id, (uses, shown))| format!("{id}:{uses}:{}", u8::from(*shown)))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Count one mouse use; true when this is the one the tip is for.
    pub fn note(&mut self, id: &str) -> bool {
        let (uses, shown) = self.commands.entry(id.to_owned()).or_default();
        if *shown {
            return false;
        }
        *uses = uses.saturating_add(1);
        if *uses >= TIP_AT_USE {
            *shown = true;
            return true;
        }
        false
    }
}

// ---- the cheat sheet's rows -----------------------------------------------------------------

/// One line of the cheat sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub keys: Vec<String>,
}

/// The cheat sheet: each group that has rows, with its rows.
pub fn sheet(cx: &App) -> Vec<(Group, Vec<Row>)> {
    let mut sections: Vec<(Group, Vec<Row>)> = Group::ALL.iter().map(|g| (*g, Vec::new())).collect();
    let section = |sections: &mut Vec<(Group, Vec<Row>)>, g: Group| -> usize {
        sections.iter().position(|(s, _)| *s == g).expect("every group is listed")
    };

    for entry in ENTRIES {
        let all = bindings_of(entry, Scope::Any, cx);
        let keys: Vec<String> = if entry.all_keys {
            all.iter().map(|s| s.text.clone()).collect()
        } else {
            primary(entry, Scope::Any, cx)
                .map(|s| vec![s.text])
                .unwrap_or_default()
        };
        if keys.is_empty() {
            continue;
        }
        let at = section(&mut sections, entry.group);
        sections[at].1.push(Row {
            label: entry.label.to_owned(),
            keys,
        });
    }

    // Plain letters nobody named: listed under their action's name.
    let known: Vec<&str> = ENTRIES.iter().flat_map(|e| e.actions.iter().copied()).collect();
    let mut unnamed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for binding in cx.key_bindings().borrow().bindings() {
        let name = short_name(binding);
        if known.contains(&name) {
            continue;
        }
        if let Some(s) = shown(binding) {
            let is_letter = s.single && s.text.chars().count() == 1 && s.text.chars().all(char::is_alphanumeric);
            if is_letter {
                let keys = unnamed.entry(humanize(name)).or_default();
                if !keys.contains(&s.text) {
                    keys.push(s.text);
                }
            }
        }
    }
    let more = section(&mut sections, Group::More);
    for (label, keys) in unnamed {
        sections[more].1.push(Row { label, keys });
    }

    // The global hotkeys, when they are on.
    let globals_on = cx
        .try_global::<KeyHelp>()
        .is_some_and(|h| h.settings.read(cx).get(&GLOBAL_HOTKEYS));
    if globals_on {
        let at = section(&mut sections, Group::Window);
        for (label, keys) in global_rows() {
            sections[at].1.push(Row { label, keys });
        }
    }

    sections.retain(|(_, rows)| !rows.is_empty());
    sections
}

fn global_key(key: GlobalKey) -> &'static str {
    match key {
        GlobalKey::Space => "Space",
        GlobalKey::Left => "←",
        GlobalKey::Right => "→",
        GlobalKey::Up => "↑",
        GlobalKey::Down => "↓",
        GlobalKey::H => "H",
    }
}

/// The global hotkeys as sheet rows ("Anywhere: ...").
fn global_rows() -> Vec<(String, Vec<String>)> {
    use crate::keymap::GlobalCommand::*;
    let keys_of = |commands: &[crate::keymap::GlobalCommand]| -> Vec<String> {
        commands
            .iter()
            .filter_map(|c| GLOBAL_BINDINGS.iter().find(|(g, _)| g == c))
            .map(|(_, key)| format!("Ctrl+Alt+{}", global_key(*key)))
            .collect()
    };
    vec![
        ("Anywhere: play / pause".into(), keys_of(&[PlayPause])),
        ("Anywhere: next / previous".into(), keys_of(&[Next, Previous])),
        ("Anywhere: volume".into(), keys_of(&[VolumeUp, VolumeDown])),
        ("Anywhere: mini player HUD".into(), keys_of(&[ToggleHud])),
    ]
}

/// "LikeSelected" -> "Like selected".
pub fn humanize(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests;
