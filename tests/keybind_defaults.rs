// SPDX-License-Identifier: GPL-3.0-only

use camera::app::keybind::{Action, ActionCategory};
use cosmic::widget::menu::key_bind::KeyBind;
use std::collections::{HashMap, HashSet};

#[test]
fn all_contains_every_variant_exactly_once() {
    // 21 variants in the enum — keep this in sync with the spec.
    assert_eq!(Action::ALL.len(), 21);

    let set: HashSet<Action> = Action::ALL.iter().copied().collect();
    assert_eq!(set.len(), Action::ALL.len(), "ALL contains duplicates");
}

#[test]
fn every_action_has_category() {
    for a in Action::ALL {
        let _ = a.category();
    }
}

#[test]
fn every_category_has_at_least_one_action() {
    for cat in ActionCategory::ALL {
        assert!(
            Action::ALL.iter().any(|a| a.category() == *cat),
            "category {:?} has no actions",
            cat
        );
    }
}

#[test]
fn defaults_have_no_duplicates() {
    let mut seen: HashMap<KeyBind, Action> = HashMap::new();
    for &a in Action::ALL {
        if let Some(kb) = a.default_keybind() {
            if let Some(prev) = seen.insert(kb.clone(), a) {
                panic!(
                    "default keybind {:?} bound to both {:?} and {:?}",
                    kb, prev, a
                );
            }
        }
    }
}

#[test]
fn capture_default_is_space() {
    let kb = Action::Capture
        .default_keybind()
        .expect("Capture must have a default");
    // Space arrives as Key::Character(" ") in this iced version.
    assert_eq!(kb.key, cosmic::iced::keyboard::Key::Character(" ".into()));
    assert!(kb.modifiers.is_empty());
}

#[test]
fn mode_cycle_defaults() {
    // n cycles backward, m cycles forward.
    let n = Action::PrevMode
        .default_keybind()
        .expect("PrevMode must have a default");
    let m = Action::NextMode
        .default_keybind()
        .expect("NextMode must have a default");
    assert_eq!(n.key, cosmic::iced::keyboard::Key::Character("n".into()));
    assert_eq!(m.key, cosmic::iced::keyboard::Key::Character("m".into()));
}

#[test]
fn space_renders_as_word_not_glyph() {
    use camera::app::keybind::format_keybind;
    let space_kb = Action::Capture.default_keybind().unwrap();
    assert_eq!(format_keybind(&space_kb), "Space");
}

#[test]
fn every_default_keybind_round_trips_through_serde() {
    use camera::app::keybind::SerializedKeyBind;
    // If a new default uses a Named variant that parse_named doesn't know
    // about, the round-trip silently produces None — making the binding
    // un-persistable. This test catches that early.
    for &a in Action::ALL {
        let Some(kb) = a.default_keybind() else {
            continue;
        };
        let ser = SerializedKeyBind::from(&kb);
        let round_tripped = ser.to_keybind();
        assert!(
            round_tripped.is_some(),
            "default for {a:?} failed serde round-trip (key={:?})",
            kb.key,
        );
    }
}
