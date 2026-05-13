// SPDX-License-Identifier: GPL-3.0-only

//! Active key bindings: a `HashMap<KeyBind, Action>` built from defaults
//! merged with user overrides.

use super::{Action, SerializedKeyBind};
use cosmic::widget::menu::key_bind::KeyBind;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Bindings {
    /// Active KeyBind → Action map. Mutate only through `set` /
    /// `reset_to_default` so `version` stays in sync.
    pub(crate) map: HashMap<KeyBind, Action>,
    /// Bumped on every mutation; included in subscription identity hashes
    /// so rebinds take effect without an app restart.
    pub version: u64,
}

impl Bindings {
    pub fn defaults() -> Self {
        let mut map = HashMap::new();
        for &action in Action::ALL {
            if let Some(kb) = action.default_keybind() {
                map.insert(kb, action);
            }
        }
        Self { map, version: 0 }
    }

    pub fn with_overrides(overrides: &HashMap<Action, SerializedKeyBind>) -> Self {
        let mut b = Self::defaults();
        for (&action, ser) in overrides {
            if let Some(default_kb) = action.default_keybind() {
                b.map.remove(&default_kb);
            }
            if let Some(kb) = ser.to_keybind() {
                b.map.remove(&kb);
                b.map.insert(kb, action);
            }
        }
        b
    }

    /// Consume `self` and return the underlying map. Used by the subscription
    /// closure which owns the map for the lifetime of the subscription.
    pub(crate) fn into_map(self) -> HashMap<KeyBind, Action> {
        self.map
    }

    pub fn action_for(&self, kb: &KeyBind) -> Option<Action> {
        self.map.get(kb).copied()
    }

    pub fn keybind_for(&self, action: Action) -> Option<&KeyBind> {
        self.map.iter().find(|(_, a)| **a == action).map(|(k, _)| k)
    }

    pub fn conflict_for(&self, kb: &KeyBind, ignore: Action) -> Option<Action> {
        self.map
            .iter()
            .find_map(|(existing, a)| (existing == kb && *a != ignore).then_some(*a))
    }

    pub fn set(&mut self, action: Action, kb: Option<KeyBind>) {
        self.map.retain(|_, a| *a != action);
        if let Some(kb) = kb {
            self.map.insert(kb, action);
        }
        self.version += 1;
    }

    pub fn reset_to_default(&mut self, action: Action) {
        self.set(action, action.default_keybind());
    }
}
