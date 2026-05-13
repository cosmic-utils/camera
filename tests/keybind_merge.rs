// SPDX-License-Identifier: GPL-3.0-only

use camera::app::keybind::{Action, Bindings, SerializedKeyBind};
use cosmic::iced::keyboard::Key;
use cosmic::widget::menu::key_bind::{KeyBind, Modifier};
use std::collections::HashMap;

fn kb_char(c: &str, modifiers: Vec<Modifier>) -> KeyBind {
    KeyBind {
        modifiers,
        key: Key::Character(c.into()),
    }
}

#[test]
fn defaults_round_trip_through_with_overrides_empty() {
    let defaults = Bindings::defaults();
    let merged = Bindings::with_overrides(&HashMap::new());
    // Compare via the public API: every action's binding must agree.
    for &a in Action::ALL {
        assert_eq!(
            defaults.keybind_for(a),
            merged.keybind_for(a),
            "binding for {a:?} differs between defaults and empty-override merge",
        );
    }
}

#[test]
fn override_swaps_action_keybind() {
    let mut overrides = HashMap::new();
    let new_combo = kb_char("k", vec![]);
    overrides.insert(Action::Capture, SerializedKeyBind::from(&new_combo));

    let b = Bindings::with_overrides(&overrides);

    assert_eq!(b.action_for(&new_combo), Some(Action::Capture));
    // Space (the default, delivered as Character " ") no longer maps to Capture.
    let space = kb_char(" ", vec![]);
    assert_eq!(b.action_for(&space), None);
}

#[test]
fn empty_serialized_unbinds_action() {
    let mut overrides = HashMap::new();
    overrides.insert(Action::Capture, SerializedKeyBind::default());

    let b = Bindings::with_overrides(&overrides);
    assert_eq!(b.keybind_for(Action::Capture), None);
}

#[test]
fn set_replaces_existing_binding_and_unbinds_conflict() {
    let mut b = Bindings::defaults();
    // Bind ToggleFocusAuto to Space (which currently belongs to Capture).
    let space = kb_char(" ", vec![]);
    b.set(Action::ToggleFocusAuto, Some(space.clone()));

    assert_eq!(b.action_for(&space), Some(Action::ToggleFocusAuto));
    assert_eq!(b.keybind_for(Action::Capture), None);
}

#[test]
fn set_none_unbinds_action() {
    let mut b = Bindings::defaults();
    let prev = b.keybind_for(Action::Capture).cloned();
    b.set(Action::Capture, None);
    assert_eq!(b.keybind_for(Action::Capture), None);
    // Other actions unaffected.
    assert!(b.keybind_for(Action::SwitchCamera).is_some());
    let _ = prev;
}

#[test]
fn version_bumps_on_set() {
    let mut b = Bindings::defaults();
    let v0 = b.version;
    b.set(Action::Capture, None);
    assert!(b.version > v0);
}
