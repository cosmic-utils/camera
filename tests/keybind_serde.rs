// SPDX-License-Identifier: GPL-3.0-only

use camera::app::keybind::SerializedKeyBind;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::widget::menu::key_bind::{KeyBind, Modifier};

fn kb(modifiers: Vec<Modifier>, key: Key) -> KeyBind {
    KeyBind { modifiers, key }
}

#[test]
fn round_trip_plain_character() {
    let kb = kb(vec![], Key::Character("a".into()));
    let s = SerializedKeyBind::from(&kb);
    assert_eq!(s.modifiers, Vec::<String>::new());
    assert_eq!(s.key, "a");
    assert_eq!(s.to_keybind(), Some(kb));
}

#[test]
fn round_trip_ctrl_character() {
    let kb = kb(vec![Modifier::Ctrl], Key::Character("a".into()));
    let s = SerializedKeyBind::from(&kb);
    assert_eq!(s.modifiers, vec!["Ctrl".to_string()]);
    assert_eq!(s.key, "a");
    assert_eq!(s.to_keybind(), Some(kb));
}

#[test]
fn round_trip_named_keys() {
    for named in [Named::F1, Named::Tab, Named::Enter, Named::Escape] {
        let kb = kb(vec![], Key::Named(named));
        let s = SerializedKeyBind::from(&kb);
        assert_eq!(
            s.to_keybind(),
            Some(kb),
            "round-trip failed for {:?}",
            named
        );
    }
}

#[test]
fn round_trip_super_shift_alt() {
    // Input order is arbitrary; serialization sorts, so we compare the
    // round-tripped result against a freshly-created kb using the same
    // sorted order that `from` produces (Alt < Shift < Super).
    let original = kb(
        vec![Modifier::Super, Modifier::Shift, Modifier::Alt],
        Key::Character("k".into()),
    );
    let s = SerializedKeyBind::from(&original);
    assert!(s.modifiers.contains(&"Super".to_string()));
    assert!(s.modifiers.contains(&"Shift".to_string()));
    assert!(s.modifiers.contains(&"Alt".to_string()));
    // Round-trip reconstructs modifiers in sorted order.
    let expected = kb(
        vec![Modifier::Alt, Modifier::Shift, Modifier::Super],
        Key::Character("k".into()),
    );
    assert_eq!(s.to_keybind(), Some(expected));
}

#[test]
fn round_trip_special_characters() {
    for c in ["+", "-", "0", ","] {
        let kb = kb(vec![Modifier::Ctrl], Key::Character(c.into()));
        let s = SerializedKeyBind::from(&kb);
        assert_eq!(s.to_keybind(), Some(kb), "round-trip failed for '{c}'");
    }
}

#[test]
fn unbound_sentinel() {
    let s = SerializedKeyBind::default();
    assert!(s.is_unbound());
    assert_eq!(s.to_keybind(), None);
}

#[test]
fn parse_rejects_garbage() {
    let s = SerializedKeyBind {
        modifiers: vec!["Bogus".into()],
        key: "a".into(),
    };
    assert_eq!(s.to_keybind(), None);

    let s = SerializedKeyBind {
        modifiers: vec![],
        key: "NotAKey".into(),
    };
    // Unknown named keys are rejected; single-character strings become Key::Character.
    // "NotAKey" is multi-char so it can't be a Character, and it's not a Named — reject.
    assert_eq!(s.to_keybind(), None);
}
