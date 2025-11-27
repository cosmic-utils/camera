// SPDX-License-Identifier: MPL-2.0

//! Core types for frame processing results
//!
//! These types represent the output of frame analysis tasks and are used
//! throughout the application for rendering overlays and handling user actions.

/// A rectangular region within a frame
///
/// Coordinates are normalized (0.0 to 1.0) relative to the frame dimensions.
/// This allows easy transformation to screen coordinates regardless of
/// the actual frame size or display scaling.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRegion {
    /// Left edge (0.0 = left of frame, 1.0 = right of frame)
    pub x: f32,
    /// Top edge (0.0 = top of frame, 1.0 = bottom of frame)
    pub y: f32,
    /// Width as fraction of frame width
    pub width: f32,
    /// Height as fraction of frame height
    pub height: f32,
}

impl FrameRegion {
    /// Create a frame region from pixel coordinates
    pub fn from_pixels(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        frame_width: u32,
        frame_height: u32,
    ) -> Self {
        Self {
            x: x as f32 / frame_width as f32,
            y: y as f32 / frame_height as f32,
            width: width as f32 / frame_width as f32,
            height: height as f32 / frame_height as f32,
        }
    }
}

/// WiFi security type parsed from QR code
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WifiSecurity {
    /// No security (open network)
    None,
    /// WEP security (legacy, insecure)
    Wep,
    /// WPA/WPA2 Personal
    Wpa,
    /// WPA2 Enterprise
    Wpa2Enterprise,
    /// WPA3
    Wpa3,
}

impl WifiSecurity {
    /// Parse security type from WiFi QR code string
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "WEP" => Self::Wep,
            "WPA" | "WPA2" => Self::Wpa,
            "WPA2-EAP" | "WPA3-EAP" => Self::Wpa2Enterprise,
            "WPA3" | "SAE" => Self::Wpa3,
            "NOPASS" | "" => Self::None,
            _ => Self::Wpa, // Default to WPA for unknown
        }
    }

    /// Get display name for the security type
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::None => "Open",
            Self::Wep => "WEP",
            Self::Wpa => "WPA/WPA2",
            Self::Wpa2Enterprise => "Enterprise",
            Self::Wpa3 => "WPA3",
        }
    }
}

/// Action type derived from QR code content
///
/// QR codes can contain various types of data. This enum represents
/// the parsed action that should be available to the user.
#[derive(Debug, Clone, PartialEq)]
pub enum QrAction {
    /// URL that can be opened in a browser
    Url(String),

    /// WiFi network credentials
    Wifi {
        /// Network name (SSID)
        ssid: String,
        /// Network password (None for open networks)
        password: Option<String>,
        /// Security type
        security: WifiSecurity,
        /// Hidden network flag
        hidden: bool,
    },

    /// Plain text that can be copied to clipboard
    Text(String),

    /// Phone number (tel: URI)
    Phone(String),

    /// Email address (mailto: URI)
    Email {
        address: String,
        subject: Option<String>,
        body: Option<String>,
    },

    /// SMS message (sms: or smsto: URI)
    Sms {
        number: String,
        message: Option<String>,
    },

    /// Geographic location (geo: URI)
    Location {
        latitude: f64,
        longitude: f64,
        label: Option<String>,
    },

    /// vCard contact information
    Contact(String),

    /// Calendar event (VCALENDAR)
    Event(String),
}

impl QrAction {
    /// Parse QR code content into an action
    ///
    /// Attempts to identify the content type and parse accordingly.
    /// Falls back to `Text` for unrecognized formats.
    pub fn parse(content: &str) -> Self {
        let trimmed = content.trim();

        // Check for WiFi QR code format: WIFI:S:<ssid>;T:<security>;P:<password>;;
        if trimmed.starts_with("WIFI:") {
            return Self::parse_wifi(trimmed);
        }

        // Check for URL schemes
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            return Self::Url(trimmed.to_string());
        }

        // Check for tel: URI
        if let Some(number) = trimmed.strip_prefix("tel:") {
            return Self::Phone(number.to_string());
        }

        // Check for mailto: URI
        if let Some(rest) = trimmed.strip_prefix("mailto:") {
            return Self::parse_mailto(rest);
        }

        // Check for sms: or smsto: URI
        if let Some(rest) = trimmed
            .strip_prefix("sms:")
            .or_else(|| trimmed.strip_prefix("smsto:"))
        {
            return Self::parse_sms(rest);
        }

        // Check for geo: URI
        if let Some(rest) = trimmed.strip_prefix("geo:") {
            if let Some(loc) = Self::parse_geo(rest) {
                return loc;
            }
        }

        // Check for vCard
        if trimmed.starts_with("BEGIN:VCARD") {
            return Self::Contact(trimmed.to_string());
        }

        // Check for calendar event
        if trimmed.starts_with("BEGIN:VCALENDAR") || trimmed.starts_with("BEGIN:VEVENT") {
            return Self::Event(trimmed.to_string());
        }

        // Check if it looks like a URL without scheme
        if trimmed.contains('.') && !trimmed.contains(' ') && trimmed.len() < 256 {
            // Could be a domain name - treat as URL
            if trimmed.contains("www.")
                || trimmed.ends_with(".com")
                || trimmed.ends_with(".org")
                || trimmed.ends_with(".net")
                || trimmed.ends_with(".io")
            {
                return Self::Url(format!("https://{}", trimmed));
            }
        }

        // Default to plain text
        Self::Text(trimmed.to_string())
    }

    /// Parse WiFi QR code format
    fn parse_wifi(content: &str) -> Self {
        let mut ssid = String::new();
        let mut password = None;
        let mut security = WifiSecurity::None;
        let mut hidden = false;

        // Remove WIFI: prefix and trailing ;;
        let content = content.strip_prefix("WIFI:").unwrap_or(content);
        let content = content.trim_end_matches(';');

        // Parse fields (format: T:WPA;S:network;P:password;H:true)
        for part in content.split(';') {
            if let Some((key, value)) = part.split_once(':') {
                // Handle escaped characters in values
                let value = value
                    .replace("\\;", ";")
                    .replace("\\:", ":")
                    .replace("\\\\", "\\")
                    .replace("\\,", ",");

                match key {
                    "S" => ssid = value,
                    "P" => password = Some(value),
                    "T" => security = WifiSecurity::from_str(&value),
                    "H" => hidden = value.eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }

        Self::Wifi {
            ssid,
            password,
            security,
            hidden,
        }
    }

    /// Parse mailto: URI
    fn parse_mailto(content: &str) -> Self {
        let (address, params) = content.split_once('?').unwrap_or((content, ""));

        let mut subject = None;
        let mut body = None;

        for param in params.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                let value = urlencoding_decode(value);
                match key.to_lowercase().as_str() {
                    "subject" => subject = Some(value),
                    "body" => body = Some(value),
                    _ => {}
                }
            }
        }

        Self::Email {
            address: address.to_string(),
            subject,
            body,
        }
    }

    /// Parse sms: URI
    fn parse_sms(content: &str) -> Self {
        let (number, params) = content.split_once('?').unwrap_or((content, ""));

        let mut message = None;

        for param in params.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                let value = urlencoding_decode(value);
                if key.to_lowercase() == "body" {
                    message = Some(value);
                }
            }
        }

        Self::Sms {
            number: number.to_string(),
            message,
        }
    }

    /// Parse geo: URI
    fn parse_geo(content: &str) -> Option<Self> {
        let (coords, params) = content.split_once('?').unwrap_or((content, ""));

        let parts: Vec<&str> = coords.split(',').collect();
        if parts.len() < 2 {
            return None;
        }

        let latitude = parts[0].parse::<f64>().ok()?;
        let longitude = parts[1].parse::<f64>().ok()?;

        let mut label = None;
        for param in params.split('&') {
            if let Some((key, value)) = param.split_once('=') {
                if key == "q" || key == "label" {
                    label = Some(urlencoding_decode(value));
                }
            }
        }

        Some(Self::Location {
            latitude,
            longitude,
            label,
        })
    }

    /// Get the primary action label for this QR code type
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::Url(_) => "Open Link",
            Self::Wifi { .. } => "Connect to WiFi",
            Self::Text(_) => "Copy Text",
            Self::Phone(_) => "Call",
            Self::Email { .. } => "Send Email",
            Self::Sms { .. } => "Send SMS",
            Self::Location { .. } => "Open Map",
            Self::Contact(_) => "Add Contact",
            Self::Event(_) => "Add Event",
        }
    }
}

/// Simple URL encoding for query parameters
pub(crate) fn urlencoding_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push('+'),
            _ => {
                for byte in c.to_string().as_bytes() {
                    result.push('%');
                    result.push_str(&format!("{:02X}", byte));
                }
            }
        }
    }
    result
}

/// Simple URL decoding for query parameters
fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// A detected QR code with its location and parsed content
#[derive(Debug, Clone, PartialEq)]
pub struct QrDetection {
    /// Bounding box of the QR code in normalized frame coordinates
    pub bounds: FrameRegion,
    /// Raw content decoded from the QR code
    pub content: String,
    /// Parsed action based on content type
    pub action: QrAction,
    /// Confidence score (0.0 to 1.0) if available
    pub confidence: Option<f32>,
}

impl QrDetection {
    /// Create a new QR detection result
    pub fn new(bounds: FrameRegion, content: String) -> Self {
        let action = QrAction::parse(&content);
        Self {
            bounds,
            content,
            action,
            confidence: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url() {
        assert!(matches!(
            QrAction::parse("https://example.com"),
            QrAction::Url(_)
        ));
        assert!(matches!(
            QrAction::parse("http://example.com/path"),
            QrAction::Url(_)
        ));
    }

    #[test]
    fn test_parse_wifi() {
        let action = QrAction::parse("WIFI:S:MyNetwork;T:WPA;P:mypassword;;");
        match action {
            QrAction::Wifi {
                ssid,
                password,
                security,
                hidden,
            } => {
                assert_eq!(ssid, "MyNetwork");
                assert_eq!(password, Some("mypassword".to_string()));
                assert_eq!(security, WifiSecurity::Wpa);
                assert!(!hidden);
            }
            _ => panic!("Expected Wifi action"),
        }
    }

    #[test]
    fn test_parse_wifi_hidden() {
        let action = QrAction::parse("WIFI:T:WPA;S:HiddenNet;P:secret;H:true;;");
        match action {
            QrAction::Wifi { hidden, .. } => {
                assert!(hidden);
            }
            _ => panic!("Expected Wifi action"),
        }
    }

    #[test]
    fn test_parse_phone() {
        let action = QrAction::parse("tel:+1234567890");
        match action {
            QrAction::Phone(number) => {
                assert_eq!(number, "+1234567890");
            }
            _ => panic!("Expected Phone action"),
        }
    }

    #[test]
    fn test_parse_mailto() {
        let action = QrAction::parse("mailto:test@example.com?subject=Hello&body=World");
        match action {
            QrAction::Email {
                address,
                subject,
                body,
            } => {
                assert_eq!(address, "test@example.com");
                assert_eq!(subject, Some("Hello".to_string()));
                assert_eq!(body, Some("World".to_string()));
            }
            _ => panic!("Expected Email action"),
        }
    }

    #[test]
    fn test_parse_geo() {
        let action = QrAction::parse("geo:37.7749,-122.4194?label=San+Francisco");
        match action {
            QrAction::Location {
                latitude,
                longitude,
                label,
            } => {
                assert!((latitude - 37.7749).abs() < 0.0001);
                assert!((longitude - (-122.4194)).abs() < 0.0001);
                assert_eq!(label, Some("San Francisco".to_string()));
            }
            _ => panic!("Expected Location action"),
        }
    }

    #[test]
    fn test_parse_plain_text() {
        let action = QrAction::parse("Hello World!");
        assert!(matches!(action, QrAction::Text(_)));
    }

    #[test]
    fn test_frame_region_from_pixels() {
        let region = FrameRegion::from_pixels(100, 50, 200, 100, 1000, 500);
        assert!((region.x - 0.1).abs() < 0.001);
        assert!((region.y - 0.1).abs() < 0.001);
        assert!((region.width - 0.2).abs() < 0.001);
        assert!((region.height - 0.2).abs() < 0.001);
    }
}
