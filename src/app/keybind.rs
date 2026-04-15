// SPDX-License-Identifier: GPL-3.0-only

use crate::app::ContextPage;
use crate::{Message, Subscription};
use cosmic::iced::Event;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;

//TODO: HashMap for reconfigurable ones

/// Keybinding are currently hardcoded as follows:
///
/// | Character | Binding |
/// |-----------|---------|
/// | F1 |          => about |
/// | Enter |       => take a picture or start/stop recording |
/// | a |           => toggles auto focus |
/// | c |           => toggles color picker |
/// | e |           => toggles exposure picker |
/// | f |           => toggles flash |
/// | g |           => opens gallery |
/// | n |           => next mode |
/// | m |           => toggle audio recording or mute/unmute |
/// | p |           => toggle motor picker |
/// | q |           => toggle QR detection |
/// | r |           => toggle save burst RAW |
/// | s |           => switches camera |
/// | t |           => toggles timelapse |
/// | u |           => toggles theatre UI |
/// | v |           => toggles virtual camera |
/// |   |           => toggles play/pause |
/// | Ctrl + a |    => cycles aspect ratio |
/// | Ctrl + f |    => toggles format picker |
/// | Ctrl + q |    => currently noop //TODO: close app |
/// | Ctrl + r |    => reset settings |
/// | Ctrl + t |    => toggles theatre mode |
/// | Ctrl + + |    => zoom in |
/// | Ctrl + - |    => zoom out |
/// | Ctrl + 0 |    => reset zoom |
/// | Ctrl + , |    => opens/closes settings |
/// | Ctrl + Enter |=> start recording after delay |
///
/// **Returns** a subscription to key events mapped similarly as GNOME Camera's for now
pub fn key_subscription() -> Subscription<Message> {
    fn event_to_functionality(
        event: Event,
        _status: cosmic::iced::event::Status,
        _window: cosmic::iced::window::Id,
    ) -> Option<Message> {
        let Event::Keyboard(Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };

        match &key {
            Key::Named(Named::F1)
                if !modifiers.control() && !modifiers.logo() && !modifiers.alt() =>
            {
                Some(Message::ToggleContextPage(ContextPage::About))
            }

            // TODO: couldn't capture mode, need to think about it
            Key::Named(Named::Enter)
                if !modifiers.control() && !modifiers.logo() && !modifiers.alt() =>
            {
                Some(Message::Capture)
            }
            Key::Named(Named::Enter)
                if modifiers.control() && !modifiers.logo() && !modifiers.alt() =>
            {
                Some(Message::StartRecordingAfterDelay)
            }

            Key::Character(c) if modifiers.control() && !modifiers.logo() && !modifiers.alt() => {
                match c.as_str() {
                    "a" => Some(Message::CyclePhotoAspectRatio),
                    "f" => Some(Message::ToggleFormatPicker),
                    "q" => Some(Message::Noop),
                    "r" => Some(Message::ResetAllSettings),
                    "t" => Some(Message::ToggleTheatreMode),
                    "+" => Some(Message::ZoomIn),
                    "-" => Some(Message::ZoomOut),
                    " " => Some(Message::AbortPhotoTimer),
                    "0" => Some(Message::ResetZoom),
                    "," => Some(Message::ToggleContextPage(ContextPage::Settings)),
                    _ => None,
                }
            }

            Key::Character(c) if !modifiers.control() && !modifiers.logo() && !modifiers.alt() => {
                match c.as_str() {
                    "a" => Some(Message::ToggleFocusAuto),
                    "c" => Some(Message::ToggleColorPicker),
                    "e" => Some(Message::ToggleExposurePicker),
                    "f" => Some(Message::ToggleFlash),
                    "g" => Some(Message::OpenGallery),
                    "n" => Some(Message::NextMode),
                    "m" => Some(Message::ToggleRecordAudio),
                    "p" => Some(Message::ToggleMotorPicker),
                    "q" => Some(Message::ToggleQrDetection),
                    "r" => Some(Message::ToggleSaveBurstRaw),
                    "s" => Some(Message::SwitchCamera),
                    "t" => Some(Message::ToggleTimelapse),
                    "u" => Some(Message::TheatreToggleUI),
                    "v" => Some(Message::ToggleVirtualCamera),
                    " " => Some(Message::ToggleVideoPlayPause),
                    _ => None,
                }
            }
        }
    }
    cosmic::iced::event::listen_raw(event_to_functionality)
}
