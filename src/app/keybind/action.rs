// SPDX-License-Identifier: GPL-3.0-only

//! User-rebindable actions and their categories.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Action {
    // Capture
    Capture,
    /// Take a still photo. In Video mode while recording, this triggers the
    /// "photo during recording" button without interrupting the video.
    PhotoSnapshot,

    // Camera
    SwitchCamera,
    ToggleFocusAuto,
    ToggleFlash,

    // Pickers / drawers
    ToggleExposurePicker,
    ToggleColorPicker,
    ToggleMotorPicker,
    ToggleFormatPicker,
    ToggleSettings,

    // Mode / display
    NextMode,
    PrevMode,

    // Zoom / framing
    ZoomIn,
    ZoomOut,
    ResetZoom,
    CyclePhotoAspectRatio,

    // App
    OpenGallery,
    ToggleAbout,
    ResetAllSettings,
    ShowShortcuts,
    QuitApp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionCategory {
    Capture,
    Camera,
    Pickers,
    Display,
    Zoom,
    App,
}

impl Action {
    /// Every action, in the display order used by the help dialog and
    /// settings sub-page. Ordering is grouped by category.
    pub const ALL: &'static [Action] = &[
        // Capture
        Action::Capture,
        Action::PhotoSnapshot,
        // Camera
        Action::SwitchCamera,
        Action::ToggleFocusAuto,
        Action::ToggleFlash,
        // Pickers
        Action::ToggleExposurePicker,
        Action::ToggleColorPicker,
        Action::ToggleMotorPicker,
        Action::ToggleFormatPicker,
        Action::ToggleSettings,
        // Display
        Action::NextMode,
        Action::PrevMode,
        // Zoom
        Action::ZoomIn,
        Action::ZoomOut,
        Action::ResetZoom,
        Action::CyclePhotoAspectRatio,
        // App
        Action::OpenGallery,
        Action::ToggleAbout,
        Action::ResetAllSettings,
        Action::ShowShortcuts,
        Action::QuitApp,
    ];

    pub fn category(self) -> ActionCategory {
        match self {
            Action::Capture | Action::PhotoSnapshot => ActionCategory::Capture,
            Action::SwitchCamera | Action::ToggleFocusAuto | Action::ToggleFlash => {
                ActionCategory::Camera
            }
            Action::ToggleExposurePicker
            | Action::ToggleColorPicker
            | Action::ToggleMotorPicker
            | Action::ToggleFormatPicker
            | Action::ToggleSettings => ActionCategory::Pickers,
            Action::NextMode | Action::PrevMode => ActionCategory::Display,
            Action::ZoomIn
            | Action::ZoomOut
            | Action::ResetZoom
            | Action::CyclePhotoAspectRatio => ActionCategory::Zoom,
            Action::OpenGallery
            | Action::ToggleAbout
            | Action::ResetAllSettings
            | Action::ShowShortcuts
            | Action::QuitApp => ActionCategory::App,
        }
    }

    pub fn default_keybind(self) -> Option<cosmic::widget::menu::key_bind::KeyBind> {
        use cosmic::iced::keyboard::Key;
        use cosmic::iced::keyboard::key::Named;
        use cosmic::widget::menu::key_bind::{KeyBind, Modifier};

        let kb = |modifiers: Vec<Modifier>, key: Key| KeyBind { modifiers, key };

        let ctrl = || vec![Modifier::Ctrl];

        Some(match self {
            // Spacebar arrives as Key::Character(" "), not a Named variant.
            Action::Capture => kb(vec![], Key::Character(" ".into())),
            Action::PhotoSnapshot => kb(vec![], Key::Named(Named::Enter)),

            Action::SwitchCamera => kb(vec![], Key::Character("s".into())),
            Action::ToggleFocusAuto => kb(vec![], Key::Character("a".into())),
            Action::ToggleFlash => kb(vec![], Key::Character("f".into())),

            Action::ToggleExposurePicker => kb(vec![], Key::Character("e".into())),
            Action::ToggleColorPicker => kb(vec![], Key::Character("c".into())),
            Action::ToggleMotorPicker => kb(vec![], Key::Character("p".into())),
            Action::ToggleFormatPicker => kb(ctrl(), Key::Character("f".into())),
            Action::ToggleSettings => kb(ctrl(), Key::Character(",".into())),

            Action::NextMode => kb(vec![], Key::Character("m".into())),
            Action::PrevMode => kb(vec![], Key::Character("n".into())),

            Action::ZoomIn => kb(ctrl(), Key::Character("+".into())),
            Action::ZoomOut => kb(ctrl(), Key::Character("-".into())),
            Action::ResetZoom => kb(ctrl(), Key::Character("0".into())),
            Action::CyclePhotoAspectRatio => kb(ctrl(), Key::Character("a".into())),

            Action::OpenGallery => kb(vec![], Key::Character("g".into())),
            Action::ToggleAbout => kb(vec![], Key::Named(Named::F1)),
            Action::ResetAllSettings => kb(ctrl(), Key::Character("r".into())),
            // `?` is matched against iced's `modified_key`, which is layout-
            // aware (Shift+/ on US, Shift+ß on German, etc. all yield "?").
            Action::ShowShortcuts => kb(vec![Modifier::Shift], Key::Character("?".into())),
            Action::QuitApp => kb(ctrl(), Key::Character("q".into())),
        })
    }

    /// Maps an `Action` to its `Message`. The subscription overrides this
    /// for `Action::Capture` to dispatch context-aware messages — every other
    /// action uses this mapping directly.
    pub fn message(self) -> crate::app::state::Message {
        use crate::app::state::{ContextPage, Message};
        match self {
            Action::Capture => Message::Capture,
            // PhotoSnapshot is suppressed in the subscription unless video is
            // recording. This mapping is the message the subscription emits
            // when that gate passes.
            Action::PhotoSnapshot => Message::Capture,

            Action::SwitchCamera => Message::SwitchCamera,
            Action::ToggleFocusAuto => Message::ToggleFocusAuto,
            Action::ToggleFlash => Message::ToggleFlash,

            Action::ToggleExposurePicker => Message::ToggleExposurePicker,
            Action::ToggleColorPicker => Message::ToggleColorPicker,
            Action::ToggleMotorPicker => Message::ToggleMotorPicker,
            Action::ToggleFormatPicker => Message::ToggleFormatPicker,
            Action::ToggleSettings => Message::ToggleContextPage(ContextPage::Settings),

            Action::NextMode => Message::NextMode,
            Action::PrevMode => Message::PrevMode,

            Action::ZoomIn => Message::ZoomIn,
            Action::ZoomOut => Message::ZoomOut,
            Action::ResetZoom => Message::ResetZoom,
            Action::CyclePhotoAspectRatio => Message::CyclePhotoAspectRatio,

            Action::OpenGallery => Message::OpenGallery,
            Action::ToggleAbout => Message::ToggleContextPage(ContextPage::About),
            Action::ResetAllSettings => Message::ResetAllSettings,
            Action::ShowShortcuts => Message::ToggleContextPage(ContextPage::KeyBindings),
            Action::QuitApp => Message::WindowClose,
        }
    }

    pub fn label(self) -> String {
        use crate::fl;
        match self {
            Action::Capture => fl!("action-capture"),
            Action::PhotoSnapshot => fl!("action-photo-snapshot"),
            Action::SwitchCamera => fl!("action-switch-camera"),
            Action::ToggleFocusAuto => fl!("action-toggle-focus-auto"),
            Action::ToggleFlash => fl!("action-toggle-flash"),
            Action::ToggleExposurePicker => fl!("action-toggle-exposure-picker"),
            Action::ToggleColorPicker => fl!("action-toggle-color-picker"),
            Action::ToggleMotorPicker => fl!("action-toggle-motor-picker"),
            Action::ToggleFormatPicker => fl!("action-toggle-format-picker"),
            Action::ToggleSettings => fl!("action-toggle-settings"),
            Action::NextMode => fl!("action-next-mode"),
            Action::PrevMode => fl!("action-prev-mode"),
            Action::ZoomIn => fl!("action-zoom-in"),
            Action::ZoomOut => fl!("action-zoom-out"),
            Action::ResetZoom => fl!("action-reset-zoom"),
            Action::CyclePhotoAspectRatio => fl!("action-cycle-photo-aspect-ratio"),
            Action::OpenGallery => fl!("action-open-gallery"),
            Action::ToggleAbout => fl!("action-toggle-about"),
            Action::ResetAllSettings => fl!("action-reset-all-settings"),
            Action::ShowShortcuts => fl!("action-show-shortcuts"),
            Action::QuitApp => fl!("action-quit-app"),
        }
    }
}

impl ActionCategory {
    pub const ALL: &'static [ActionCategory] = &[
        ActionCategory::Capture,
        ActionCategory::Camera,
        ActionCategory::Pickers,
        ActionCategory::Display,
        ActionCategory::Zoom,
        ActionCategory::App,
    ];

    pub fn label(self) -> String {
        use crate::fl;
        match self {
            ActionCategory::Capture => fl!("shortcut-category-capture"),
            ActionCategory::Camera => fl!("shortcut-category-camera"),
            ActionCategory::Pickers => fl!("shortcut-category-pickers"),
            ActionCategory::Display => fl!("shortcut-category-display"),
            ActionCategory::Zoom => fl!("shortcut-category-zoom"),
            ActionCategory::App => fl!("shortcut-category-app"),
        }
    }
}
