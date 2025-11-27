// SPDX-License-Identifier: MPL-2.0

//! Utility functions for message parsing and common operations

/// Parse resolution string in format "WIDTHxHEIGHT"
pub fn parse_resolution(resolution_str: &str) -> Option<(u32, u32)> {
    let parts: Vec<&str> = resolution_str.split('x').collect();
    if parts.len() == 2 {
        if let (Ok(width), Ok(height)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            return Some((width, height));
        }
    }
    None
}

/// Parse codec string in format "PIXEL_FORMAT - Description"
/// Returns the pixel format part
pub fn parse_codec(codec_str: &str) -> String {
    codec_str
        .split(" - ")
        .next()
        .unwrap_or(codec_str)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_resolution() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution("3840x2160"), Some((3840, 2160)));
        assert_eq!(parse_resolution("invalid"), None);
        assert_eq!(parse_resolution("1920"), None);
    }

    #[test]
    fn test_parse_codec() {
        assert_eq!(parse_codec("H264 - H.264/AVC"), "H264");
        assert_eq!(parse_codec("MJPEG"), "MJPEG");
        assert_eq!(parse_codec(""), "");
    }
}
