// SPDX-License-Identifier: GPL-3.0-only

//! Keyboard shortcuts, bindings, and the help/rebinding UI.

pub mod action;
pub mod bindings;
pub mod key_bindings_page;
pub mod serde;

pub use action::{Action, ActionCategory};
pub use bindings::Bindings;
pub use serde::SerializedKeyBind;

use crate::app::state::{CameraMode, Message};
use cosmic::iced::Event;
use cosmic::iced::Subscription;
use cosmic::iced::event;
use cosmic::iced::keyboard;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::widget::menu::key_bind::{KeyBind, Modifier};
use iced_futures::subscription as iced_sub;

/// Render a `KeyBind` for display in the UI.
///
/// libcosmic's `KeyBind: Display` writes `Key::Character(c)` verbatim, which
/// renders Space as an invisible glyph. We special-case the spacebar (and other
/// invisible characters) so users can read the binding.
pub fn format_keybind(kb: &KeyBind) -> String {
    fn key_label(key: &Key) -> String {
        match key {
            Key::Character(c) if c.as_str() == " " => "Space".to_string(),
            Key::Character(c) => c.to_uppercase(),
            Key::Named(n) => format!("{n:?}"),
            other => format!("{other:?}"),
        }
    }

    let mut s = String::new();
    for m in &kb.modifiers {
        s.push_str(&format!("{m:?}"));
        s.push_str(" + ");
    }
    s.push_str(&key_label(&kb.key));
    s
}

fn modifier_vec(m: keyboard::Modifiers) -> Vec<Modifier> {
    let mut v = Vec::new();
    if m.control() {
        v.push(Modifier::Ctrl);
    }
    if m.shift() {
        v.push(Modifier::Shift);
    }
    if m.alt() {
        v.push(Modifier::Alt);
    }
    if m.logo() {
        v.push(Modifier::Super);
    }
    v
}

fn dispatch_capture(mode: CameraMode, has_file_source: bool) -> Option<Message> {
    if has_file_source {
        return Some(Message::ToggleVideoPlayPause);
    }
    Some(match mode {
        CameraMode::Photo => Message::Capture,
        // Timelapse mirrors Video: Space toggles the capture session on/off.
        CameraMode::Video => Message::ToggleRecording,
        CameraMode::Timelapse => Message::ToggleTimelapse,
        CameraMode::Virtual => Message::ToggleVirtualCamera,
        CameraMode::View => return None,
    })
}

/// Global keyboard-shortcuts subscription. Filters on `event::Status::Ignored`
/// so the shortcut never fires when a widget (text input, focused button) has
/// already consumed the event.
pub fn subscription(
    bindings: Bindings,
    mode: CameraMode,
    has_file_source: bool,
    is_video_recording: bool,
) -> Subscription<Message> {
    #[derive(Hash)]
    struct KeyShortcutsId {
        version: u64,
        mode: CameraMode,
        has_file_source: bool,
        is_video_recording: bool,
    }

    let id = KeyShortcutsId {
        version: bindings.version,
        mode,
        has_file_source,
        is_video_recording,
    };
    let map = bindings.into_map();

    iced_sub::filter_map(id, move |event| {
        let iced_sub::Event::Interaction { event, status, .. } = event else {
            return None;
        };
        if status != event::Status::Ignored {
            return None;
        }
        let Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modified_key,
            modifiers,
            ..
        }) = event
        else {
            return None;
        };

        // Esc closes drawers/pickers. libcosmic's keyboard_nav is disabled, so we
        // emit Message::Escape ourselves; the update handler routes it to on_escape().
        if let Key::Named(Named::Escape) = &key {
            return Some(Message::Escape);
        }

        // Match against `modified_key` (layout-aware: Shift+/ on US and
        // Shift+ß on German both yield "?"), falling back to the raw key.
        // KeyBind::matches handles case-insensitive character compare.
        let action = map
            .iter()
            .find(|(kb, _)| kb.matches(modifiers, &modified_key) || kb.matches(modifiers, &key))
            .map(|(_, a)| *a)?;

        if action == Action::Capture {
            return dispatch_capture(mode, has_file_source);
        }
        // PhotoSnapshot is meaningful only as the "photo during video recording"
        // shortcut. Suppress it in every other context so the same key (Enter)
        // doesn't redundantly fire Capture in Photo / Timelapse / etc.
        if action == Action::PhotoSnapshot {
            return is_video_recording.then_some(Message::Capture);
        }
        Some(action.message())
    })
}

/// Used only while `AppModel::recording_keybind` is `Some(_)`. Captures the
/// next non-modifier key press as the recorded combo. Escape cancels.
///
/// Unlike `subscription`, this does *not* gate on `event::Status::Ignored`:
/// the recording dialog must be able to capture keys that focused widgets
/// would normally consume, otherwise the user could not bind those keys.
///
/// Note: Esc itself cannot be bound — it always cancels the dialog.
pub fn capture_subscription() -> Subscription<Message> {
    #[derive(Hash)]
    struct KeyBindCaptureId;

    iced_sub::filter_map(KeyBindCaptureId, |event| {
        let iced_sub::Event::Interaction { event, .. } = event else {
            return None;
        };
        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };

        if let Key::Named(Named::Escape) = &key {
            return Some(Message::CancelKeyBindRecording);
        }

        // Ignore presses that are only modifiers — wait for the "real" key.
        if matches!(
            &key,
            Key::Named(Named::Control | Named::Shift | Named::Alt | Named::Super | Named::Meta)
        ) {
            return None;
        }

        let combo = KeyBind {
            modifiers: modifier_vec(modifiers),
            key: key.clone(),
        };
        Some(Message::KeyBindRecordingCaptured(combo))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: `Message` doesn't impl `PartialEq`, so we tag each variant we
    /// care about with a small discriminator for comparison.
    fn tag(m: Option<Message>) -> &'static str {
        match m {
            None => "none",
            Some(Message::Capture) => "capture",
            Some(Message::ToggleRecording) => "toggle-recording",
            Some(Message::ToggleTimelapse) => "toggle-timelapse",
            Some(Message::ToggleVirtualCamera) => "toggle-virtual-camera",
            Some(Message::ToggleVideoPlayPause) => "toggle-video-play-pause",
            _ => "other",
        }
    }

    #[test]
    fn dispatch_capture_per_mode() {
        assert_eq!(tag(dispatch_capture(CameraMode::Photo, false)), "capture");
        assert_eq!(
            tag(dispatch_capture(CameraMode::Video, false)),
            "toggle-recording"
        );
        assert_eq!(
            tag(dispatch_capture(CameraMode::Timelapse, false)),
            "toggle-timelapse"
        );
        assert_eq!(
            tag(dispatch_capture(CameraMode::Virtual, false)),
            "toggle-virtual-camera"
        );
        assert_eq!(tag(dispatch_capture(CameraMode::View, false)), "none");
    }

    #[test]
    fn dispatch_capture_with_file_source_always_play_pause() {
        for mode in [
            CameraMode::Photo,
            CameraMode::Video,
            CameraMode::Timelapse,
            CameraMode::Virtual,
            CameraMode::View,
        ] {
            assert_eq!(
                tag(dispatch_capture(mode, true)),
                "toggle-video-play-pause",
                "file-source override failed for {mode:?}",
            );
        }
    }
}
