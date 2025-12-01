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
use crate::app::state::{AppModel, CameraMode, FilterType, Message};
use crate::app::video_widget::VideoContentFit;
use crate::constants::{resolution_thresholds, ui};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget::{self, icon};
use tracing::info;

/// Flash icon SVG (lightning bolt)
const FLASH_ICON: &[u8] = include_bytes!("../../resources/button_icons/flash.svg");
/// Flash off icon SVG (lightning bolt with strike-through)
const FLASH_OFF_ICON: &[u8] = include_bytes!("../../resources/button_icons/flash-off.svg");

impl AppModel {
    /// Check if filters are available in the current mode (Photo or Virtual)
    fn filters_available(&self) -> bool {
        self.mode == CameraMode::Photo || self.mode == CameraMode::Virtual
    }

    /// Build the main application view
    ///
    /// Composes all UI components into a layered layout with overlays.
    pub fn view(&self) -> Element<'_, Message> {
        let _spacing = cosmic::theme::spacing();

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
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::BLACK)),
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

        // Check if filter name label should be shown (only when a non-standard filter is selected)
        let show_filter_label =
            self.filters_available() && self.selected_filter != FilterType::Standard;

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
            info!(
                "Building theatre mode layout (UI visible: {})",
                self.theatre.ui_visible
            );

            if self.theatre.ui_visible {
                // Theatre mode with UI visible - overlay all UI on top of preview
                // Use same layout structure as normal mode to prevent position jumps

                // Bottom controls: filter label + capture button + bottom area in a column
                // Filter label is added first (above capture button) with same 8px padding as normal mode
                let mut bottom_controls = widget::column().width(Length::Fill);

                // Add filter name label above capture button (same 8px margin as normal mode)
                if show_filter_label {
                    bottom_controls = bottom_controls.push(
                        widget::container(self.build_filter_name_label())
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
                // Theatre mode with UI hidden - show only full-screen preview with QR overlay
                cosmic::iced::widget::stack![camera_preview, self.build_qr_overlay()]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        } else {
            // Normal mode - traditional layout
            // Preview with top bar, QR overlay, and optional filter name label overlaid
            let mut preview_stack = cosmic::iced::widget::stack![
                camera_preview,
                // QR overlay (custom widget calculates positions at render time)
                self.build_qr_overlay(),
                widget::container(top_bar)
                    .width(Length::Fill)
                    .align_y(cosmic::iced::alignment::Vertical::Top)
            ];

            // Add filter name label overlapping bottom of preview (centered above capture button)
            if show_filter_label {
                preview_stack = preview_stack.push(
                    widget::container(self.build_filter_name_label())
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

        // Wrap everything in a black background container
        widget::container(main_stack)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::BLACK)),
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

        // Photo and Virtual mode buttons: Flash and filter (filter for both, flash only for Photo)
        if self.filters_available() {
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
                    row = row.push(
                        widget::button::icon(flash_icon)
                            .on_press(Message::ToggleFlash)
                            .class(if self.flash_enabled {
                                cosmic::theme::Button::Suggested
                            } else {
                                cosmic::theme::Button::Standard
                            }),
                    );
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // File open button (only in Virtual mode, hidden when streaming)
            // - When no file is selected: opens file picker
            // - When file is selected: clears file source and switches back to camera
            if self.mode == CameraMode::Virtual && !self.virtual_camera.is_streaming() {
                let has_file = self.virtual_camera_file_source.is_some();
                if is_disabled {
                    let file_button =
                        widget::button::icon(icon::from_name("document-open-symbolic"));
                    row = row.push(widget::container(file_button).style(|_theme| {
                        widget::container::Style {
                            text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                            ..Default::default()
                        }
                    }));
                } else {
                    // Toggle behavior: open file picker or clear file source
                    let message = if has_file {
                        Message::ClearVirtualCameraFile
                    } else {
                        Message::OpenVirtualCameraFile
                    };
                    row = row.push(
                        widget::button::icon(icon::from_name("document-open-symbolic"))
                            .on_press(message)
                            .class(if has_file {
                                cosmic::theme::Button::Suggested
                            } else {
                                cosmic::theme::Button::Standard
                            }),
                    );
                }

                // 5px spacing
                row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
            }

            // Filter picker button (available in Photo and Virtual modes)
            if is_disabled {
                let filter_button = widget::button::icon(icon::from_name("image-filter-symbolic"));
                row = row.push(widget::container(filter_button).style(|_theme| {
                    widget::container::Style {
                        text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                        ..Default::default()
                    }
                }));
            } else {
                // Highlight only when a non-standard filter is active
                let is_highlighted = self.selected_filter != FilterType::Standard;
                row = row.push(
                    widget::button::icon(icon::from_name("image-filter-symbolic"))
                        .on_press(Message::ToggleContextPage(
                            crate::app::state::ContextPage::Filters,
                        ))
                        .class(if is_highlighted {
                            cosmic::theme::Button::Suggested
                        } else {
                            cosmic::theme::Button::Standard
                        }),
                );
            }

            // 5px spacing before theatre button
            row = row.push(widget::Space::new(Length::Fixed(5.0), Length::Shrink));
        }

        // Theatre mode button
        if is_disabled {
            let theatre_button = widget::button::icon(icon::from_name("view-fullscreen-symbolic"));
            row = row.push(widget::container(theatre_button).style(|_theme| {
                widget::container::Style {
                    text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                    ..Default::default()
                }
            }));
        } else {
            let theatre_icon = if self.theatre.enabled {
                "view-restore-symbolic"
            } else {
                "view-fullscreen-symbolic"
            };
            row = row.push(
                widget::button::icon(icon::from_name(theatre_icon))
                    .on_press(Message::ToggleTheatreMode)
                    .class(if self.theatre.enabled {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Standard
                    }),
            );
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

        if is_disabled {
            widget::container(button)
                .style(|_theme| widget::container::Style {
                    text_color: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                    ..Default::default()
                })
                .into()
        } else {
            button.into()
        }
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

    /// Build filter name label styled like the mode buttons
    ///
    /// Used to display the current filter name when filter picker is open.
    /// Styled like a Suggested button but not clickable.
    fn build_filter_name_label(&self) -> Element<'_, Message> {
        let filter_name = self.selected_filter_name();
        // Use text button style with Suggested class (like active mode button)
        // No on_press makes it non-clickable
        widget::button::text(filter_name)
            .class(cosmic::theme::Button::Suggested)
            .into()
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

        build_qr_overlay(
            &self.qr_detections,
            frame.width,
            frame.height,
            content_fit,
            self.config.mirror_preview,
        )
    }
}
