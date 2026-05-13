// SPDX-License-Identifier: GPL-3.0-only

//! String-based serialization of `cosmic::widget::menu::key_bind::KeyBind`
//! used for persisting user shortcut overrides in `Config`.
//!
//! We do not serialize `KeyBind` directly because its `Key` field uses
//! `SmolStr` internally and we don't want our config schema to depend on
//! libcosmic's private representation.

use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::widget::menu::key_bind::{KeyBind, Modifier};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SerializedKeyBind {
    pub modifiers: Vec<String>,
    pub key: String,
}

impl From<&KeyBind> for SerializedKeyBind {
    fn from(kb: &KeyBind) -> Self {
        let mut modifiers: Vec<String> = kb
            .modifiers
            .iter()
            .map(|m| match m {
                Modifier::Super => "Super",
                Modifier::Ctrl => "Ctrl",
                Modifier::Alt => "Alt",
                Modifier::Shift => "Shift",
            })
            .map(str::to_string)
            .collect();
        // Sort so the persisted form is stable regardless of insertion order.
        modifiers.sort();
        modifiers.dedup();

        let key = match &kb.key {
            Key::Character(c) => c.to_string(),
            Key::Named(n) => format!("{n:?}"),
            // Identified / Unidentified / Dead are not user-bindable; persist as
            // empty (effectively unbound) — to_keybind() will reject them.
            _ => String::new(),
        };

        Self { modifiers, key }
    }
}

impl SerializedKeyBind {
    pub fn to_keybind(&self) -> Option<KeyBind> {
        let mut modifiers = Vec::with_capacity(self.modifiers.len());
        for m in &self.modifiers {
            modifiers.push(match m.as_str() {
                "Super" => Modifier::Super,
                "Ctrl" => Modifier::Ctrl,
                "Alt" => Modifier::Alt,
                "Shift" => Modifier::Shift,
                _ => return None,
            });
        }

        let key = if self.key.is_empty() {
            return None;
        } else if let Some(named) = parse_named(&self.key) {
            Key::Named(named)
        } else if self.key.chars().count() == 1 {
            Key::Character(self.key.clone().into())
        } else {
            return None;
        };

        Some(KeyBind { modifiers, key })
    }

    pub fn is_unbound(&self) -> bool {
        self.modifiers.is_empty() && self.key.is_empty()
    }
}

fn parse_named(s: &str) -> Option<Named> {
    // Whitelist the Named variants we care about for shortcuts. Add more if
    // a future default binding needs them.
    Some(match s {
        "Enter" => Named::Enter,
        "Tab" => Named::Tab,
        "Escape" => Named::Escape,
        "Backspace" => Named::Backspace,
        "Delete" => Named::Delete,
        "ArrowLeft" => Named::ArrowLeft,
        "ArrowRight" => Named::ArrowRight,
        "ArrowUp" => Named::ArrowUp,
        "ArrowDown" => Named::ArrowDown,
        "PageUp" => Named::PageUp,
        "PageDown" => Named::PageDown,
        "Home" => Named::Home,
        "End" => Named::End,
        "F1" => Named::F1,
        "F2" => Named::F2,
        "F3" => Named::F3,
        "F4" => Named::F4,
        "F5" => Named::F5,
        "F6" => Named::F6,
        "F7" => Named::F7,
        "F8" => Named::F8,
        "F9" => Named::F9,
        "F10" => Named::F10,
        "F11" => Named::F11,
        "F12" => Named::F12,
        _ => return None,
    })
}
