use crate::app::ContextPage;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;

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
