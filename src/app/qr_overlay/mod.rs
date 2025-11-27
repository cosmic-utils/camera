// SPDX-License-Identifier: MPL-2.0

//! QR code overlay module
//!
//! This module provides widgets for rendering QR code detection results
//! as overlays on top of the camera preview. It includes:
//!
//! - Transparent boxes with themed borders around detected QR codes
//! - Context-aware action buttons based on QR code content
//!
//! # Coordinate System
//!
//! QR detections use normalized coordinates (0.0 to 1.0) relative to the
//! camera frame. The overlay widget handles transformation to screen
//! coordinates at render time, accounting for video scaling and letterboxing.

pub mod action_button;
mod widget;

use crate::app::frame_processor::{QrAction, QrDetection};
use crate::app::state::Message;
use crate::app::video_widget::VideoContentFit;
use cosmic::Element;
use cosmic::iced::{Color, Length};

/// Border width for QR overlay boxes (in pixels)
const OVERLAY_BORDER_WIDTH: f32 = 3.0;

/// Corner radius for QR overlay boxes
const OVERLAY_BORDER_RADIUS: f32 = 8.0;

/// Minimum size for overlay boxes (prevents tiny boxes for small QR codes)
const MIN_OVERLAY_SIZE: f32 = 60.0;

/// Gap between QR box and action button
const BUTTON_GAP: f32 = 8.0;

/// Build the QR overlay layer using a custom widget
///
/// This creates an overlay element that renders boxes around detected QR codes
/// and action buttons below them. The widget handles coordinate transformation
/// at render time to correctly position elements over the video content.
pub fn build_qr_overlay<'a>(
    detections: &[QrDetection],
    frame_width: u32,
    frame_height: u32,
    content_fit: VideoContentFit,
    mirrored: bool,
) -> Element<'a, Message> {
    if detections.is_empty() {
        return cosmic::widget::Space::new(Length::Fill, Length::Fill).into();
    }

    // Use the custom overlay widget that handles positioning at render time
    widget::QrOverlayWidget::new(
        detections.to_vec(),
        frame_width,
        frame_height,
        content_fit,
        mirrored,
    )
    .into()
}

/// Get the border color for a QR action type
///
/// Uses COSMIC theme-inspired colors based on the action type.
pub fn get_action_color(action: &QrAction) -> Color {
    match action {
        QrAction::Url(_) => Color::from_rgb(0.29, 0.56, 0.89), // Blue for links
        QrAction::Wifi { .. } => Color::from_rgb(0.30, 0.69, 0.31), // Green for WiFi
        QrAction::Phone(_) => Color::from_rgb(0.61, 0.35, 0.71), // Purple for phone
        QrAction::Email { .. } => Color::from_rgb(0.90, 0.49, 0.13), // Orange for email
        QrAction::Location { .. } => Color::from_rgb(0.96, 0.26, 0.21), // Red for location
        QrAction::Contact(_) => Color::from_rgb(0.00, 0.59, 0.53), // Teal for contacts
        QrAction::Event(_) => Color::from_rgb(0.91, 0.12, 0.39), // Pink for events
        QrAction::Sms { .. } => Color::from_rgb(0.55, 0.76, 0.29), // Light green for SMS
        QrAction::Text(_) => Color::from_rgb(0.62, 0.62, 0.62), // Gray for plain text
    }
}

/// Calculate the video content bounds within a container
///
/// Returns (offset_x, offset_y, video_width, video_height) for the actual
/// video content area within the container, accounting for letterboxing.
pub fn calculate_video_bounds(
    container_width: f32,
    container_height: f32,
    frame_width: u32,
    frame_height: u32,
    content_fit: VideoContentFit,
) -> (f32, f32, f32, f32) {
    let frame_aspect = frame_width as f32 / frame_height as f32;
    let container_aspect = container_width / container_height;

    match content_fit {
        VideoContentFit::Contain => {
            // Scale to fit within bounds (letterbox)
            let (video_width, video_height) = if frame_aspect > container_aspect {
                // Frame is wider - fit to width
                let video_width = container_width;
                let video_height = container_width / frame_aspect;
                (video_width, video_height)
            } else {
                // Frame is taller - fit to height
                let video_height = container_height;
                let video_width = container_height * frame_aspect;
                (video_width, video_height)
            };

            let offset_x = (container_width - video_width) / 2.0;
            let offset_y = (container_height - video_height) / 2.0;

            (offset_x, offset_y, video_width, video_height)
        }
        VideoContentFit::Cover => {
            // Fill entire container
            (0.0, 0.0, container_width, container_height)
        }
    }
}

/// Transform normalized QR detection coordinates to screen coordinates
pub fn transform_detection_to_screen(
    detection: &QrDetection,
    offset_x: f32,
    offset_y: f32,
    video_width: f32,
    video_height: f32,
    mirrored: bool,
) -> (f32, f32, f32, f32) {
    let bounds = &detection.bounds;

    // Scale from normalized (0-1) to video pixel coordinates
    let mut x = bounds.x * video_width;
    let y = bounds.y * video_height;
    let width = bounds.width * video_width;
    let height = bounds.height * video_height;

    // Handle mirroring (for front camera selfie mode)
    if mirrored {
        x = video_width - x - width;
    }

    // Add video offset (for letterboxing)
    let screen_x = x + offset_x;
    let screen_y = y + offset_y;

    // Ensure minimum size
    let screen_width = width.max(MIN_OVERLAY_SIZE);
    let screen_height = height.max(MIN_OVERLAY_SIZE);

    (screen_x, screen_y, screen_width, screen_height)
}
