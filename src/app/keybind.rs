use crate::app::ContextPage;
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;

/// **Returns** a subscription to key events mapped similarly as GNOME Camera's for now
pub fn key_subscription(mode: CameraMode) -> Subscription<Message> {
    cosmic::iced::event::listen_raw(|event, _status, _window| {
        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };

        if modifiers.shift {
            return match &key {
                n => Some(Message::PrevMode()),
                _ => None,
            };
        }

        if modifiers.ctrl {
            return match &key {
                a => Some(Message::CyclePhotoAspectRatio()),
                c => Some(Message::ToggleColorPicker()),
                f => Some(Message::ToggleFormatPicker()),
                q => Some(Message::KeyPressed(Key::Named(Named::Q))),
                r => Some(Message::ResetAllSettings()),
                t => Some(Message::ToggleTheatherMode()),
                Named::Plus => Some(Message::ZoomIn()),
                Named::Minus => Some(Message::ZoomOut()),
                Named::Enter => Some(Message::StartRecordingAfterDelay()),
                Named::Space => Some(Message::AbortPhotoTimer()),
                Named::Zero => Some(Message::ResetZoom()),
                Named::Comma => Some(Message::ToggleContextPage(ContextPage::Settings)),
                _ => None,
            };
        }

        match &key {
            a => Some(Message::ToggleFocusAuto()),
            c => Some(Message::ToggleColorPicker()),
            e => Some(Message::ToggleExposurePicker()),
            f => Some(Message::ToggleFlash()),
            g => Some(Message::OpenGallery()),
            n => Some(Message::NextMode()),
            m => Some(Message::ToggleRecordAudio()),
            p => Some(Message::ToggleMotorPicker()),
            q => Some(Message::ToggleQrDetection()),
            r => Some(Message::ToggleSaveBurstRaw()),
            s => Some(Message::SwitchCamera()),
            t => Some(Message::ToggleTimelapse()),
            u => Some(Message::TheatreToggleUI()),
            v => Some(Message::ToggleVirtualCamera()),
            Named::F1 => Some(Message::ToggleContextPage(ContextPage::About)),
            Named::Enter => Some(Message::ToggleRecording()),
            Named::Space => Some(Message::ToggleVideoPlayPause()),
            _ => None,
        }
    })
}
