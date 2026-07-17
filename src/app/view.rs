// SPDX-License-Identifier: GPL-3.0-only

//! Main application view
//!
//! This module composes the main UI from modularized components:
//! - Camera preview (camera_preview module)
//! - Top bar with format picker (inline)
//! - Capture button (controls module)
//! - Bottom bar (bottom_bar module)
//! - Format picker overlay (format_picker module)

use crate::app::bottom_bar::slide_h::SlideH;
use crate::app::overlay_style::{
    OVERLAY_CONTAINER, PICKER_PANEL, POPUP_PANEL, overlay_chip_button_class,
};
use crate::app::preview_geometry::TOP_BAR_HEIGHT;
use crate::app::qr_overlay::build_qr_overlay;
use crate::app::state::{AppModel, BurstModeStage, CameraMode, FilterType, Message};
use crate::constants::resolution_thresholds;
use crate::constants::ui;
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget::{self, icon};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::info;

/// Flash icon SVG (lightning bolt)
const FLASH_ICON: &[u8] = include_bytes!("../../resources/button_icons/flash.svg");
/// Flash off icon SVG (lightning bolt with strike-through)
const FLASH_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/flash-off.svg");
/// Timer off icon SVG
const TIMER_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/timer-off.svg");
/// Timer 3s icon SVG
const TIMER_3_ICON: &[u8] = include_bytes!("../../resources/button_icons/timer-3.svg");
/// Timer 5s icon SVG
const TIMER_5_ICON: &[u8] = include_bytes!("../../resources/button_icons/timer-5.svg");
/// Timer 10s icon SVG
const TIMER_10_ICON: &[u8] = include_bytes!("../../resources/button_icons/timer-10.svg");
/// Aspect ratio native icon SVG
const ASPECT_NATIVE_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-native.svg");
/// Aspect ratio 4:3 icon SVG (landscape)
const ASPECT_4_3_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-4-3.svg");
/// Aspect ratio 3:4 icon SVG (portrait companion of 4:3)
const ASPECT_3_4_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-3-4.svg");
/// Aspect ratio 16:9 icon SVG (landscape)
const ASPECT_16_9_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-16-9.svg");
/// Aspect ratio 9:16 icon SVG (portrait companion of 16:9)
const ASPECT_9_16_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-9-16.svg");
/// Aspect ratio 2:1 (18:9) icon SVG (landscape)
const ASPECT_2_1_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-2-1.svg");
/// Aspect ratio 1:2 icon SVG (portrait companion of 2:1)
const ASPECT_1_2_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-1-2.svg");
/// Aspect ratio 1:1 icon SVG
const ASPECT_1_1_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-1-1.svg");
/// Exposure icon SVG
const EXPOSURE_ICON: &[u8] = include_bytes!("../../resources/button_icons/exposure.svg");
const TOOLS_GRID_ICON: &[u8] = include_bytes!("../../resources/button_icons/tools-grid.svg");
const FILTER_ICON: &[u8] = include_bytes!("../../resources/button_icons/image-filter.svg");
/// Moon icon SVG (burst mode)
const MOON_ICON: &[u8] = include_bytes!("../../resources/button_icons/moon.svg");
/// Moon off icon SVG (burst mode disabled, with strike-through)
const MOON_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/moon-off.svg");
/// Camera tilt/motor control icon SVG
const CAMERA_TILT_ICON: &[u8] = include_bytes!("../../resources/button_icons/camera-tilt.svg");

/// Burst mode progress bar dimensions
const BURST_MODE_PROGRESS_BAR_WIDTH: f32 = 200.0;
const BURST_MODE_PROGRESS_BAR_HEIGHT: f32 = 8.0;

/// Fallback aspect ratio used before the first window-resize event arrives.
const FALLBACK_ASPECT_RATIO: f32 = 16.0 / 9.0;

impl AppModel {
    /// Current window aspect ratio, populated from `on_window_resize`. Returns
    /// 16:9 as a fallback before the first resize event.
    pub fn screen_aspect_ratio(&self) -> f32 {
        if self.screen_width > 0.0 && self.screen_height > 0.0 {
            self.screen_width / self.screen_height
        } else {
            FALLBACK_ASPECT_RATIO
        }
    }

    /// `true` when the window is taller than wide. Drives the orientation
    /// flip applied to the aspect-ratio crop, the canvas overlay bars, the
    /// composition guide and the aspect-icon selection so all four agree
    /// with what the rotated preview shows.
    pub fn screen_is_portrait(&self) -> bool {
        self.screen_width > 0.0
            && self.screen_height > 0.0
            && self.screen_height > self.screen_width
    }

    /// Settled top-bar scrim / shader bar height. 0 in View mode (the
    /// preview takes the full window in fit/fill); `TOP_BAR_HEIGHT`
    /// otherwise.
    pub fn settled_top_ui_height(&self) -> f32 {
        if self.mode.is_view_only() {
            0.0
        } else {
            TOP_BAR_HEIGHT
        }
    }

    /// Animated top-bar scrim height. Interpolates between snapshots through
    /// `fit_animation` so the Photo↔View transition slides smoothly.
    pub fn top_ui_height(&self) -> f32 {
        let target = self.settled_top_ui_height();
        let Some(anim) = self.fit_animation else {
            return target;
        };
        anim.from.top_ui_height + (target - anim.from.top_ui_height) * self.fit_animation_eased()
    }

    /// Settled pixel height of the bottom UI scrim. The top edge sits at:
    ///
    /// - **View mode**: 0. The preview extends to the window's bottom edge
    ///   in fit/fill (the carousel renders on top of the live preview
    ///   without a scrim).
    /// - **Photo mode**: the top of the capture-button area. By construction
    ///   the symmetric `space_xs` paddings (`build_capture_button`'s top
    ///   padding and the zoom row's `control_spacing` bottom padding) make
    ///   that line coincide with the midpoint between the capture circle
    ///   and the zoom/fit row above it.
    /// - **Other modes**: a quarter of the capture button's bottom padding
    ///   (`space_xs / 4`) above the carousel's top edge — close to the
    ///   carousel but with a small visual gap so the bar doesn't appear
    ///   to swallow the bottom controls.
    ///
    /// Photo capture math (`cover_capture_crop`) reads this through the
    /// settled value so a shot taken mid-animation isn't cropped against
    /// an in-flight value.
    pub fn settled_bottom_ui_height(&self) -> f32 {
        if self.mode.is_view_only() {
            return 0.0;
        }
        let spacing = cosmic::theme::spacing();
        let bottom_bar_h = crate::app::bottom_bar::BOTTOM_BAR_HEIGHT;
        if self.mode == CameraMode::Photo {
            let capture_h = crate::app::controls::capture_button::CAPTURE_BUTTON_OUTER_SIZE
                + 2.0 * f32::from(spacing.space_xs);
            bottom_bar_h + capture_h
        } else {
            bottom_bar_h + f32::from(spacing.space_xs) / 4.0
        }
    }

    /// Animated bottom-bar scrim height. During an in-flight fit animation,
    /// interpolates from the captured starting height toward
    /// `settled_bottom_ui_height()` using the same eased progress as
    /// `cover_blend`. Drives the canvas scrim and the video shader's
    /// `bar_bottom_px`, so the preview's centre slides with the scrim during
    /// a Photo↔non-Photo transition.
    pub fn bottom_ui_height(&self) -> f32 {
        let target = self.settled_bottom_ui_height();
        let Some(anim) = self.fit_animation else {
            return target;
        };
        anim.from.bottom_ui_height
            + (target - anim.from.bottom_ui_height) * self.fit_animation_eased()
    }

    /// Settled height of the empty placeholder above the bottom bar. 0 in
    /// View (no capture button — fit/zoom row sits flush above the
    /// carousel); the capture button area otherwise.
    pub fn settled_capture_area_height(&self) -> f32 {
        if self.mode.is_view_only() {
            0.0
        } else {
            let spacing = cosmic::theme::spacing();
            crate::app::controls::capture_button::CAPTURE_BUTTON_OUTER_SIZE
                + 2.0 * f32::from(spacing.space_xs)
        }
    }

    /// Animated capture-area placeholder height. Interpolates through
    /// `fit_animation` so Photo↔View glides the fit/zoom row toward the
    /// carousel instead of snapping.
    pub fn capture_area_height(&self) -> f32 {
        let target = self.settled_capture_area_height();
        let Some(anim) = self.fit_animation else {
            return target;
        };
        anim.from.capture_area_height
            + (target - anim.from.capture_area_height) * self.fit_animation_eased()
    }

    /// Height the floating fit/zoom chip row occupies at the BOTTOM of the
    /// preview content area, i.e. *inside* `frame_rect_on_screen` rather than
    /// below it — the chips deliberately float over the live preview, so
    /// `bottom_ui_height()` stops above them.
    ///
    /// **Invariant**: must match the row `view()` actually builds — the fit
    /// chip's fixed `space_l` height plus the `control_spacing` (`space_xs`)
    /// bottom padding of its centring container, under the same visibility
    /// condition. The zoom chip is a `button::text` of the same standard
    /// height, and the row is `align_y(Center)`, so `space_l` is the row's
    /// height. Grep `show_zoom_label` if that row changes shape.
    ///
    /// Used by `build_overlay_popup` to keep popups out of the chip strip.
    fn zoom_chip_strip_height(&self) -> f32 {
        if self.mode.supports_fit_and_zoom() && !self.tools_menu_visible {
            let spacing = cosmic::theme::spacing();
            f32::from(spacing.space_l) + f32::from(spacing.space_xs)
        } else {
            0.0
        }
    }
}

/// Build a centered overlay popup dialog with icon, title, body text, and optional button
///
/// Used for modal-style popups (privacy warning, flash error). Frosted like the
/// rest of the overlay chrome: a live-blurred preview backdrop behind the theme's
/// translucent surface when frosting is on, opaque when it's off. The blur is
/// what keeps the text legible, so this no longer needs the near-opaque hardcoded
/// alpha it used to carry.
///
/// Takes `model` (rather than standing alone) because the backdrop needs the
/// current frame and preview transforms, which `frosted_panel` reads off it.
fn build_overlay_popup<'a>(
    model: &'a AppModel,
    icon: Element<'a, Message>,
    title: &str,
    body: &str,
    button: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let spacing = cosmic::theme::spacing();

    let mut content = widget::Column::new()
        .push(icon)
        .push(
            widget::text(title.to_string())
                .size(20)
                .font(cosmic::font::bold()),
        )
        .push(widget::text(body.to_string()).size(14))
        .spacing(spacing.space_s)
        .align_x(Alignment::Center);

    if let Some(btn) = button {
        content = content.push(btn);
    }

    // Padding goes on an inner container: `frosted_panel` wraps `content` in the
    // styled container itself, so the tint sizes to the padded content.
    let popup_box = model.frosted_panel(
        widget::container(content).padding(spacing.space_l).into(),
        POPUP_PANEL,
    );

    // Centre the popup in the CAMERA PREVIEW, not the window. The popup layer
    // is a full-window stack child, so window-centring dropped it below the
    // preview's middle (the bottom UI is much taller than the top bar) and let
    // it collide with the fit/zoom chips floating at the preview's bottom edge.
    //
    // Padding the layer by the UI bar heights reproduces `frame_rect_on_screen`
    // without re-deriving it: that helper's content rect spans
    // `top_ui_height()..H - bottom_ui_height()`, and both of its aspect-ratio
    // branches keep the framed rect *concentric* with that content rect. So a
    // Fill container inset by the two bar heights and centred lands exactly on
    // the frame rect's centre — for every ratio, and animating in step with the
    // bars during a Photo↔View transition.
    //
    // The bottom inset additionally reserves the chip strip, which lives INSIDE
    // the content rect (`bottom_ui_height()` stops above the chips). Centring on
    // the bare preview centre would clear today's popups by ~65 px but is not
    // collision-proof — a long flash-error message grows the popup vertically
    // and would reach the chips again. Reserving the strip makes the clearance
    // structural, and costs only ~22 px of upward shift (half the strip), so the
    // popup still reads as centred on the preview.
    widget::container(popup_box)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding([
            model.top_ui_height(),
            0.0,
            model.bottom_ui_height() + model.zoom_chip_strip_height(),
            0.0,
        ])
        .align_x(cosmic::iced::alignment::Horizontal::Center)
        .align_y(cosmic::iced::alignment::Vertical::Center)
        .into()
}

/// Create an icon button with a themed background for use on camera preview overlays.
/// `highlighted = true` switches to the accent (Suggested) class so toggle-state
/// buttons (flash, HDR, tools menu) show their active state visually.
fn overlay_icon_button<'a, M: Clone + 'static>(
    handle: impl Into<widget::icon::Handle>,
    message: Option<M>,
    highlighted: bool,
) -> Element<'a, M> {
    let mut button = widget::button::icon(handle).extra_small();
    if highlighted {
        button = button.class(cosmic::theme::Button::Suggested);
    }
    if let Some(msg) = message {
        button = button.on_press(msg);
    }
    button.into()
}

/// Animation duration for fit/fill transition.
pub const FIT_ANIMATION_DURATION: std::time::Duration = std::time::Duration::from_millis(300);

/// Animation duration for the zoom-reset transition.
pub const ZOOM_ANIMATION_DURATION: std::time::Duration = std::time::Duration::from_millis(300);

impl AppModel {
    /// Settled cover blend: 0.0 (Contain) when fit-to-view is enabled in a
    /// mode that supports it (Photo, View), 1.0 (Cover) everywhere else.
    /// The single source of truth for the preview's geometry target.
    ///
    /// Virtual mode is forced to Contain regardless of the toggle — the
    /// fit/fill chip is hidden there anyway (it's gated on
    /// `supports_fit_and_zoom`), and Cover would silently crop edges off
    /// what's being streamed to consumer apps, which doesn't match the
    /// "what you see is what you send" expectation.
    pub fn settled_blend(&self) -> f32 {
        if matches!(self.mode, crate::app::state::CameraMode::Virtual) {
            return 0.0;
        }
        if self.preview_fit_to_view && self.mode.supports_fit_and_zoom() {
            0.0
        } else {
            1.0
        }
    }

    /// Animated zoom level. During an in-flight zoom-reset transition,
    /// interpolates from the captured starting zoom toward `self.zoom_level`
    /// using the same ease-out cubic shape as the fit/fill animation.
    /// Pinch and step zoom clear `zoom_animation`, so they remain real-time.
    pub fn current_zoom_level(&self) -> f32 {
        let target = self.zoom_level;
        let Some(anim) = self.zoom_animation else {
            return target;
        };
        let t =
            (anim.start.elapsed().as_secs_f32() / ZOOM_ANIMATION_DURATION.as_secs_f32()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        anim.from + (target - anim.from) * eased
    }

    /// Eased progress of the in-flight fit animation, in [0, 1]. Returns 1.0
    /// when no animation is running (i.e. fully settled).
    fn fit_animation_eased(&self) -> f32 {
        let Some(anim) = self.fit_animation else {
            return 1.0;
        };
        let t =
            (anim.start.elapsed().as_secs_f32() / FIT_ANIMATION_DURATION.as_secs_f32()).min(1.0);
        // Ease-out cubic: 1 - (1-t)^3
        1.0 - (1.0 - t).powi(3)
    }

    /// Returns the current cover blend value (0.0 = contain/fit, 1.0 = cover/fill).
    /// During animation, returns an ease-out interpolation toward `settled_blend()`.
    pub fn cover_blend(&self) -> f32 {
        let target = self.settled_blend();
        let Some(anim) = self.fit_animation else {
            return target;
        };
        anim.from.blend + (target - anim.from.blend) * self.fit_animation_eased()
    }

    /// Snapshot every value that animates through a fit/fill transition.
    /// Callers take this *before* mutating `self.mode` or
    /// `self.preview_fit_to_view`, then pass the snapshot to
    /// `start_fit_animation`. Centralising the read here means a new
    /// animated channel only needs to be added once (struct + this method
    /// + the matching settled getter).
    pub fn capture_fit_state(&self) -> crate::app::state::FitFrom {
        crate::app::state::FitFrom {
            blend: self.cover_blend(),
            top_ui_height: self.top_ui_height(),
            bottom_ui_height: self.bottom_ui_height(),
            capture_area_height: self.capture_area_height(),
        }
    }

    /// Install a fit/fill animation if any of the animated values differ
    /// from where the eye currently is, returning the tick task that drives
    /// it (or `Task::none` when no animation is needed). Callers must mutate
    /// `self.mode` and/or `self.preview_fit_to_view` before calling so the
    /// settled values reflect the new state. If a tick chain is already in
    /// flight (i.e. `fit_animation` was already `Some`), no new chain is
    /// spawned — the existing one picks up the replaced animation on its
    /// next fire, so re-triggers don't double the tick rate.
    pub fn start_fit_animation(
        &mut self,
        from: crate::app::state::FitFrom,
    ) -> cosmic::Task<cosmic::Action<Message>> {
        let target_blend = self.settled_blend();
        let target_top = self.settled_top_ui_height();
        let target_bottom = self.settled_bottom_ui_height();
        let target_capture = self.settled_capture_area_height();
        let differs = (target_blend - from.blend).abs() > f32::EPSILON
            || (target_top - from.top_ui_height).abs() > f32::EPSILON
            || (target_bottom - from.bottom_ui_height).abs() > f32::EPSILON
            || (target_capture - from.capture_area_height).abs() > f32::EPSILON;
        if !differs {
            return cosmic::Task::none();
        }
        // The animation crosses the View-mode boundary whenever the source
        // or destination has zero capture-area height (View's signature).
        // Storing this explicitly means downstream rendering paths don't
        // have to infer it from a height comparison.
        let is_view_boundary =
            from.capture_area_height <= f32::EPSILON || target_capture <= f32::EPSILON;
        let was_idle = self.fit_animation.is_none();
        self.fit_animation = Some(crate::app::state::FitAnimation {
            start: std::time::Instant::now(),
            from,
            is_view_boundary,
        });
        if was_idle {
            Self::delay_task(16, Message::FitAnimationTick)
        } else {
            cosmic::Task::none()
        }
    }

    /// Build the main application view
    ///
    /// Composes all UI components into a layered layout with overlays.
    pub fn view(&self) -> Element<'_, Message> {
        static HAS_RENDERED: AtomicBool = AtomicBool::new(false);
        if !HAS_RENDERED.swap(true, Ordering::Relaxed) {
            info!("first UI render");
        }

        // Camera preview from camera_preview module
        let camera_preview = self.build_camera_preview();

        // Flash mode - show only preview with white overlay, no UI
        // Only show screen flash overlay for front cameras (back cameras use hardware LED)
        if self.flash.active && !self.use_hardware_flash() {
            let flash_overlay = widget::container(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::WHITE)),
                ..Default::default()
            });

            return widget::container(
                cosmic::iced::widget::stack![camera_preview, flash_overlay]
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme| widget::container::Style {
                background: Some(Background::Color(theme.cosmic().bg_color().into())),
                ..Default::default()
            })
            .into();
        }

        // Burst mode capture/processing - show progress overlay
        if self.burst_mode.is_active() {
            let burst_mode_overlay = self.build_burst_mode_overlay();
            return widget::container(
                cosmic::iced::widget::stack![camera_preview, burst_mode_overlay]
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme| widget::container::Style {
                background: Some(Background::Color(theme.cosmic().bg_color().into())),
                ..Default::default()
            })
            .into();
        }

        // Build top bar
        let top_bar = self.build_top_bar();

        // Zoom/fit row is shown in modes that allow manual zoom and the
        // fit-to-view toggle (Photo, View).
        let show_zoom_label = self.mode.supports_fit_and_zoom();

        // Capture button area - changes based on recording/streaming state and video file selection
        // Check if we have video file controls (play/pause button for video file sources)
        let play_pause_button = self.build_video_play_pause_button();
        let has_video_controls = play_pause_button.is_some();

        let capture_button_only = if (self.recording.is_recording()
            && !self.quick_record.is_recording())
            || self.virtual_camera.is_streaming()
        {
            // Mirror the bottom bar's three-column layout so the stop circle
            // sits where the carousel does and the photo button lines up with
            // the camera-switch position. `three_col_row` is the shared shape;
            // the side spacer width and center container width must match the
            // bottom bar's gallery/switch buttons and carousel width.
            let stop_circle = self.build_capture_circle();
            let photo_button = self.build_photo_during_recording_button();
            let slide = std::sync::Arc::clone(&self.carousel_button_slide);

            let spacing = cosmic::theme::spacing();
            let side_width = ui::PLACEHOLDER_BUTTON_WIDTH;
            let center_width = crate::app::bottom_bar::mode_carousel::carousel_width_for_modes(
                &self.available_modes(),
            );

            // While the virtual camera is streaming a video file source, keep
            // the play/pause control reachable in the left slot (it's hidden
            // by the streaming layout otherwise). For the camera-source
            // streaming case `play_pause_button` is `None` and we fall back
            // to the original spacer.
            let left_slot: Element<'_, Message> = if let Some(pp_button) = play_pause_button {
                widget::container(pp_button)
                    .width(Length::Fixed(side_width))
                    .center_x(side_width)
                    .into()
            } else {
                widget::Space::new()
                    .width(Length::Fixed(side_width))
                    .height(Length::Shrink)
                    .into()
            };

            // Vertical padding matches build_capture_button so the circle
            // doesn't shift when the layout flips between idle and recording.
            crate::app::bottom_bar::three_col_row(
                left_slot,
                widget::container(stop_circle)
                    .width(Length::Fixed(center_width))
                    .center_x(center_width)
                    .into(),
                SlideH::new(photo_button, slide, -1.0).into(),
                [spacing.space_xs, spacing.space_m],
            )
        } else if has_video_controls {
            // Video file selected but not streaming: show play button + capture button
            let capture_button = self.build_capture_button();
            let icon_button_width = crate::constants::ui::ICON_BUTTON_WIDTH;

            // Layout: [Fill] [Play container] [Capture] [Spacer matching Play] [Fill]
            // Use fixed-width container for play button to ensure centering
            let mut row = widget::Row::new().push(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink),
            );

            if let Some(pp_button) = play_pause_button {
                // Wrap play/pause button in fixed-width container for consistent centering
                row = row.push(
                    widget::container(pp_button)
                        .width(Length::Fixed(icon_button_width))
                        .align_x(cosmic::iced::alignment::Horizontal::Center),
                );
            }

            row = row
                .push(capture_button)
                // Spacer matches play/pause button width for centering
                .push(
                    widget::Space::new()
                        .width(Length::Fixed(icon_button_width))
                        .height(Length::Shrink),
                )
                .push(
                    widget::Space::new()
                        .width(Length::Fill)
                        .height(Length::Shrink),
                )
                .align_y(Alignment::Center)
                .width(Length::Fill);

            row.into()
        } else {
            // Normal single capture button
            self.build_capture_button()
        };

        // Capture button area (filter name label is now an overlay on the
        // preview). Wrap in a fixed-height container driven by the animated
        // `capture_area_height` so the slot collapses to 0 when entering
        // View and expands back when leaving — the fit/zoom row above
        // glides toward / away from the carousel. The capture button
        // itself, however, pops in/out instead of being gradually clipped:
        // we render an empty Space whenever a View transition is in flight
        // and only swap to the real button once it's at its settled height.
        let capture_h = self.capture_area_height();
        let view_transition_in_flight = self.fit_animation.is_some_and(|a| a.is_view_boundary);
        let inner: Element<'_, Message> = if self.mode.is_view_only() || view_transition_in_flight {
            widget::Space::new()
                .width(Length::Fill)
                .height(Length::Shrink)
                .into()
        } else {
            capture_button_only
        };
        let capture_button_area: Element<'_, Message> = widget::container(inner)
            .width(Length::Fill)
            .height(Length::Fixed(capture_h.max(0.0)))
            .clip(true)
            .into();

        // Bottom area: always show bottom bar (filter picker is now a sidebar overlay)
        let bottom_area: Element<'_, Message> = self.build_bottom_bar();

        // Immersive layout: camera preview fills the screen, all UI overlaid on top.
        // Aspect ratio crop is shown as translucent top/bottom bars (canvas overlay).
        let content: Element<'_, Message> = {
            let spacing = cosmic::theme::spacing();
            let control_spacing = spacing.space_xs;

            let mut bottom_controls = widget::Column::new().width(Length::Fill);

            if let Some(progress_bar) = self.build_video_progress_bar() {
                bottom_controls = bottom_controls.push(progress_bar);
            }

            bottom_controls = bottom_controls.push(capture_button_area).push(bottom_area);

            // Bottom section: zoom label + bottom controls
            let mut bottom_section = widget::Column::new().width(Length::Fill);

            // Hide the fit/zoom row while the tools menu is open so the two
            // don't visually compete — the menu itself is shown as an overlay.
            if show_zoom_label && !self.tools_menu_visible {
                let fit_icon_name = if self.preview_fit_to_view {
                    "view-fullscreen-symbolic"
                } else {
                    "view-restore-symbolic"
                };
                let fit_button_inner = widget::button::custom(
                    widget::Row::new()
                        .push(
                            widget::icon::from_name(fit_icon_name)
                                .symbolic(true)
                                .size(16),
                        )
                        .padding([0, spacing.space_s])
                        .height(Length::Fixed(spacing.space_l.into()))
                        .align_y(Alignment::Center),
                )
                .padding(0)
                .on_press(Message::TogglePreviewFit)
                .class(if self.preview_fit_to_view {
                    cosmic::theme::Button::Suggested
                } else {
                    overlay_chip_button_class()
                });
                // Inactive: frosted like the top/bottom bars so the button sits
                // on a matching surface. Active: keep the Suggested (accent)
                // fill so toggle state stays visible.
                let fit_button: Element<'_, Message> = if self.preview_fit_to_view {
                    fit_button_inner.into()
                } else {
                    self.frosted_panel(fit_button_inner.into(), OVERLAY_CONTAINER)
                };

                let zoom_row = widget::Row::new()
                    .push(fit_button)
                    .push(widget::space::horizontal().width(Length::Fixed(8.0)))
                    .push(self.build_zoom_label())
                    .align_y(Alignment::Center);

                bottom_section = bottom_section.push(
                    widget::container(zoom_row)
                        .width(Length::Fill)
                        .center_x(Length::Fill)
                        .padding([0, 0, control_spacing, 0]),
                );
            }

            bottom_section = bottom_section.push(bottom_controls);

            // The shader handles the Cover/Contain blend via cover_blend(), so
            // the preview always uses Cover layout (fills the window).  The shader
            // zooms out to show the full frame in Contain mode, with transparent
            // letterbox areas.
            let camera_layer: Element<'_, Message> = camera_preview;

            let mut main_stack = cosmic::iced::widget::stack![
                camera_layer,
                self.frosted_bars(),
                self.build_crop_overlay(),
                self.build_composition_overlay(),
                self.build_qr_overlay(),
                self.build_privacy_warning(),
                widget::container(top_bar)
                    .width(Length::Fill)
                    .align_y(cosmic::iced::alignment::Vertical::Top),
                widget::container(bottom_section)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(cosmic::iced::alignment::Vertical::Bottom)
            ];

            if self.flash.error_popup.is_some() {
                main_stack = main_stack.push(self.build_flash_error_popup());
            }

            if let Some(remaining) = self.photo_timer_countdown {
                main_stack = main_stack.push(self.build_timer_overlay(remaining));
            }

            main_stack.width(Length::Fill).height(Length::Fill).into()
        };

        // Wrap content in a stack so we can overlay the picker
        let mut main_stack = cosmic::iced::widget::stack![content];

        // Add format picker overlay if visible
        // Hide with libcamera backend in photo/video modes (resolution is handled automatically)
        if self.format_picker_visible && !self.is_format_picker_hidden() {
            main_stack = main_stack.push(self.build_format_picker());
        }

        // Add exposure picker overlay if visible
        if self.exposure_picker_visible {
            main_stack = main_stack.push(self.build_exposure_picker());
        }

        // Add color picker overlay if visible
        if self.color_picker_visible {
            main_stack = main_stack.push(self.build_color_picker());
        }

        // Add motor/PTZ controls picker overlay if visible
        if self.motor_picker_visible {
            main_stack = main_stack.push(self.build_motor_picker());
        }

        // Add tools menu overlay if visible
        if self.tools_menu_visible {
            main_stack = main_stack.push(self.build_tools_menu());
        }

        // Wrap everything in a themed background container
        widget::container(main_stack)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme| widget::container::Style {
                background: Some(Background::Color(theme.cosmic().bg_color().into())),
                ..Default::default()
            })
            .into()
    }

    /// Build the top bar with recording indicator and format button
    fn build_top_bar(&self) -> Element<'_, Message> {
        // View mode strips every top-bar button (and the title-bar window
        // controls) but keeps the draggable row so the user can still move
        // / double-click-to-maximize the window.
        if self.mode.is_view_only() {
            let empty = widget::container(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(TOP_BAR_HEIGHT)),
            )
            .width(Length::Fill)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            });
            return widget::mouse_area(empty)
                .on_drag(Message::WindowDrag)
                .on_double_press(Message::WindowToggleMaximize)
                .into();
        }

        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        // Match the native COSMIC header bar padding: [7, 7, 8, 7] (not maximized)
        let mut row = widget::Row::new()
            .padding([7, 7, 8, 7])
            .align_y(Alignment::Center);

        // Show recording indicator when recording (from controls module)
        if let Some(indicator) = self.build_recording_indicator() {
            row = row.push(indicator);
            row = row.push(widget::space::horizontal().width(spacing.space_s));
        }

        // Show streaming indicator when streaming virtual camera
        if let Some(indicator) = self.build_streaming_indicator() {
            row = row.push(indicator);
            row = row.push(widget::space::horizontal().width(spacing.space_s));
        }

        // Show timelapse indicator when timelapse is running
        if let Some(indicator) = self.build_timelapse_indicator() {
            row = row.push(indicator);
            row = row.push(widget::space::horizontal().width(spacing.space_s));
        }

        // Show format/resolution button in both photo and video modes
        // Hide button when:
        // - Format picker is visible
        // - Recording in video mode
        // - Streaming virtual camera (resolution cannot be changed during streaming)
        // - File source is set in Virtual mode (show file resolution instead)
        let has_file_source =
            self.mode == CameraMode::Virtual && self.virtual_camera_file_source.is_some();
        let show_format_button = !self.format_picker_visible
            && (self.mode == CameraMode::Photo
                || self.mode == CameraMode::Timelapse
                || !self.recording.is_recording())
            && !self.virtual_camera.is_streaming()
            && !has_file_source
            && !self.is_format_picker_hidden();

        if show_format_button {
            row = row.push(self.build_format_button());
        } else if has_file_source {
            // Show file source resolution (non-clickable)
            row = row.push(self.build_file_source_resolution_label());
        }

        // Right side buttons
        row = row.push(
            widget::Space::new()
                .width(Length::Fill)
                .height(Length::Shrink),
        );

        // Top-bar toggle buttons (flash, HDR, file, tools) are always
        // shown. Picker overlays appear on top of them but never replace them.
        // Flash toggle button (Photo mode, or Video/Timelapse mode with hardware flash for torch)
        if self.mode == CameraMode::Photo
            || ((self.mode == CameraMode::Video || self.mode == CameraMode::Timelapse)
                && self.use_hardware_flash())
        {
            let flash_icon_bytes = if self.flash.enabled {
                FLASH_ICON
            } else {
                FLASH_OFF_ICON
            };
            let flash_icon = widget::icon::from_svg_bytes(flash_icon_bytes).symbolic(true);

            if is_disabled {
                row = row.push(
                    widget::container(widget::icon(flash_icon).size(20))
                        .style(|_theme| widget::container::Style {
                            text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                            ..Default::default()
                        })
                        .padding([4, 8]),
                );
            } else {
                row = row.push(overlay_icon_button(
                    flash_icon,
                    Some(Message::ToggleFlash),
                    self.flash.enabled,
                ));
            }

            // 5px spacing
            row = row.push(
                widget::Space::new()
                    .width(Length::Fixed(5.0))
                    .height(Length::Shrink),
            );

            if self.should_show_burst_button() {
                // Show moon-off icon when HDR+ is disabled (by override or setting)
                let is_hdr_active = self.would_use_burst_mode();
                let moon_icon_bytes = if is_hdr_active {
                    MOON_ICON
                } else {
                    MOON_OFF_ICON
                };
                let moon_icon = widget::icon::from_svg_bytes(moon_icon_bytes).symbolic(true);

                if is_disabled {
                    row = row.push(
                        widget::container(widget::icon(moon_icon).size(20))
                            .style(|_theme| widget::container::Style {
                                text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                                ..Default::default()
                            })
                            .padding([4, 8]),
                    );
                } else {
                    row = row.push(overlay_icon_button(
                        moon_icon,
                        Some(Message::ToggleBurstMode),
                        is_hdr_active,
                    ));
                }

                // 5px spacing
                row = row.push(
                    widget::Space::new()
                        .width(Length::Fixed(5.0))
                        .height(Length::Shrink),
                );
            }
        }

        // File open button (only in Virtual mode, hidden when streaming)
        if self.mode == CameraMode::Virtual && !self.virtual_camera.is_streaming() {
            let has_file = self.virtual_camera_file_source.is_some();
            if is_disabled {
                let file_button =
                    widget::button::icon(icon::from_name("document-open-symbolic").symbolic(true));
                row = row.push(widget::container(file_button).style(|_theme| {
                    widget::container::Style {
                        text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                        ..Default::default()
                    }
                }));
            } else {
                let message = if has_file {
                    Message::ClearVirtualCameraFile
                } else {
                    Message::OpenVirtualCameraFile
                };
                row = row.push(overlay_icon_button(
                    icon::from_name("document-open-symbolic").symbolic(true),
                    Some(message),
                    has_file,
                ));
            }

            // 5px spacing
            row = row.push(
                widget::Space::new()
                    .width(Length::Fixed(5.0))
                    .height(Length::Shrink),
            );
        }

        // Tools menu button (opens overlay with timer, aspect ratio, exposure, filter, motor)
        // Highlight when tools menu is open or any tool setting is non-default
        let tools_active = self.tools_menu_visible || self.has_non_default_tool_settings();
        let tools_icon = widget::icon::from_svg_bytes(TOOLS_GRID_ICON).symbolic(true);

        if is_disabled {
            row = row.push(
                widget::container(widget::icon(tools_icon).size(20))
                    .style(|_theme| widget::container::Style {
                        text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                        ..Default::default()
                    })
                    .padding([4, 8]),
            );
        } else {
            row = row.push(overlay_icon_button(
                tools_icon,
                Some(Message::ToggleToolsMenu),
                tools_active,
            ));
        }

        // About and settings buttons (normally in header_end)
        if !is_disabled {
            row = row.push(
                widget::button::icon(icon::from_name("help-about-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::ToggleContextPage(
                        crate::app::state::ContextPage::About,
                    )),
            );
            row = row.push(
                widget::button::icon(icon::from_name("preferences-system-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::ToggleContextPage(
                        crate::app::state::ContextPage::Settings,
                    )),
            );
        }

        // Window control buttons
        row = row.push(
            widget::Space::new()
                .width(Length::Fixed(5.0))
                .height(Length::Shrink),
        );
        row = row
            .push(
                widget::button::icon(icon::from_name("window-minimize-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::WindowMinimize),
            )
            .push(
                widget::button::icon(icon::from_name("window-maximize-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::WindowToggleMaximize),
            )
            .push(
                widget::button::icon(icon::from_name("window-close-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::WindowClose),
            );

        let top_bar_widget =
            widget::container(row)
                .width(Length::Fill)
                .style(|_theme| widget::container::Style {
                    background: Some(Background::Color(Color::TRANSPARENT)),
                    ..Default::default()
                });

        // Make the top bar draggable for window movement
        widget::mouse_area(top_bar_widget)
            .on_drag(Message::WindowDrag)
            .on_double_press(Message::WindowToggleMaximize)
            .into()
    }

    /// Build the format button (resolution/FPS display)
    fn build_format_button(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        // Format label with superscript-style RES and FPS
        let (res_label, fps_label) = if let Some(fmt) = &self.active_format {
            let res = if fmt.width >= resolution_thresholds::THRESHOLD_4K {
                fl!("indicator-4k")
            } else if fmt.width >= resolution_thresholds::THRESHOLD_HD {
                fl!("indicator-hd")
            } else if fmt.width >= resolution_thresholds::THRESHOLD_720P {
                fl!("indicator-720p")
            } else {
                fl!("indicator-sd")
            };

            let fps = if let Some(fps) = fmt.framerate {
                fps.to_string()
            } else {
                ui::DEFAULT_FPS_DISPLAY.to_string()
            };

            (res, fps)
        } else {
            (fl!("indicator-hd"), ui::DEFAULT_FPS_DISPLAY.to_string())
        };

        // Create button with resolution^RES framerate^FPS layout
        let res_superscript =
            widget::container(widget::text(fl!("indicator-res")).size(ui::SUPERSCRIPT_TEXT_SIZE))
                .padding(ui::SUPERSCRIPT_PADDING);
        let fps_superscript =
            widget::container(widget::text(fl!("indicator-fps")).size(ui::SUPERSCRIPT_TEXT_SIZE))
                .padding(ui::SUPERSCRIPT_PADDING);

        let button_content = widget::Row::new()
            .push(widget::text(res_label).size(ui::RES_LABEL_TEXT_SIZE))
            .push(res_superscript)
            .push(widget::space::horizontal().width(spacing.space_xxs))
            .push(widget::text(fps_label).size(ui::RES_LABEL_TEXT_SIZE))
            .push(fps_superscript)
            .spacing(ui::RES_LABEL_SPACING)
            .align_y(Alignment::Center);

        let button = if is_disabled {
            widget::button::custom(button_content).class(cosmic::theme::Button::Text)
        } else {
            widget::button::custom(button_content)
                .on_press(Message::ToggleFormatPicker)
                .class(cosmic::theme::Button::Text)
        };

        // Wrap in container with themed semi-transparent background for visibility on camera preview
        widget::container(button)
            .style(move |theme| {
                let mut style = OVERLAY_CONTAINER.container_style(theme);
                if is_disabled {
                    style.text_color = Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3));
                }
                style
            })
            .into()
    }

    /// Build file source resolution label (non-clickable)
    ///
    /// Shows the resolution of the selected file source (image or video).
    /// Displayed instead of the format picker when a file source is selected.
    fn build_file_source_resolution_label(&self) -> Element<'_, Message> {
        // Get resolution from current_frame (which contains the file preview)
        let (width, height) = if let Some(ref frame) = self.current_frame {
            (frame.width, frame.height)
        } else {
            (0, 0)
        };

        // Show actual resolution (e.g., "1280×720")
        let dimensions = if width > 0 && height > 0 {
            format!("{}×{}", width, height)
        } else {
            "---".to_string()
        };

        let label_content = widget::Row::new()
            .push(
                widget::text(dimensions)
                    .size(ui::RES_LABEL_TEXT_SIZE)
                    .class(cosmic::theme::style::Text::Accent),
            )
            .align_y(Alignment::Center);

        // Non-clickable container with same styling as format button
        widget::container(label_content).padding([4, 8]).into()
    }

    /// Build zoom level button for display above capture button
    ///
    /// Shows current zoom level (1x, 1.3x, 2x, etc.) in Photo mode.
    /// Click to reset zoom to 1.0.
    fn build_zoom_label(&self) -> Element<'_, Message> {
        let zoom_text = if self.zoom_level >= 10.0 {
            "10x".to_string()
        } else if (self.zoom_level - self.zoom_level.round()).abs() < 0.05 {
            format!("{}x", self.zoom_level.round() as u32)
        } else {
            format!("{:.1}x", self.zoom_level)
        };

        let is_zoomed = (self.zoom_level - 1.0).abs() > 0.01;

        // Suggested (accent fill) when zoomed; otherwise a frosted Text button
        // so the resting background matches the top/bottom bars.
        let button = widget::button::text(zoom_text)
            .on_press(Message::ResetZoom)
            .class(if is_zoomed {
                cosmic::theme::Button::Suggested
            } else {
                overlay_chip_button_class()
            });
        if is_zoomed {
            button.into()
        } else {
            self.frosted_panel(button.into(), OVERLAY_CONTAINER)
        }
    }

    /// Build the QR code overlay layer
    ///
    /// This creates an overlay that shows detected QR codes with bounding boxes
    /// and action buttons. The overlay widget handles coordinate transformation
    /// at render time to correctly position elements over the video content.
    fn build_qr_overlay(&self) -> Element<'_, Message> {
        // Only show overlay if QR detection is enabled and we have detections
        if !self.qr_detection_enabled || self.qr_detections.is_empty() {
            return widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        // Get frame dimensions
        let Some(frame) = &self.current_frame else {
            return widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        };

        let should_mirror = self.should_mirror_preview();

        // Pass the animated `cover_blend` and UI bar heights so the QR
        // overlay tracks the live preview through a Photo↔fit-to-view
        // transition — without this the boxes are placed against the full
        // window in Contain mode where the video is actually letterboxed.
        build_qr_overlay(
            &self.qr_detections,
            frame.width,
            frame.height,
            self.cover_blend(),
            self.top_ui_height(),
            self.bottom_ui_height(),
            should_mirror,
        )
    }

    /// Build the tools menu overlay
    ///
    /// Shows timer, aspect ratio, exposure, filter buttons
    /// in a floating panel aligned to the top-right with large icon buttons in a 2-row grid.
    fn build_tools_menu(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let is_photo_mode = self.mode == CameraMode::Photo;

        // Collect all tool buttons for the grid
        let mut buttons: Vec<Element<'_, Message>> = Vec::new();

        // Timer button (Photo mode only)
        if is_photo_mode {
            let timer_active =
                self.photo_timer_setting != crate::app::state::PhotoTimerSetting::Off;
            let timer_icon_bytes = match self.photo_timer_setting {
                crate::app::state::PhotoTimerSetting::Off => TIMER_OFF_ICON,
                crate::app::state::PhotoTimerSetting::Sec3 => TIMER_3_ICON,
                crate::app::state::PhotoTimerSetting::Sec5 => TIMER_5_ICON,
                crate::app::state::PhotoTimerSetting::Sec10 => TIMER_10_ICON,
            };
            let timer_icon = widget::icon::from_svg_bytes(timer_icon_bytes).symbolic(true);
            buttons.push(self.build_tools_grid_button(
                timer_icon,
                fl!("tools-timer"),
                Message::CyclePhotoTimer,
                timer_active,
            ));

            // Aspect ratio button (Photo mode only). Square ratios (Native,
            // 1:1) are orientation-agnostic; the others swap to their
            // portrait companion icon when the window is taller than wide
            // so the label matches the rotated preview.
            let aspect_active = self.is_aspect_ratio_changed();
            let portrait = self.screen_is_portrait();
            let aspect_icon_bytes = match self.photo_aspect_ratio {
                crate::app::state::PhotoAspectRatio::Native => ASPECT_NATIVE_ICON,
                crate::app::state::PhotoAspectRatio::Ratio1x1 => ASPECT_1_1_ICON,
                crate::app::state::PhotoAspectRatio::Ratio4x3 if portrait => ASPECT_3_4_ICON,
                crate::app::state::PhotoAspectRatio::Ratio4x3 => ASPECT_4_3_ICON,
                crate::app::state::PhotoAspectRatio::Ratio16x9 if portrait => ASPECT_9_16_ICON,
                crate::app::state::PhotoAspectRatio::Ratio16x9 => ASPECT_16_9_ICON,
                crate::app::state::PhotoAspectRatio::Ratio2x1 if portrait => ASPECT_1_2_ICON,
                crate::app::state::PhotoAspectRatio::Ratio2x1 => ASPECT_2_1_ICON,
            };
            let aspect_icon = widget::icon::from_svg_bytes(aspect_icon_bytes).symbolic(true);
            buttons.push(self.build_tools_grid_button(
                aspect_icon,
                fl!("tools-aspect"),
                Message::CyclePhotoAspectRatio,
                aspect_active,
            ));
        }

        // Exposure button
        if self.available_exposure_controls.has_any_essential() {
            let exposure_icon = widget::icon::from_svg_bytes(EXPOSURE_ICON).symbolic(true);
            buttons.push(self.build_tools_grid_button(
                exposure_icon,
                fl!("tools-exposure"),
                Message::ToggleExposurePicker,
                self.is_exposure_changed(),
            ));
        }

        // Color button (for contrast, saturation, white balance, etc.)
        if self.available_exposure_controls.has_any_image_controls()
            || self.available_exposure_controls.has_any_white_balance()
        {
            buttons.push(self.build_tools_grid_button(
                icon::from_name("applications-graphics-symbolic").symbolic(true),
                fl!("tools-color"),
                Message::ToggleColorPicker,
                self.is_color_changed(),
            ));
        }

        // Filter button (photo, video, timelapse, and virtual-camera modes)
        if self.mode == CameraMode::Photo
            || self.mode == CameraMode::Video
            || self.mode == CameraMode::Timelapse
            || self.mode == CameraMode::Virtual
        {
            let filter_active = self.selected_filter != FilterType::Standard;
            buttons.push(self.build_tools_grid_button(
                widget::icon::from_svg_bytes(FILTER_ICON).symbolic(true),
                fl!("tools-filter"),
                Message::ToggleContextPage(crate::app::state::ContextPage::Filters),
                filter_active,
            ));
        }

        // Motor/PTZ button (shows when camera has motor controls)
        if self.has_motor_controls() {
            buttons.push(self.build_tools_grid_button(
                widget::icon::from_svg_bytes(CAMERA_TILT_ICON).symbolic(true),
                fl!("tools-motor"),
                Message::ToggleMotorPicker,
                self.motor_picker_visible,
            ));
        }

        // Distribute buttons into 2 rows
        let items_per_row = buttons.len().div_ceil(2); // Ceiling division
        let mut rows: Vec<Element<'_, Message>> = Vec::new();
        let mut current_row: Vec<Element<'_, Message>> = Vec::new();

        for (i, button) in buttons.into_iter().enumerate() {
            current_row.push(button);
            if current_row.len() >= items_per_row || i == items_per_row * 2 - 1 {
                let row = widget::row::with_children(std::mem::take(&mut current_row))
                    .spacing(spacing.space_s)
                    .align_y(Alignment::Start);
                rows.push(row.into());
            }
        }
        if !current_row.is_empty() {
            let row = widget::row::with_children(current_row)
                .spacing(spacing.space_s)
                .align_y(Alignment::Start);
            rows.push(row.into());
        }

        // Build column from rows
        let column = widget::column::with_children(rows)
            .spacing(spacing.space_s)
            .padding(spacing.space_s);

        // Build panel with semi-transparent themed background
        let panel = self.frosted_panel(column.into(), PICKER_PANEL);

        // Position in top-right corner, below the custom title bar so the menu
        // doesn't overlap the window controls.
        let positioned = widget::Row::new()
            .push(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink),
            )
            .push(panel)
            .padding([
                TOP_BAR_HEIGHT as u16 + spacing.space_xs,
                spacing.space_xs,
                0,
                spacing.space_xs,
            ]);

        widget::mouse_area(
            widget::container(positioned)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::CloseToolsMenu)
        .into()
    }

    /// Build a grid button with large icon and text label below (outside the button)
    fn build_tools_grid_button<'a>(
        &self,
        icon_handle: impl Into<widget::icon::Handle>,
        label: String,
        message: Message,
        is_active: bool,
    ) -> Element<'a, Message> {
        self.build_tools_grid_button_with_enabled(icon_handle, label, message, is_active, true)
    }

    /// Build a grid button with large icon and text label below, with optional enabled state
    fn build_tools_grid_button_with_enabled<'a>(
        &self,
        icon_handle: impl Into<widget::icon::Handle>,
        label: String,
        message: Message,
        is_active: bool,
        enabled: bool,
    ) -> Element<'a, Message> {
        // Icon button with appropriate styling
        let mut button = widget::button::custom(widget::icon(icon_handle.into()).size(32))
            .class(if is_active {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Text
            })
            .padding(12);

        // Only add on_press handler if enabled
        if enabled {
            button = button.on_press(message);
        }

        // Wrap inactive buttons in a container with visible background
        let button_element: Element<'_, Message> = if is_active {
            button.into()
        } else {
            widget::container(button)
                .style(OVERLAY_CONTAINER.style())
                .into()
        };

        // Button with label below
        widget::Column::new()
            .push(button_element)
            .push(widget::text(label).size(11))
            .spacing(4)
            .align_x(Alignment::Center)
            .into()
    }

    /// Check if any tool settings are non-default (for highlighting tools button).
    /// Photo-only settings (timer, aspect ratio) are only counted while the
    /// app is in Photo mode — they don't take effect elsewhere, so they
    /// shouldn't drive the highlight in Video / Timelapse / Virtual.
    fn has_non_default_tool_settings(&self) -> bool {
        let in_photo = self.mode == CameraMode::Photo;
        let timer_active =
            in_photo && self.photo_timer_setting != crate::app::state::PhotoTimerSetting::Off;
        let aspect_active = in_photo && self.is_aspect_ratio_changed();
        let exposure_active = self.is_exposure_changed();
        let color_active = self.is_color_changed();
        let filter_active = self.selected_filter != FilterType::Standard;

        timer_active || aspect_active || exposure_active || color_active || filter_active
    }

    /// Check if aspect ratio is cropped (not using native ratio)
    fn is_aspect_ratio_changed(&self) -> bool {
        self.photo_aspect_ratio != crate::app::state::PhotoAspectRatio::Native
    }

    /// Check if exposure settings differ from defaults
    fn is_exposure_changed(&self) -> bool {
        let controls = &self.available_exposure_controls;
        self.exposure_settings
            .as_ref()
            .map(|s| {
                let mode_changed = controls.has_exposure_auto
                    && s.mode != crate::app::exposure_picker::ExposureMode::AperturePriority;
                let ev_changed = controls.exposure_bias.available
                    && s.exposure_compensation != controls.exposure_bias.default;
                let backlight_changed = controls.backlight_compensation.available
                    && s.backlight_compensation
                        .map(|v| v != controls.backlight_compensation.default)
                        .unwrap_or(false);
                mode_changed || ev_changed || backlight_changed
            })
            .unwrap_or(false)
    }

    /// Check if color settings differ from defaults
    fn is_color_changed(&self) -> bool {
        let controls = &self.available_exposure_controls;
        self.color_settings
            .as_ref()
            .map(|s| {
                let image_changed = (controls.contrast.available
                    && s.contrast
                        .map(|v| v != controls.contrast.default)
                        .unwrap_or(false))
                    || (controls.saturation.available
                        && s.saturation
                            .map(|v| v != controls.saturation.default)
                            .unwrap_or(false))
                    || (controls.sharpness.available
                        && s.sharpness
                            .map(|v| v != controls.sharpness.default)
                            .unwrap_or(false))
                    || (controls.hue.available
                        && s.hue.map(|v| v != controls.hue.default).unwrap_or(false));
                let wb_auto_off = controls.has_white_balance_auto
                    && s.white_balance_auto.map(|v| !v).unwrap_or(false);
                image_changed || wb_auto_off
            })
            .unwrap_or(false)
    }

    /// Build the privacy cover warning overlay
    ///
    /// Shows a centered warning when the camera's privacy cover is closed.
    fn build_privacy_warning(&self) -> Element<'_, Message> {
        if !self.privacy_cover_closed {
            return widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        build_overlay_popup(
            self,
            widget::text("\u{26A0}").size(48).into(),
            &fl!("privacy-cover-closed"),
            &fl!("privacy-cover-hint"),
            None,
        )
    }

    /// Build the burst mode progress overlay
    ///
    /// Shows status text, frame count, and progress bar during burst mode capture/processing.
    fn build_burst_mode_overlay(&self) -> Element<'_, Message> {
        let (status_text, detail_text) = match self.burst_mode.stage {
            BurstModeStage::Capturing => (
                fl!("burst-mode-hold-steady"),
                fl!(
                    "burst-mode-frames",
                    captured = self.burst_mode.frames_captured(),
                    total = self.burst_mode.target_frame_count
                ),
            ),
            BurstModeStage::Processing => (fl!("burst-mode-processing"), String::new()),
            _ => (String::new(), String::new()),
        };

        // Progress percentage
        let progress_percent = (self.burst_mode.progress() * 100.0) as u32;

        // Build progress bar (simple filled bar)
        let progress_width = BURST_MODE_PROGRESS_BAR_WIDTH;
        let progress_height = BURST_MODE_PROGRESS_BAR_HEIGHT;
        let filled_width = progress_width * self.burst_mode.progress();

        let progress_bar = widget::container(
            widget::Row::new()
                .push(
                    widget::container(
                        widget::Space::new()
                            .width(Length::Fixed(filled_width))
                            .height(Length::Fixed(progress_height)),
                    )
                    .style(|theme: &cosmic::Theme| {
                        let accent = theme.cosmic().accent_color();
                        widget::container::Style {
                            background: Some(Background::Color(Color::from_rgb(
                                accent.red,
                                accent.green,
                                accent.blue,
                            ))),
                            ..Default::default()
                        }
                    }),
                )
                .push(
                    widget::container(
                        widget::Space::new()
                            .width(Length::Fixed(progress_width - filled_width))
                            .height(Length::Fixed(progress_height)),
                    )
                    .style(|_theme| widget::container::Style {
                        background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.3))),
                        ..Default::default()
                    }),
                ),
        )
        .style(|theme: &cosmic::Theme| widget::container::Style {
            border: cosmic::iced::Border {
                radius: theme.cosmic().corner_radii.radius_xs.into(),
                ..Default::default()
            },
            ..Default::default()
        });

        // Build the overlay content
        let overlay_content = widget::Column::new()
            .push(
                widget::text(status_text)
                    .size(32)
                    .font(cosmic::font::bold()),
            )
            .push(
                widget::Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(8.0)),
            )
            .push(widget::text(detail_text).size(18))
            .push(
                widget::Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(16.0)),
            )
            .push(progress_bar)
            .push(
                widget::Space::new()
                    .width(Length::Shrink)
                    .height(Length::Fixed(8.0)),
            )
            .push(widget::text(format!("{}%", progress_percent)).size(14))
            .align_x(Alignment::Center);

        // Semi-transparent background panel
        let overlay_panel = self.frosted_panel(
            widget::container(overlay_content).padding(24).into(),
            POPUP_PANEL,
        );

        widget::container(overlay_panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
    }

    /// Build the flash permission error popup dialog
    ///
    /// Shows a centered modal with warning icon, error message, and OK button
    /// when flash hardware was detected but cannot be controlled.
    fn build_flash_error_popup(&self) -> Element<'_, Message> {
        let error_msg = self
            .flash
            .error_popup
            .as_deref()
            .unwrap_or("Flash permission error");

        build_overlay_popup(
            self,
            widget::text("\u{26A0}").size(48).into(),
            "Flash Permission Error",
            error_msg,
            Some(
                widget::button::suggested("OK")
                    .on_press(Message::DismissFlashError)
                    .into(),
            ),
        )
    }

    /// Build the timer countdown overlay
    ///
    /// Shows large countdown number with fade effect during photo timer countdown.
    fn build_timer_overlay(&self, remaining: u8) -> Element<'_, Message> {
        // Calculate fade opacity based on elapsed time since tick start
        // Opacity starts at 1.0 and fades to 0.0 over the second
        let opacity = if let Some(tick_start) = self.photo_timer_tick_start {
            let elapsed_ms = tick_start.elapsed().as_millis() as f32;
            // Fade out over 900ms (leave 100ms fully transparent before next number)
            (1.0 - (elapsed_ms / 900.0)).max(0.0)
        } else {
            1.0
        };

        // Large countdown number with fade effect
        let countdown_text = widget::container(
            widget::text(remaining.to_string())
                .size(400) // Very large to fill preview
                .font(cosmic::font::bold()),
        )
        .style(move |_theme| widget::container::Style {
            text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, opacity)),
            ..Default::default()
        });

        widget::container(countdown_text)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
    }
}
