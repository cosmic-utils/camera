// SPDX-License-Identifier: GPL-3.0-only

//! Capture button widget implementation

use crate::app::state::{AppModel, CameraMode, Message};
use crate::constants::ui;
use cosmic::Element;
use cosmic::iced::{Background, Color, Length};
use cosmic::widget;

/// Ring border thickness for the capture button.
const RING_WIDTH: f32 = 3.0;
/// Gap between the ring's inner edge and the filled circle.
const RING_GAP: f32 = 5.0;
/// Reference element size that the theme's `radius_xl` is designed for.
const RADIUS_XL_REFERENCE_SIZE: f32 = 48.0;
/// Size ratio of the photo-during-recording button relative to the main capture button.
const PHOTO_BTN_SIZE_RATIO: f32 = 0.55;

/// Get the theme-based corner radius for a given element size.
/// Scales the cosmic theme's radius_xl proportionally to the element size,
/// capped so it never exceeds a full circle.
fn theme_radius(corner_radius: f32, size: f32) -> f32 {
    // Scale theme radius proportionally (radius_xl is designed for ~48px elements)
    let scaled = corner_radius * (size / RADIUS_XL_REFERENCE_SIZE);
    scaled.min(size / 2.0)
}

/// Build a capture button with a static ring around an animated filled circle.
/// `button_size` scales the entire button uniformly (ring, gap, fill, border).
/// `anim_scale` only scales the inner filled circle (for press animation).
///
/// Layout (inside out):
///   fill circle (INNER * button_size * anim_scale)
///   → centered in ring inner area (INNER + GAP*2) * button_size
///   → ring border (RING_WIDTH * button_size)
fn build_ringed_circle(
    color: Color,
    ring_color: Color,
    button_size: f32,
    anim_scale: f32,
) -> Element<'static, Message> {
    let base_inner = ui::CAPTURE_BUTTON_INNER * button_size;
    let border_w = RING_WIDTH * button_size;

    // Fetch the theme once for corner radius and background luminance.
    let theme = cosmic::theme::active();
    let cosmic_theme = theme.cosmic();
    let corner_radius = cosmic_theme.corner_radii.radius_xl[0];

    let fill_size = base_inner * anim_scale;
    let fill_radius = theme_radius(corner_radius, fill_size);

    // Compensate for irradiation illusion: bright backgrounds make gaps between
    // dark elements appear wider. Reduce the gap on light themes so the button
    // looks perceptually identical across themes.
    let bg = cosmic_theme.bg_color();
    let bg_luminance = 0.299 * bg.red + 0.587 * bg.green + 0.114 * bg.blue;
    let gap = RING_GAP * button_size * (1.0 - 0.25 * bg_luminance);

    let ring_inner = base_inner + gap * 2.0;
    let ring_outer = ring_inner + border_w * 2.0;
    let ring_radius = theme_radius(corner_radius, ring_outer);

    // Filled circle
    let circle = widget::container(
        widget::Space::new()
            .width(Length::Fixed(fill_size))
            .height(Length::Fixed(fill_size)),
    )
    .style(move |_theme| widget::container::Style {
        background: Some(Background::Color(color)),
        border: cosmic::iced::Border {
            radius: [fill_radius; 4].into(),
            ..Default::default()
        },
        ..Default::default()
    });

    // Ring with fill centered inside
    let ringed = widget::container(
        widget::container(circle)
            .width(Length::Fixed(ring_inner))
            .height(Length::Fixed(ring_inner))
            .center_x(ring_inner)
            .center_y(ring_inner)
            .style(|_| widget::container::Style::default()),
    )
    .width(Length::Fixed(ring_outer))
    .height(Length::Fixed(ring_outer))
    .center_x(ring_outer)
    .center_y(ring_outer)
    .style(move |_theme| widget::container::Style {
        border: cosmic::iced::Border {
            radius: [ring_radius; 4].into(),
            width: border_w,
            color: ring_color,
        },
        ..Default::default()
    });

    // Stable layout container (doesn't change size with animation)
    widget::container(ringed)
        .width(Length::Fixed(ring_outer))
        .height(Length::Fixed(ring_outer))
        .center_x(ring_outer)
        .center_y(ring_outer)
        .style(|_| widget::container::Style::default())
        .into()
}

impl AppModel {
    /// Animate capture button scale toward a new target.
    /// Call this whenever the button's target scale changes.
    pub fn animate_capture_scale(&mut self, target: f32) {
        if (self.capture_scale_to - target).abs() < 0.001 {
            return; // already targeting this scale
        }
        // Start from the current animated scale
        self.capture_scale_from = self.current_capture_scale();
        self.capture_scale_to = target;
        self.capture_anim_start = Some(std::time::Instant::now());
    }

    /// Get the current animated capture button scale.
    pub fn current_capture_scale(&self) -> f32 {
        Self::ease_scale(
            self.capture_anim_start,
            self.capture_scale_from,
            self.capture_scale_to,
        )
    }

    /// Animate the photo-during-recording button scale toward a new target.
    pub fn animate_photo_btn_scale(&mut self, target: f32) {
        if (self.photo_btn_scale_to - target).abs() < 0.001 {
            return;
        }
        self.photo_btn_scale_from = self.current_photo_btn_scale();
        self.photo_btn_scale_to = target;
        self.photo_btn_anim_start = Some(std::time::Instant::now());
    }

    /// Get the current animated photo-during-recording button scale.
    pub fn current_photo_btn_scale(&self) -> f32 {
        Self::ease_scale(
            self.photo_btn_anim_start,
            self.photo_btn_scale_from,
            self.photo_btn_scale_to,
        )
    }

    /// Shared ease-out cubic interpolation for button scale animations.
    fn ease_scale(start: Option<std::time::Instant>, from: f32, to: f32) -> f32 {
        if let Some(start) = start {
            let duration_ms = if to < from { 120.0 } else { 200.0 };
            let t = (start.elapsed().as_secs_f32() * 1000.0 / duration_ms).min(1.0);
            let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
            from + (to - from) * eased
        } else {
            to
        }
    }

    /// Build the interactive capture circle (Shrink width, no outer Fill container).
    /// Used by both the centered layout and the recording row layout.
    pub(crate) fn build_capture_circle(&self) -> Element<'_, Message> {
        let is_disabled = self.transition_state.ui_disabled || self.burst_mode.is_active();

        let theme = cosmic::theme::active();
        let cosmic_theme = theme.cosmic();
        let text_hint = cosmic_theme.on_bg_color().into();
        let destructive: Color = cosmic_theme.destructive_color().into();

        let color = if is_disabled {
            Color::from_rgba(0.5, 0.5, 0.5, 0.3)
        } else {
            match self.mode {
                CameraMode::Photo => {
                    if self.quick_record.is_recording() {
                        destructive
                    } else {
                        text_hint
                    }
                }
                CameraMode::Video => destructive,
                CameraMode::Timelapse => destructive,
                CameraMode::Virtual => {
                    if self.virtual_camera.is_streaming() {
                        Color::from_rgb(0.1, 0.7, 0.2)
                    } else {
                        Color::from_rgb(0.2, 0.5, 0.9)
                    }
                }
            }
        };

        let scale = self.current_capture_scale();
        let circle = build_ringed_circle(color, color, 1.0, scale);

        if is_disabled {
            circle
        } else {
            let press_message = match self.mode {
                CameraMode::Photo => Message::CaptureButtonPressed,
                CameraMode::Video => Message::ToggleRecording,
                CameraMode::Virtual => Message::ToggleVirtualCamera,
                CameraMode::Timelapse => Message::ToggleTimelapse,
            };
            let mut area = widget::mouse_area(circle)
                .on_press(press_message)
                .interaction(cosmic::iced::mouse::Interaction::Pointer);
            if self.mode == CameraMode::Photo {
                area = area.on_release(Message::CaptureButtonReleased);
            }
            area.into()
        }
    }

    /// Build the main capture button (shown in all modes).
    /// Wraps the circle in a Fill container for centering when used standalone.
    pub fn build_capture_button(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let circle = self.build_capture_circle();

        widget::container(circle)
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding([spacing.space_xs, 0])
            .into()
    }

    /// Build the photo capture button shown during video recording (right side).
    pub fn build_photo_during_recording_button(&self) -> Element<'_, Message> {
        let scale = self.current_photo_btn_scale();
        let circle = build_ringed_circle(Color::WHITE, Color::WHITE, PHOTO_BTN_SIZE_RATIO, scale);

        widget::mouse_area(circle)
            .on_press(Message::Capture)
            .interaction(cosmic::iced::mouse::Interaction::Pointer)
            .into()
    }
}
