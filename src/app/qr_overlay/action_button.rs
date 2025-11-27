// SPDX-License-Identifier: MPL-2.0

//! Action buttons for QR code detections
//!
//! This module provides utilities for converting QR actions to app messages.

use crate::app::frame_processor::{QrAction, urlencoding_encode};
use crate::app::state::Message;

/// Convert a QR action to the appropriate app message
pub fn action_to_message(action: &QrAction) -> Message {
    match action {
        QrAction::Url(url) => Message::QrOpenUrl(url.clone()),
        QrAction::Wifi {
            ssid,
            password,
            security,
            hidden,
        } => Message::QrConnectWifi {
            ssid: ssid.clone(),
            password: password.clone(),
            security: security.display_name().to_string(),
            hidden: *hidden,
        },
        QrAction::Text(text) => Message::QrCopyText(text.clone()),
        QrAction::Phone(number) => Message::QrOpenUrl(format!("tel:{}", number)),
        QrAction::Email {
            address,
            subject,
            body,
        } => {
            let mut url = format!("mailto:{}", address);
            let mut params = Vec::new();
            if let Some(s) = subject {
                params.push(format!("subject={}", urlencoding_encode(s)));
            }
            if let Some(b) = body {
                params.push(format!("body={}", urlencoding_encode(b)));
            }
            if !params.is_empty() {
                url.push('?');
                url.push_str(&params.join("&"));
            }
            Message::QrOpenUrl(url)
        }
        QrAction::Sms { number, message } => {
            let url = if let Some(msg) = message {
                format!("sms:{}?body={}", number, urlencoding_encode(msg))
            } else {
                format!("sms:{}", number)
            };
            Message::QrOpenUrl(url)
        }
        QrAction::Location {
            latitude,
            longitude,
            label,
        } => {
            // Open in default map application
            let url = if let Some(lbl) = label {
                format!(
                    "geo:{},{}?q={},{} ({})",
                    latitude, longitude, latitude, longitude, lbl
                )
            } else {
                format!("geo:{},{}", latitude, longitude)
            };
            Message::QrOpenUrl(url)
        }
        QrAction::Contact(vcard) => Message::QrCopyText(vcard.clone()),
        QrAction::Event(vcal) => Message::QrCopyText(vcal.clone()),
    }
}
