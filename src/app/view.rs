// SPDX-License-Identifier: GPL-3.0-only

//! Main application view
//!
//! This module composes the main UI from modularized components:
//! - Camera preview (camera_preview module)
//! - Top bar with format picker (inline)
//! - Capture button (controls module)
//! - Bottom bar (bottom_bar module)
//! - Format picker overlay (format_picker module)

use crate::app::qr_overlay::build_qr_overlay;
use crate::app::state::{AppModel, BurstModeStage, CameraMode, FilterType, Message};
use crate::app::video_widget::VideoContentFit;
use crate::constants::resolution_thresholds;
use crate::constants::ui::{self, OVERLAY_BACKGROUND_ALPHA};
use crate::fl;
use crate::shaders::depth::{DEPTH_MAX_MM_U16, DEPTH_MIN_MM_U16};
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget::{self, icon};
use tracing::debug;

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
/// Aspect ratio 4:3 icon SVG
const ASPECT_4_3_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-4-3.svg");
/// Aspect ratio 16:9 icon SVG
const ASPECT_16_9_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-16-9.svg");
/// Aspect ratio 1:1 icon SVG
const ASPECT_1_1_ICON: &[u8] = include_bytes!("../../resources/button_icons/aspect-1-1.svg");
/// Exposure icon SVG
const EXPOSURE_ICON: &[u8] = include_bytes!("../../resources/button_icons/exposure.svg");
const TOOLS_GRID_ICON: &[u8] = include_bytes!("../../resources/button_icons/tools-grid.svg");
/// Moon icon SVG (burst mode)
const MOON_ICON: &[u8] = include_bytes!("../../resources/button_icons/moon.svg");
/// Moon off icon SVG (burst mode disabled, with strike-through)
const MOON_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/moon-off.svg");
/// Depth visualization icon SVG (for depth cameras like Kinect)
const DEPTH_ICON: &[u8] = include_bytes!("../../resources/button_icons/depth-waves.svg");
/// Depth visualization off icon SVG
const DEPTH_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/depth-waves-off.svg");
/// Camera tilt/motor control icon SVG
const CAMERA_TILT_ICON: &[u8] = include_bytes!("../../resources/button_icons/camera-tilt.svg");
/// Mesh view icon SVG (triangulated surface)
const MESH_ICON: &[u8] = include_bytes!("../../resources/button_icons/mesh.svg");
/// Points view icon SVG (point cloud)
const POINTS_ICON: &[u8] = include_bytes!("../../resources/button_icons/points.svg");

/// Burst mode progress bar dimensions
const BURST_MODE_PROGRESS_BAR_WIDTH: f32 = 200.0;
const BURST_MODE_PROGRESS_BAR_HEIGHT: f32 = 8.0;

/// Create a container style with semi-transparent themed background for overlay elements
///
/// Uses `radius_xl` to match COSMIC button corner radius (follows round/slightly round/square theme setting)
/// Does not set text_color to allow buttons to use their native COSMIC theme colors.
pub fn overlay_container_style(theme: &cosmic::Theme) -> widget::container::Style {
    let cosmic = theme.cosmic();
    let bg = cosmic.bg_color();
    widget::container::Style {
        background: Some(Background::Color(Color::from_rgba(
            bg.red,
            bg.green,
            bg.blue,
            OVERLAY_BACKGROUND_ALPHA,
        ))),
        border: cosmic::iced::Border {
            // Use radius_xl to match COSMIC button styling
            radius: cosmic.corner_radii.radius_xl.into(),
            ..Default::default()
        },
        // Don't set text_color - let buttons use their native COSMIC theme colors
        ..Default::default()
    }
}

/// Create an icon button with a themed background for use on camera preview overlays
fn overlay_icon_button<'a, M: Clone + 'static>(
    handle: impl Into<widget::icon::Handle>,
    message: Option<M>,
    highlighted: bool,
) -> Element<'a, M> {
    // Create icon widget that inherits theme colors
    let icon_widget = widget::icon(handle.into()).size(20);

    // Use custom button with icon as content - this allows icon to inherit theme colors
    // Use Suggested for active state, Text for inactive (transparent background)
    let mut button = widget::button::custom(icon_widget)
        .padding(8)
        .class(if highlighted {
            cosmic::theme::Button::Suggested
        } else {
            cosmic::theme::Button::Text
        });

    if let Some(msg) = message {
        button = button.on_press(msg);
    }

    // Wrap in container with themed background for better visibility on camera preview
    widget::container(button)
        .style(overlay_container_style)
        .into()
}

impl AppModel {
    /// Build the main application view
    ///
    /// Composes all UI components into a layered layout with overlays.
    pub fn view(&self) -> Element<'_, Message> {
        // Camera preview from camera_preview module
        let camera_preview = self.build_camera_preview();

        // Flash mode - show only preview with white overlay, no UI
        if self.flash_active {
            let flash_overlay = widget::container(widget::Space::new(Length::Fill, Length::Fill))
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

        // Timer countdown mode - show only preview with countdown overlay and capture button
        if let Some(remaining) = self.photo_timer_countdown {
            let countdown_overlay = self.build_timer_overlay(remaining);

            // Capture button (acts as abort during countdown)
            let capture_button = self.build_capture_button();
            let capture_area = widget::container(capture_button)
                .width(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center);

            let content = widget::column()
                .push(
                    cosmic::iced::widget::stack![camera_preview, countdown_overlay]
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .push(capture_area)
                .width(Length::Fill)
                .height(Length::Fill);

            return widget::container(content)
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

        // Wrap preview in mouse area for theatre mode interactions
        let camera_preview = if self.theatre.enabled {
            // In theatre mode, show UI on click or mouse movement
            widget::mouse_area(camera_preview)
                .on_press(Message::TheatreShowUI)
                .on_move(|_| Message::TheatreShowUI)
                .into()
        } else {
            camera_preview
        };

        // Check if zoom label should be shown (Photo mode or 3D preview mode)
        let show_zoom_label = self.mode == CameraMode::Photo || self.preview_3d.enabled;

        // Capture button area - changes based on recording/streaming state and video file selection
        // Check if we have video file controls (play/pause button for video file sources)
        let has_video_controls = self.build_video_play_pause_button().is_some();

        let capture_button_only =
            if self.recording.is_recording() || self.virtual_camera.is_streaming() {
                // When recording/streaming: stop button centered, photo button to its right
                // For video file sources: also show play/pause button to the left
                let stop_button = self.build_capture_button();
                let photo_button = self.build_photo_during_recording_button();
                let play_pause_button = self.build_video_play_pause_button();

                // Calculate button width for layout balancing
                let button_width = crate::constants::ui::CAPTURE_BUTTON_OUTER;

                // Layout depends on whether we have a play/pause button
                // With play/pause: [Fill] [Play/Pause] [Stop] [Photo] [Fill]
                // Without: [Fill] [Spacer] [Stop] [Photo] [Fill]
                let mut row = widget::row().push(widget::Space::new(Length::Fill, Length::Shrink));

                if let Some(pp_button) = play_pause_button {
                    // Add play/pause button to the left of stop button
                    row = row.push(pp_button);
                } else {
                    // Add spacer to balance the photo button on the right
                    row = row.push(widget::Space::new(
                        Length::Fixed(button_width),
                        Length::Shrink,
                    ));
                }

                row = row
                    .push(stop_button)
                    .push(photo_button)
                    .push(widget::Space::new(Length::Fill, Length::Shrink))
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                row.into()
            } else if has_video_controls {
                // Video file selected but not streaming: show play button + capture button
                let capture_button = self.build_capture_button();
                let play_pause_button = self.build_video_play_pause_button();
                let icon_button_width = crate::constants::ui::ICON_BUTTON_WIDTH;

                // Layout: [Fill] [Play container] [Capture] [Spacer matching Play] [Fill]
                // Use fixed-width container for play button to ensure centering
                let mut row = widget::row().push(widget::Space::new(Length::Fill, Length::Shrink));

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
                    .push(widget::Space::new(
                        Length::Fixed(icon_button_width),
                        Length::Shrink,
                    ))
                    .push(widget::Space::new(Length::Fill, Length::Shrink))
                    .align_y(Alignment::Center)
                    .width(Length::Fill);

                row.into()
            } else {
                // Normal single capture button
                self.build_capture_button()
            };

        // Capture button area (filter name label is now an overlay on the preview)
        let capture_button_area: Element<'_, Message> = capture_button_only;

        // Bottom area: always show bottom bar (filter picker is now a sidebar overlay)
        let bottom_area: Element<'_, Message> = self.build_bottom_bar();

        // Build content based on theatre mode
        let content: Element<'_, Message> = if self.theatre.enabled {
            // Theatre mode - camera preview as full background with UI overlaid
            debug!(
                "Building theatre mode layout (UI visible: {})",
                self.theatre.ui_visible
            );

            if self.theatre.ui_visible {
                // Theatre mode with UI visible - overlay all UI on top of preview
                // Use same layout structure as normal mode to prevent position jumps

                // Bottom controls: zoom label + capture button + bottom area in a column
                // Zoom label is added first (above capture button) with same 8px padding as normal mode
                let mut bottom_controls = widget::column().width(Length::Fill);

                // Add zoom label above capture button (same 8px margin as normal mode)
                if show_zoom_label {
                    bottom_controls = bottom_controls.push(
                        widget::container(self.build_zoom_label())
                            .width(Length::Fill)
                            .center_x(Length::Fill)
                            .padding([0, 0, 8, 0]),
                    );
                }

                // Add video progress bar between preview and capture button (if streaming video)
                if let Some(progress_bar) = self.build_video_progress_bar() {
                    bottom_controls = bottom_controls.push(progress_bar);
                }

                bottom_controls = bottom_controls.push(capture_button_area).push(bottom_area);

                let theatre_stack = cosmic::iced::widget::stack![
                    camera_preview,
                    // QR overlay (custom widget calculates positions at render time)
                    self.build_qr_overlay(),
                    // Depth legend overlay (bottom right, when depth overlay enabled)
                    self.build_depth_legend(),
                    // Privacy cover warning overlay (centered)
                    self.build_privacy_warning(),
                    // Top bar aligned to top (no extra padding - row has its own padding)
                    widget::container(top_bar)
                        .width(Length::Fill)
                        .align_y(cosmic::iced::alignment::Vertical::Top),
                    // Bottom controls aligned to bottom
                    widget::container(bottom_controls)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_y(cosmic::iced::alignment::Vertical::Bottom)
                ];

                theatre_stack
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                // Theatre mode with UI hidden - show only full-screen preview with QR overlay and privacy warning
                cosmic::iced::widget::stack![
                    camera_preview,
                    self.build_qr_overlay(),
                    self.build_depth_legend(),
                    self.build_privacy_warning()
                ]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
        } else {
            // Normal mode - traditional layout
            // Preview with top bar, QR overlay, privacy warning, and optional filter name label overlaid
            let mut preview_stack = cosmic::iced::widget::stack![
                camera_preview,
                // QR overlay (custom widget calculates positions at render time)
                self.build_qr_overlay(),
                // Depth legend overlay (bottom right, when depth overlay enabled)
                self.build_depth_legend(),
                // Privacy cover warning overlay (centered)
                self.build_privacy_warning(),
                widget::container(top_bar)
                    .width(Length::Fill)
                    .align_y(cosmic::iced::alignment::Vertical::Top)
            ];

            // Add zoom label overlapping bottom of preview (centered above capture button)
            if show_zoom_label {
                preview_stack = preview_stack.push(
                    widget::container(self.build_zoom_label())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(cosmic::iced::alignment::Horizontal::Center)
                        .align_y(cosmic::iced::alignment::Vertical::Bottom)
                        .padding([0, 0, 8, 0]),
                );
            }

            let preview_with_overlays = preview_stack.width(Length::Fill).height(Length::Fill);

            // Column layout: preview with overlays, optional progress bar, capture button area, bottom area
            let mut main_column = widget::column()
                .push(preview_with_overlays)
                .width(Length::Fill)
                .height(Length::Fill);

            // Add video progress bar between preview and capture button (if streaming video)
            if let Some(progress_bar) = self.build_video_progress_bar() {
                main_column = main_column.push(progress_bar);
            }

            main_column = main_column.push(capture_button_area).push(bottom_area);

            main_column.into()
        };

        // Wrap content in a stack so we can overlay the picker
        let mut main_stack = cosmic::iced::widget::stack![content];

        // Add iOS-style format picker overlay if visible
        if self.format_picker_visible {
            main_stack = main_stack.push(self.build_format_picker());
        }

        // Add iOS-style exposure picker overlay if visible
        if self.exposure_picker_visible {
            main_stack = main_stack.push(self.build_exposure_picker());
        }

        // Add iOS-style color picker overlay if visible
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

        // Add calibration dialog overlay if visible
        if self.kinect.calibration_dialog_visible {
            main_stack = main_stack.push(self.build_calibration_dialog());
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
        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        let mut row = widget::row()
            .padding(spacing.space_xs)
            .align_y(Alignment::Center);

        // Show recording indicator when recording (from controls module)
        if let Some(indicator) = self.build_recording_indicator() {
            row = row.push(indicator);
            row = row.push(widget::horizontal_space().width(spacing.space_s));
        }

        // Show streaming indicator when streaming virtual camera
        if let Some(indicator) = self.build_streaming_indicator() {
            row = row.push(indicator);
            row = row.push(widget::horizontal_space().width(spacing.space_s));
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
            && (self.mode == CameraMode::Photo || !self.recording.is_recording())
            && !self.virtual_camera.is_streaming()
            && !has_file_source;

        if show_format_button {
            row = row.push(self.build_format_button());
        } else if has_file_source {
            // Show file source resolution (non-clickable)
            row = row.push(self.build_file_source_resolution_label());
        }

        // Right side buttons
        row = row.push(widget::Space::new(Length::Fill, Length::Shrink));

        // Hide flash and tools buttons when any picker/menu is open
        let hide_top_bar_buttons = self.tools_menu_visible
            || self.exposure_picker_visible
            || self.color_picker_visible
            || self.motor_picker_visible;

        if !hide_top_bar_buttons {
            // Flash toggle button (only in Photo mode)
            if self.mode == CameraMode::Photo {
                let flash_icon_bytes = if self.flash_enabled {
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
                        self.flash_enabled,
                    ));
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));

                // Burst mode toggle button
                // - Toggles HDR+ between Auto and Off
                // - Shows moon-off icon with strike-through when Off
                // - Highlighted when burst mode would actually be used (based on scene brightness)
                let is_hdr_off = !self.config.burst_mode_setting.is_enabled();
                let moon_icon_bytes = if is_hdr_off { MOON_OFF_ICON } else { MOON_ICON };
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
                    // Highlight when burst mode would actually be triggered
                    let would_burst = self.would_use_burst_mode();
                    row = row.push(overlay_icon_button(
                        moon_icon,
                        Some(Message::ToggleBurstMode),
                        would_burst,
                    ));
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // File open button (only in Virtual mode, hidden when streaming)
            if self.mode == CameraMode::Virtual && !self.virtual_camera.is_streaming() {
                let has_file = self.virtual_camera_file_source.is_some();
                if is_disabled {
                    let file_button = widget::button::icon(
                        icon::from_name("document-open-symbolic").symbolic(true),
                    );
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
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // Motor/PTZ control button (shows when camera has motor controls)
            if self.has_motor_controls() {
                let motor_icon = widget::icon::from_svg_bytes(CAMERA_TILT_ICON).symbolic(true);

                if is_disabled {
                    row = row.push(
                        widget::container(widget::icon(motor_icon.clone()).size(20))
                            .style(|_theme| widget::container::Style {
                                text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                                ..Default::default()
                            })
                            .padding([4, 8]),
                    );
                } else {
                    row = row.push(overlay_icon_button(
                        motor_icon,
                        Some(Message::ToggleMotorPicker),
                        self.motor_picker_visible,
                    ));
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // Depth visualization toggle (shows when camera has depth data)
            let has_depth = self
                .current_frame
                .as_ref()
                .map(|f| f.depth_data.is_some())
                .unwrap_or(false);
            if has_depth {
                let depth_icon_bytes = if self.depth_viz.overlay_enabled {
                    DEPTH_ICON
                } else {
                    DEPTH_OFF_ICON
                };
                let depth_icon = widget::icon::from_svg_bytes(depth_icon_bytes).symbolic(true);

                if is_disabled {
                    row = row.push(
                        widget::container(widget::icon(depth_icon).size(20))
                            .style(|_theme| widget::container::Style {
                                text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                                ..Default::default()
                            })
                            .padding([4, 8]),
                    );
                } else {
                    row = row.push(overlay_icon_button(
                        depth_icon,
                        Some(Message::ToggleDepthOverlay),
                        self.depth_viz.overlay_enabled,
                    ));
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // Scene view mode toggle (only in Scene mode)
            // Switches between point cloud and mesh rendering
            if self.mode == CameraMode::Scene && self.preview_3d.enabled {
                use crate::app::state::SceneViewMode;
                let is_mesh = self.preview_3d.view_mode == SceneViewMode::Mesh;
                let view_icon_bytes = if is_mesh { MESH_ICON } else { POINTS_ICON };
                let view_icon = widget::icon::from_svg_bytes(view_icon_bytes).symbolic(true);

                if is_disabled {
                    row = row.push(
                        widget::container(widget::icon(view_icon).size(20))
                            .style(|_theme| widget::container::Style {
                                text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                                ..Default::default()
                            })
                            .padding([4, 8]),
                    );
                } else {
                    row = row.push(overlay_icon_button(
                        view_icon,
                        Some(Message::ToggleSceneViewMode),
                        is_mesh,
                    ));
                }
            }

            // 5px spacing
            row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));

            // Tools menu button (opens overlay with timer, aspect ratio, exposure, filter, theatre)
            // Highlight when tools menu is open or any tool setting is non-default
            let tools_active = self.tools_menu_visible || self.has_non_default_tool_settings();
            let tools_icon = widget::icon::from_svg_bytes(TOOLS_GRID_ICON).symbolic(true);

            if is_disabled {
                row = row.push(
                    widget::container(widget::icon(tools_icon.into()).size(20))
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
        }

        widget::container(row)
            .width(Length::Fill)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            })
            .into()
    }

    /// Build the format button (iOS-style resolution/FPS display)
    fn build_format_button(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();
        let is_disabled = self.transition_state.ui_disabled;

        // Format iOS-style label with superscript-style RES and FPS
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

        let button_content = widget::row()
            .push(widget::text(res_label).size(ui::RES_LABEL_TEXT_SIZE))
            .push(res_superscript)
            .push(widget::horizontal_space().width(spacing.space_xxs))
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
                let mut style = overlay_container_style(theme);
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

        let label_content = widget::row()
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
    /// Shows current zoom level (1x, 1.3x, 2x, etc.) in Photo mode or 3D preview mode.
    /// In 3D mode, also shows a rotation reset button.
    /// Click zoom to reset to 1.0.
    fn build_zoom_label(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        if self.preview_3d.enabled {
            // 3D preview mode: "fly into scene" zoom model
            // preview_3d_zoom = 0 means camera at sensor (1x view)
            // Positive values = camera moved into scene = zoomed in
            // Negative values = camera moved back = zoomed out
            let display_zoom = if self.preview_3d.zoom >= 0.0 {
                1.0 + self.preview_3d.zoom // 0=1x, 1=2x, 2=3x
            } else {
                1.0 / (1.0 - self.preview_3d.zoom) // -1=0.5x, -2=0.33x
            };
            let zoom_text = if display_zoom >= 4.0 {
                format!("{:.0}x", display_zoom)
            } else if display_zoom < 1.0 {
                format!("{:.1}x", display_zoom)
            } else if (display_zoom - display_zoom.round()).abs() < 0.05 {
                format!("{}x", display_zoom.round() as u32)
            } else {
                format!("{:.1}x", display_zoom)
            };

            let is_zoomed = self.preview_3d.zoom.abs() > 0.01;
            let (pitch, yaw) = self.preview_3d.rotation;
            let is_rotated = pitch.abs() > 0.01 || yaw.abs() > 0.01;

            // Zoom reset button
            let zoom_button = widget::button::text(zoom_text)
                .on_press(Message::Reset3DPreviewRotation)
                .class(if is_zoomed {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                });

            // Rotation reset button (shows when rotated)
            let reset_button = widget::button::text("⟲")
                .on_press(Message::Reset3DPreviewRotation)
                .class(if is_rotated {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                });

            widget::row()
                .push(reset_button)
                .push(zoom_button)
                .spacing(spacing.space_xxs)
                .align_y(Alignment::Center)
                .into()
        } else {
            // Normal Photo mode zoom
            let zoom_text = if self.zoom_level >= 10.0 {
                "10x".to_string()
            } else if (self.zoom_level - self.zoom_level.round()).abs() < 0.05 {
                format!("{}x", self.zoom_level.round() as u32)
            } else {
                format!("{:.1}x", self.zoom_level)
            };

            let is_zoomed = (self.zoom_level - 1.0).abs() > 0.01;

            // Use text button style - Suggested when zoomed, Standard when at 1x
            widget::button::text(zoom_text)
                .on_press(Message::ResetZoom)
                .class(if is_zoomed {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Standard
                })
                .into()
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
            return widget::Space::new(Length::Fill, Length::Fill).into();
        }

        // Get frame dimensions
        let Some(frame) = &self.current_frame else {
            return widget::Space::new(Length::Fill, Length::Fill).into();
        };

        // Determine content fit mode based on theatre state
        let content_fit = if self.theatre.enabled {
            VideoContentFit::Cover
        } else {
            VideoContentFit::Contain
        };

        // File sources should never be mirrored - match the video widget behavior
        let should_mirror = self.config.mirror_preview && !self.current_frame_is_file_source;

        build_qr_overlay(
            &self.qr_detections,
            frame.width,
            frame.height,
            content_fit,
            should_mirror,
        )
    }

    /// Build the tools menu overlay
    ///
    /// Shows timer, aspect ratio, exposure, filter, and theatre mode buttons
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

            // Aspect ratio button (Photo mode only, disabled in theatre mode)
            // Theatre mode always uses native resolution, so aspect ratio control is disabled
            let aspect_active = self.is_aspect_ratio_changed();
            let aspect_enabled = !self.theatre.enabled;
            let native_ratio = self.current_frame.as_ref().and_then(|f| {
                crate::app::state::PhotoAspectRatio::from_frame_dimensions(f.width, f.height)
            });
            // In theatre mode, always show native icon since aspect ratio is ignored
            let effective_ratio = if self.theatre.enabled {
                crate::app::state::PhotoAspectRatio::Native
            } else if self.photo_aspect_ratio == crate::app::state::PhotoAspectRatio::Native {
                native_ratio.unwrap_or(crate::app::state::PhotoAspectRatio::Native)
            } else {
                self.photo_aspect_ratio
            };
            let aspect_icon_bytes = match effective_ratio {
                crate::app::state::PhotoAspectRatio::Native => ASPECT_NATIVE_ICON,
                crate::app::state::PhotoAspectRatio::Ratio4x3 => ASPECT_4_3_ICON,
                crate::app::state::PhotoAspectRatio::Ratio16x9 => ASPECT_16_9_ICON,
                crate::app::state::PhotoAspectRatio::Ratio1x1 => ASPECT_1_1_ICON,
            };
            let aspect_icon = widget::icon::from_svg_bytes(aspect_icon_bytes).symbolic(true);
            buttons.push(self.build_tools_grid_button_with_enabled(
                aspect_icon,
                fl!("tools-aspect"),
                Message::CyclePhotoAspectRatio,
                aspect_active && aspect_enabled, // Only show as active if enabled and changed
                aspect_enabled,
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

        // Filter button
        let filter_active = self.selected_filter != FilterType::Standard;
        buttons.push(self.build_tools_grid_button(
            icon::from_name("image-filter-symbolic").symbolic(true),
            fl!("tools-filter"),
            Message::ToggleContextPage(crate::app::state::ContextPage::Filters),
            filter_active,
        ));

        // Theatre mode button
        let theatre_icon = if self.theatre.enabled {
            "view-restore-symbolic"
        } else {
            "view-fullscreen-symbolic"
        };
        buttons.push(self.build_tools_grid_button(
            icon::from_name(theatre_icon).symbolic(true),
            fl!("tools-theatre"),
            Message::ToggleTheatreMode,
            self.theatre.enabled,
        ));

        // Distribute buttons into 2 rows
        let items_per_row = (buttons.len() + 1) / 2; // Ceiling division
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
        let panel = widget::container(column).style(|theme: &cosmic::Theme| {
            let cosmic = theme.cosmic();
            let bg = cosmic.bg_color();
            widget::container::Style {
                background: Some(Background::Color(Color::from_rgba(
                    bg.red,
                    bg.green,
                    bg.blue,
                    OVERLAY_BACKGROUND_ALPHA,
                ))),
                border: cosmic::iced::Border {
                    radius: cosmic.corner_radii.radius_s.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });

        // Position in top-right corner (space first pushes panel to right)
        let positioned = widget::row()
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .push(panel)
            .padding([spacing.space_xs, spacing.space_xs, 0, spacing.space_xs]);

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
                .style(overlay_container_style)
                .into()
        };

        // Button with label below
        widget::column()
            .push(button_element)
            .push(widget::text(label).size(11))
            .spacing(4)
            .align_x(Alignment::Center)
            .into()
    }

    /// Check if any tool settings are non-default (for highlighting tools button)
    fn has_non_default_tool_settings(&self) -> bool {
        let timer_active = self.photo_timer_setting != crate::app::state::PhotoTimerSetting::Off;
        let aspect_active = self.is_aspect_ratio_changed();
        let exposure_active = self.is_exposure_changed();
        let color_active = self.is_color_changed();
        let filter_active = self.selected_filter != FilterType::Standard;
        let theatre_active = self.theatre.enabled;

        timer_active
            || aspect_active
            || exposure_active
            || color_active
            || filter_active
            || theatre_active
    }

    /// Check if aspect ratio is cropped (not using native ratio)
    fn is_aspect_ratio_changed(&self) -> bool {
        let (frame_width, frame_height) = self
            .current_frame
            .as_ref()
            .map(|f| (f.width, f.height))
            .unwrap_or((0, 0));
        let has_frame = frame_width > 0 && frame_height > 0;
        let native_ratio =
            crate::app::state::PhotoAspectRatio::from_frame_dimensions(frame_width, frame_height);
        has_frame
            && match (self.photo_aspect_ratio, native_ratio) {
                (crate::app::state::PhotoAspectRatio::Native, _) => false,
                (selected, Some(native)) => selected != native,
                (_, None) => true,
            }
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
            return widget::Space::new(Length::Fill, Length::Fill).into();
        }

        let spacing = cosmic::theme::spacing();

        // Warning icon and text
        let warning_content = widget::column()
            .push(
                widget::icon(
                    icon::from_name("dialog-warning-symbolic")
                        .symbolic(true)
                        .into(),
                )
                .size(48),
            )
            .push(
                widget::text(fl!("privacy-cover-closed"))
                    .size(20)
                    .font(cosmic::font::bold()),
            )
            .push(widget::text(fl!("privacy-cover-hint")).size(14))
            .spacing(spacing.space_s)
            .align_x(Alignment::Center);

        // Container with semi-transparent background
        let warning_box = widget::container(warning_content)
            .padding(spacing.space_m)
            .style(|theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                let bg = cosmic.bg_color();
                widget::container::Style {
                    background: Some(Background::Color(Color::from_rgba(
                        bg.red,
                        bg.green,
                        bg.blue,
                        OVERLAY_BACKGROUND_ALPHA,
                    ))),
                    border: cosmic::iced::Border {
                        radius: cosmic.corner_radii.radius_m.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        // Center the warning in the preview area
        widget::container(warning_box)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
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
            widget::row()
                .push(
                    widget::container(widget::Space::new(
                        Length::Fixed(filled_width),
                        Length::Fixed(progress_height),
                    ))
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
                    widget::container(widget::Space::new(
                        Length::Fixed(progress_width - filled_width),
                        Length::Fixed(progress_height),
                    ))
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
        let overlay_content = widget::column()
            .push(
                widget::text(status_text)
                    .size(32)
                    .font(cosmic::font::bold()),
            )
            .push(widget::Space::new(Length::Shrink, Length::Fixed(8.0)))
            .push(widget::text(detail_text).size(18))
            .push(widget::Space::new(Length::Shrink, Length::Fixed(16.0)))
            .push(progress_bar)
            .push(widget::Space::new(Length::Shrink, Length::Fixed(8.0)))
            .push(widget::text(format!("{}%", progress_percent)).size(14))
            .align_x(Alignment::Center);

        // Semi-transparent background panel
        let overlay_panel =
            widget::container(overlay_content)
                .padding(24)
                .style(|theme: &cosmic::Theme| {
                    let cosmic = theme.cosmic();
                    let bg = cosmic.bg_color();
                    widget::container::Style {
                        background: Some(Background::Color(Color::from_rgba(
                            bg.red, bg.green, bg.blue, 0.85,
                        ))),
                        border: cosmic::iced::Border {
                            radius: cosmic.corner_radii.radius_m.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });

        widget::container(overlay_panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center)
            .into()
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

    /// Build the depth legend overlay
    ///
    /// Shows a horizontal gradient bar with depth values in mm.
    /// Uses the turbo colormap (blue=near to red=far).
    /// Positioned in bottom right, limited to capture button height.
    fn build_depth_legend(&self) -> Element<'_, Message> {
        // Only show when depth overlay is enabled and we have depth data
        if !self.depth_viz.overlay_enabled {
            return widget::Space::new(Length::Fill, Length::Fill).into();
        }

        let has_depth = self
            .current_frame
            .as_ref()
            .map(|f| f.depth_data.is_some())
            .unwrap_or(false);

        if !has_depth {
            return widget::Space::new(Length::Fill, Length::Fill).into();
        }

        let spacing = cosmic::theme::spacing();

        // Kinect depth range from shared constants
        let min_depth_mm = DEPTH_MIN_MM_U16;
        let max_depth_mm = DEPTH_MAX_MM_U16;

        // Build the legend content: gradient bar with labels
        // Height limited to capture button size (60px)
        let legend_height = ui::CAPTURE_BUTTON_OUTER;
        let bar_height = 12.0_f32;
        let label_size = 10_u16;

        // Create gradient bar using a row of colored segments
        let num_segments = 40;
        let segment_width = 4.0_f32;
        let mut gradient_row = widget::row().spacing(0);

        // Turbo colormap gradient stops (used when not in grayscale mode)
        let turbo_colors: [(f32, Color); 7] = [
            (0.0, Color::from_rgb(0.18995, 0.07176, 0.23217)), // Dark blue/purple
            (0.17, Color::from_rgb(0.12178, 0.38550, 0.90354)), // Blue
            (0.33, Color::from_rgb(0.09859, 0.70942, 0.66632)), // Cyan
            (0.5, Color::from_rgb(0.50000, 0.85810, 0.27671)), // Green/yellow
            (0.67, Color::from_rgb(0.91567, 0.85024, 0.09695)), // Yellow
            (0.83, Color::from_rgb(0.99214, 0.50000, 0.07763)), // Orange
            (1.0, Color::from_rgb(0.72340, 0.10000, 0.08125)), // Red
        ];

        let use_grayscale = self.depth_viz.grayscale_mode;

        for i in 0..num_segments {
            let t = i as f32 / (num_segments - 1) as f32;

            // Use grayscale or turbo colormap based on mode
            // Grayscale: near (t=0) = bright, far (t=1) = dark (matches shader)
            let color = if use_grayscale {
                let gray = 1.0 - t; // Invert: near=bright, far=dark
                Color::from_rgb(gray, gray, gray)
            } else {
                Self::interpolate_turbo(t, &turbo_colors)
            };

            gradient_row = gradient_row.push(
                widget::container(widget::Space::new(
                    Length::Fixed(segment_width),
                    Length::Fixed(bar_height),
                ))
                .style(move |_| widget::container::Style {
                    background: Some(Background::Color(color)),
                    ..Default::default()
                }),
            );
        }

        // Labels row: Near (blue) on left, Far (red) on right
        // Matches turbo colormap: t=0 (left) = blue = near, t=1 (right) = red = far
        let labels_row = widget::row()
            .push(widget::text::caption(format!("{}mm", min_depth_mm)).size(label_size))
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .push(widget::text::caption(format!("{}mm", max_depth_mm)).size(label_size))
            .width(Length::Fixed(segment_width * num_segments as f32));

        // Toggle button for grayscale mode
        let grayscale_toggle = widget::row()
            .push(
                widget::checkbox("Grayscale", self.depth_viz.grayscale_mode)
                    .on_toggle(|_| Message::ToggleDepthGrayscale)
                    .size(14)
                    .text_size(label_size),
            )
            .width(Length::Fixed(segment_width * num_segments as f32));

        // Combine into a column: labels on top, gradient bar, then grayscale toggle
        let legend_content = widget::column()
            .push(labels_row)
            .push(gradient_row)
            .push(grayscale_toggle)
            .spacing(2)
            .align_x(cosmic::iced::Alignment::Center);

        // Semi-transparent container for the legend
        let legend_container = widget::container(legend_content)
            .padding([spacing.space_xxs, spacing.space_xs])
            .style(|theme: &cosmic::Theme| {
                let cosmic = theme.cosmic();
                let bg = cosmic.bg_color();
                widget::container::Style {
                    background: Some(Background::Color(Color::from_rgba(
                        bg.red,
                        bg.green,
                        bg.blue,
                        OVERLAY_BACKGROUND_ALPHA,
                    ))),
                    border: cosmic::iced::Border {
                        radius: cosmic.corner_radii.radius_s.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });

        // Position in bottom right corner
        // Bottom padding: space_s (bottom bar margin) + capture button height + xxs gap
        let bottom_padding = spacing.space_s as f32 + legend_height + spacing.space_xxs as f32;

        widget::container(
            widget::row()
                .push(widget::Space::new(Length::Fill, Length::Shrink))
                .push(legend_container),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(cosmic::iced::alignment::Vertical::Bottom)
        .padding([0.0, spacing.space_s as f32, bottom_padding, 0.0])
        .into()
    }

    /// Build the calibration dialog overlay
    ///
    /// Shows calibration status and prompts user to calibrate if using defaults.
    fn build_calibration_dialog(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Get calibration info
        let (status_text, detail_text, has_device_calibration) =
            if let Some(ref calib) = self.kinect.calibration_info {
                if calib.from_device {
                    (
                        "Device Calibration Active",
                        format!(
                            "Factory calibration loaded from device EEPROM.\n\n\
                             Reference distance: {:.0}mm\n\
                             IR-RGB baseline: {:.1}mm",
                            calib.reference_distance_mm, calib.stereo_baseline_mm
                        ),
                        true,
                    )
                } else {
                    (
                        "Using Default Calibration",
                        "Factory calibration could not be read from the device.\n\
                         Depth-to-color alignment may be inaccurate.\n\n\
                         This can happen when:\n\
                         • USB permission issues (try running as root)\n\
                         • Device not fully initialized\n\
                         • Using V4L2 mode instead of native Kinect\n\n\
                         Try restarting the camera or switching to native mode."
                            .to_string(),
                        false,
                    )
                }
            } else {
                (
                    "No Calibration Data",
                    "No depth camera is currently active.".to_string(),
                    false,
                )
            };

        // Icon - check mark for device calibration, warning for default
        let status_icon = if has_device_calibration {
            icon::from_name("emblem-ok-symbolic").symbolic(true)
        } else {
            icon::from_name("dialog-warning-symbolic").symbolic(true)
        };

        // Build dialog content
        let content = widget::column()
            .push(
                widget::icon(status_icon.into())
                    .size(48)
                    .class(if has_device_calibration {
                        cosmic::theme::Svg::Default
                    } else {
                        cosmic::theme::Svg::Custom(std::rc::Rc::new(|theme: &cosmic::Theme| {
                            cosmic::widget::svg::Style {
                                color: Some(theme.cosmic().destructive_color().into()),
                            }
                        }))
                    }),
            )
            .push(
                widget::text(status_text)
                    .size(20)
                    .font(cosmic::font::bold()),
            )
            .push(widget::text(detail_text).size(14))
            .push(widget::Space::new(Length::Shrink, Length::Fixed(16.0)))
            .push(
                widget::button::text("Close")
                    .on_press(Message::CloseCalibrationDialog)
                    .class(cosmic::theme::Button::Standard),
            )
            .spacing(spacing.space_s)
            .align_x(Alignment::Center);

        // Dialog container with semi-transparent background
        let dialog_box =
            widget::container(content)
                .padding(spacing.space_m)
                .style(|theme: &cosmic::Theme| {
                    let cosmic = theme.cosmic();
                    let bg = cosmic.bg_color();
                    widget::container::Style {
                        background: Some(Background::Color(Color::from_rgba(
                            bg.red, bg.green, bg.blue, 0.95,
                        ))),
                        border: cosmic::iced::Border {
                            radius: cosmic.corner_radii.radius_m.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                });

        // Dark overlay behind dialog to dim the background
        let overlay = widget::mouse_area(
            widget::container(dialog_box)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center)
                .align_y(cosmic::iced::alignment::Vertical::Center),
        )
        .on_press(Message::CloseCalibrationDialog);

        widget::container(overlay)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))),
                ..Default::default()
            })
            .into()
    }

    /// Interpolate turbo colormap from pre-defined color stops
    fn interpolate_turbo(t: f32, colors: &[(f32, Color); 7]) -> Color {
        let t = t.clamp(0.0, 1.0);

        // Find the two color stops to interpolate between
        let mut i = 0;
        while i < colors.len() - 1 && colors[i + 1].0 < t {
            i += 1;
        }

        if i >= colors.len() - 1 {
            return colors[colors.len() - 1].1;
        }

        let (t0, c0) = colors[i];
        let (t1, c1) = colors[i + 1];

        // Linear interpolation between the two stops
        let factor = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };

        Color::from_rgb(
            c0.r + (c1.r - c0.r) * factor,
            c0.g + (c1.g - c0.g) * factor,
            c0.b + (c1.b - c0.b) * factor,
        )
    }
}
