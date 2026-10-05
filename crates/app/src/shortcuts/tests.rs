use super::*;

#[test]
fn keystrokes_read_like_keycaps() {
    assert_eq!(display(false, false, false, false, "c"), "C");
    assert_eq!(display(true, false, false, false, "k"), "Ctrl+K");
    assert_eq!(display(true, false, true, false, "z"), "Ctrl+Shift+Z");
    assert_eq!(display(false, false, true, false, "right"), "Shift+→");
    assert_eq!(display(false, true, false, false, "up"), "Alt+↑");
    assert_eq!(display(false, false, false, false, "space"), "Space");
    assert_eq!(display(false, false, false, false, "escape"), "Esc");
    assert_eq!(display(false, false, false, false, "f11"), "F11");
    assert_eq!(display(false, false, false, false, "delete"), "Del");
    assert_eq!(display(false, false, true, false, "/"), "?");
}

#[test]
fn action_names_become_labels() {
    assert_eq!(humanize("LikeSelected"), "Like selected");
    assert_eq!(humanize("Skim"), "Skim");
    assert_eq!(humanize("KeepInCrate"), "Keep in crate");
}

#[test]
fn the_third_use_is_the_one_that_gets_a_tip_and_only_once() {
    let mut ledger = HintLedger::default();
    assert!(!ledger.note("crate"));
    assert!(!ledger.note("crate"));
    assert!(ledger.note("crate"));
    for _ in 0..10 {
        assert!(!ledger.note("crate"), "once ever");
    }
    // Another command counts on its own.
    assert!(!ledger.note("like"));
}

#[test]
fn the_ledger_survives_a_restart_and_garbage() {
    let mut ledger = HintLedger::default();
    ledger.note("crate");
    ledger.note("crate");
    ledger.note("crate");
    ledger.note("like");
    let text = ledger.to_text();
    assert_eq!(text, "crate:3:1;like:1:0");
    let mut again = HintLedger::parse(&text);
    assert_eq!(again, ledger);
    assert!(!again.note("crate"), "the tip was shown before the restart");
    assert!(!again.note("like"));
    assert!(again.note("like"), "the third like, across the restart");
    assert_eq!(HintLedger::parse("nonsense;;a:b:c;x:1"), HintLedger::default());
}

#[test]
fn entries_are_unique_and_every_tooltip_label_belongs_to_one_command() {
    for (i, a) in ENTRIES.iter().enumerate() {
        for b in &ENTRIES[i + 1..] {
            assert_ne!(a.id, b.id);
            for action in a.actions {
                assert!(!b.actions.contains(action), "{action} is in two entries");
            }
            for tip in a.tips {
                assert!(!b.tips.contains(tip), "{tip} is in two entries");
            }
        }
    }
}
