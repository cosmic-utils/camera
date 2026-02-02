// SPDX-License-Identifier: GPL-3.0-only

//! Custom video widget for efficient camera preview rendering with GPU primitives
//!
//! This widget achieves the same optimizations as iced_video_player:
//! 1. Direct GPU texture updates (no Handle recreation)
//! 2. GPU-side filter processing via WGSL shaders
//! 3. Persistent textures across frames
//! 4. Native RGBA format for simplified processing

use crate::app::state::{FilterType, Message};
use crate::app::video_primitive::{VideoFrame, VideoPrimitive};
use crate::backends::camera::types::CameraFrame;
use cosmic::iced::advanced::widget::Tree;
use cosmic::iced::advanced::{Clipboard, Shell, Widget, layout};
use cosmic::iced::event::Status;
use cosmic::iced::mouse;
use cosmic::iced::{Element, Event, Length, Rectangle, Size};
use cosmic::iced_wgpu::primitive::Renderer as PrimitiveRenderer;
use cosmic::{Renderer, Theme};
use std::sync::Arc;

/// Content fit mode for video scaling
#[derive(Debug, Clone, Copy)]
pub enum VideoContentFit {
    /// Scale to fit within bounds, maintaining aspect ratio (letterboxing)
    Contain,
    /// Scale to fill bounds completely, maintaining aspect ratio (cropping)
    Cover,
}

/// Video widget that renders camera frames using a custom GPU primitive
pub struct VideoWidget {
    primitive: VideoPrimitive,
    width: Length,
    height: Length,
    aspect_ratio: f32,
    content_fit: VideoContentFit,
    /// Enable scroll wheel zoom (only for main camera preview, not filter picker)
    scroll_zoom_enabled: bool,
}

impl VideoWidget {
    /// Create a new video widget from a camera frame
    ///
    /// # Arguments
    /// * `crop_uv` - Optional crop UV coordinates (u_min, v_min, u_max, v_max) in 0-1 range
    /// * `zoom_level` - Zoom level (1.0 = no zoom, 2.0 = 2x zoom)
    /// * `scroll_zoom_enabled` - Whether scroll wheel zoom is enabled
    pub fn new(
        frame: Arc<CameraFrame>,
        video_id: u64,
        content_fit: VideoContentFit,
        filter_type: FilterType,
        corner_radius: f32,
        mirror_horizontal: bool,
        crop_uv: Option<(f32, f32, f32, f32)>,
        zoom_level: f32,
        scroll_zoom_enabled: bool,
    ) -> Self {
        let mut primitive = VideoPrimitive::new(video_id);
        primitive.filter_type = filter_type;
        primitive.corner_radius = corner_radius;
        primitive.mirror_horizontal = mirror_horizontal;
        primitive.crop_uv = crop_uv;
        primitive.zoom_level = zoom_level;

        // Calculate aspect ratio from frame dimensions, adjusted for crop
        let aspect_ratio = if let Some((u_min, v_min, u_max, v_max)) = crop_uv {
            // Use cropped region's aspect ratio
            let crop_width = (u_max - u_min) * frame.width as f32;
            let crop_height = (v_max - v_min) * frame.height as f32;
            if crop_height > 0.0 {
                crop_width / crop_height
            } else {
                16.0 / 9.0
            }
        } else if frame.height > 0 {
            frame.width as f32 / frame.height as f32
        } else {
            16.0 / 9.0 // Default aspect ratio
        };

        // Create VideoFrame for RGBA format
        // IMPORTANT: We share the FrameData without copying to maintain zero-copy from GStreamer
        if frame.width > 0 && frame.height > 0 {
            let stride = if frame.stride > 0 {
                frame.stride
            } else {
                frame.width * 4 // Fallback: 4 bytes per pixel (RGBA)
            };

            let video_frame = VideoFrame {
                id: video_id,
                width: frame.width,
                height: frame.height,
                data: frame.data.clone(), // Clone FrameData - just refcount increment, no data copy
                stride,
            };

            primitive.update_frame(video_frame);
        }

        Self {
            primitive,
            width: Length::Fill,
            height: Length::Fill,
            aspect_ratio,
            content_fit,
            scroll_zoom_enabled,
        }
    }
}

impl Widget<crate::app::Message, Theme, Renderer> for VideoWidget {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // Get the maximum available space
        let max_size = limits.max();

        let final_size = match self.content_fit {
            VideoContentFit::Contain => {
                // Choose the scaling that fits within bounds (letterbox)
                let width = max_size.width;
                let height = max_size.height;

                let width_based_height = width / self.aspect_ratio;
                let height_based_width = height * self.aspect_ratio;

                if width_based_height <= height {
                    // Width is the limiting factor
                    Size::new(width, width_based_height)
                } else {
                    // Height is the limiting factor
                    Size::new(height_based_width, height)
                }
            }
            VideoContentFit::Cover => {
                // Fill the entire container - the primitive will handle aspect ratio and cropping
                max_size
            }
        };

        layout::Node::new(final_size)
    }

    fn on_event(
        &mut self,
        _tree: &mut Tree,
        event: Event,
        layout: layout::Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> Status {
        // Only handle scroll zoom if enabled (photo mode main preview)
        if !self.scroll_zoom_enabled {
            return Status::Ignored;
        }

        // Check if cursor is over the widget bounds
        let bounds = layout.bounds();
        if !cursor.is_over(bounds) {
            return Status::Ignored;
        }

        // Handle mouse wheel scroll for zoom
        if let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event {
            let scroll_delta = match delta {
                mouse::ScrollDelta::Lines { y, .. } => y,
                mouse::ScrollDelta::Pixels { y, .. } => y / 50.0, // Normalize pixel scrolling
            };

            if scroll_delta > 0.0 {
                // Scroll up = zoom in
                shell.publish(Message::ZoomIn);
                return Status::Captured;
            } else if scroll_delta < 0.0 {
                // Scroll down = zoom out
                shell.publish(Message::ZoomOut);
                return Status::Captured;
            }
        }

        Status::Ignored
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &cosmic::iced::advanced::renderer::Style,
        layout: layout::Layout<'_>,
        _cursor: cosmic::iced::mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // Update primitive with viewport size and content fit mode
        // The shader will handle Cover mode by adjusting UV coordinates
        self.primitive
            .update_viewport(bounds.width, bounds.height, self.content_fit);

        // Draw the custom primitive using the wgpu renderer's primitive support
        renderer.draw_primitive(bounds, self.primitive.clone());
    }
}

impl<'a> From<VideoWidget> for Element<'a, crate::app::Message, Theme, Renderer> {
    fn from(widget: VideoWidget) -> Self {
        Element::new(widget)
    }
}

/// Create a video widget from a camera frame
///
/// # Arguments
/// * `crop_uv` - Optional crop UV coordinates (u_min, v_min, u_max, v_max) in 0-1 range
/// * `zoom_level` - Zoom level (1.0 = no zoom, 2.0 = 2x zoom)
/// * `scroll_zoom_enabled` - Whether scroll wheel zoom is enabled
pub fn video_widget<'a>(
    frame: Arc<CameraFrame>,
    video_id: u64,
    content_fit: VideoContentFit,
    filter_type: FilterType,
    corner_radius: f32,
    mirror_horizontal: bool,
    crop_uv: Option<(f32, f32, f32, f32)>,
    zoom_level: f32,
    scroll_zoom_enabled: bool,
) -> Element<'a, crate::app::Message, Theme, Renderer> {
    Element::new(VideoWidget::new(
        frame,
        video_id,
        content_fit,
        filter_type,
        corner_radius,
        mirror_horizontal,
        crop_uv,
        zoom_level,
        scroll_zoom_enabled,
    ))
}
