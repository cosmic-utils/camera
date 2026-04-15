// SPDX-License-Identifier: GPL-3.0-only

use crate::Message;
use crate::app::ContextPage;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;

//TODO: HashMap for reconfigurable ones

/// Keybinding are currently hardcoded as follows:
///
/// | Character | Binding |
/// |-----------|---------|
/// | F1 |			=> about |
/// | Enter |		=> take a picture or start/stop recording |
/// | Ctrl + Enter |=> start recording after delay |
/// | a |			=> toggles auto focus |
/// | c |			=> toggles color picker |
/// | e |			=> toggles exposure picker |
/// | f |			=> toggles flash |
/// | g |			=> opens gallery |
/// | n |			=> next mode |
/// | m |			=> toggle audio recording or mute/unmute |
/// | p |			=> toggle motor picker |
/// | q |			=> toggle QR detection |
/// | r |			=> toggle save burst RAW |
/// | s |			=> switches camera |
/// | t |			=> toggles timelapse |
/// | u |			=> toggles theatre UI |
/// | v |			=> toggles virtual camera |
/// |   |			=> toggles play/pause |
/// | Ctrl + a |	=> cycles aspect ratio |
/// | Ctrl + f |	=> toggles format picker |
/// | Ctrl + q |	=> currently noop //TODO: close app |
/// | Ctrl + r |	=> reset settings |
/// | Ctrl + t |	=> toggles theatre mode |
/// | Ctrl + + |	=> zoom in |
/// | Ctrl + - |	=> zoom out |
/// | Ctrl + 0 |	=> reset zoom |
/// | Ctrl + , |	=> opens/closes settings |
///
/// **Returns** a subscription to key events mapped similarly as GNOME Camera's for now
pub fn key_subscription(mode: CameraMode) -> Subscription<Message> {
    cosmic::iced::event::listen_raw(|event, _status, _window| {
        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };

        match &key {

	        Named::Key(Named::F1) if !modifiers.control() && !modifiers.logo() && !modifiers.alt() => Some(Message::ToggleContextPage(ContextPage::About)),
	        Named::Key(Named::Enter) if !modifiers.control() && !modifiers.logo() && !modifiers.alt() && mode == CameraMode::Video => Some(Message::ToggleRecording()),
	        Named::Key(Named::Enter) if !modifiers.control() && !modifiers.logo() && !modifiers.alt() && mode == CameraMode::Camera => Some(Message::Capture()),
            Named::Key(Named::Enter) if modifiers.control() && !modifiers.logo() && !modifiers.alt() => Some(Message::StartRecordingAfterDelay()),

            Key::Character(c) if modifiers.control() && !modifiers.logo() && !modifiers.alt() => {
				match c.as_str() {
				"a" => Some(Message::CyclePhotoAspectRatio()),
                "f" => Some(Message::ToggleFormatPicker()),
                "q" => Some(Message::Noop),
                "r" => Some(Message::ResetAllSettings()),
                "t" => Some(Message::ToggleTheatherMode()),
                "+" => Some(Message::ZoomIn()),
                "-" => Some(Message::ZoomOut()),
                " " => Some(Message::AbortPhotoTimer()),
                "0" => Some(Message::ResetZoom()),
                "," => Some(Message::ToggleContextPage(ContextPage::Settings)),
					_ => None,
				}
			}

			Key::Character(c) if !modifiers.control() && !modifiers.logo() && !modifiers.alt() {
				match c.as_str() {
			        "a" => Some(Message::ToggleFocusAuto()),
			        "c" => Some(Message::ToggleColorPicker()),
			        "e" => Some(Message::ToggleExposurePicker()),
			        "f" => Some(Message::ToggleFlash()),
			        "g" => Some(Message::OpenGallery()),
			        "n" => Some(Message::NextMode()),
			        "m" => Some(Message::ToggleRecordAudio()),
			        "p" => Some(Message::ToggleMotorPicker()),
			        "q" => Some(Message::ToggleQrDetection()),
			        "r" => Some(Message::ToggleSaveBurstRaw()),
			        "s" => Some(Message::SwitchCamera()),
			        "t" => Some(Message::ToggleTimelapse()),
			        "u" => Some(Message::TheatreToggleUI()),
			        "v" => Some(Message::ToggleVirtualCamera()),
					" " => Some(Message::ToggleVideoPlayPause()),
			        _ => None,
				}
        	}
        }
    })
}
