// SPDX-License-Identifier: GPL-3.0-only

//! Settings drawer view

use crate::app::state::{AppModel, Message};
use crate::constants::{BitratePreset, ResolutionTier, app_info, format_bitrate};
use crate::fl;
use cosmic::Element;
use cosmic::app::context_drawer;
use cosmic::iced::Length;
use cosmic::widget;

impl AppModel {
    /// Create the settings view for the context drawer
    ///
    /// Shows camera selection, format options, and backend settings.
    pub fn settings_view(&self) -> context_drawer::ContextDrawer<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Camera selection dropdown
        let camera_dropdown = widget::dropdown(
            &self.camera_dropdown_options,
            Some(self.current_camera_index),
            Message::SelectCamera,
        );

        // Audio device selection dropdown
        let audio_dropdown = widget::dropdown(
            &self.audio_dropdown_options,
            Some(self.current_audio_device_index),
            Message::SelectAudioDevice,
        );

        // Video encoder selection dropdown
        let video_encoder_dropdown = widget::dropdown(
            &self.video_encoder_dropdown_options,
            Some(self.current_video_encoder_index),
            Message::SelectVideoEncoder,
        );

        // Bitrate preset dropdown
        let current_bitrate_index = BitratePreset::ALL
            .iter()
            .position(|p| *p == self.config.bitrate_preset)
            .unwrap_or(1); // Default to Medium (index 1)

        let bitrate_preset_dropdown = widget::dropdown(
            &self.bitrate_preset_dropdown_options,
            Some(current_bitrate_index),
            Message::SelectBitratePreset,
        );

        // Info button for bitrate matrix
        let info_icon = widget::icon::from_name(if self.bitrate_info_visible {
            "help-info-symbolic"
        } else {
            "help-about-symbolic"
        })
        .size(16);

        let info_button = widget::button::icon(info_icon)
            .on_press(Message::ToggleBitrateInfo)
            .class(cosmic::theme::Button::Icon);

        // Mode dropdown (consolidated format selector)
        let current_mode_index = if let Some(active) = &self.active_format {
            self.mode_list.iter().position(|f| {
                f.width == active.width
                    && f.height == active.height
                    && f.framerate == active.framerate
                    && f.pixel_format == active.pixel_format
            })
        } else {
            None
        };

        let mode_dropdown = widget::dropdown(
            &self.mode_dropdown_options,
            current_mode_index,
            Message::SelectMode,
        );

        // Bug report buttons
        let bug_report_button =
            widget::button::standard(fl!("settings-report-bug")).on_press(Message::GenerateBugReport);

        // Show report button (only if a report was generated)
        let bug_report_row = if self.last_bug_report_path.is_some() {
            let show_report_button =
                widget::button::standard(fl!("settings-show-report")).on_press(Message::ShowBugReport);

            widget::row()
                .push(bug_report_button)
                .push(widget::horizontal_space().width(spacing.space_xs))
                .push(show_report_button)
                .spacing(0)
        } else {
            widget::row().push(bug_report_button).spacing(0)
        };

        // Mirror preview toggle
        let mirror_toggle =
            widget::toggler(self.config.mirror_preview).on_toggle(|_| Message::ToggleMirrorPreview);

        // Virtual camera toggle
        let virtual_camera_toggle = widget::toggler(self.config.virtual_camera_enabled)
            .on_toggle(|_| Message::ToggleVirtualCameraEnabled);

        // Version info string
        let version_info = if app_info::is_flatpak() {
            fl!("settings-version-flatpak", version = app_info::version())
        } else {
            fl!("settings-version", version = app_info::version())
        };

        // Build settings column
        let settings_column: Element<'_, Message> = widget::column()
            .push(widget::text(fl!("settings-camera")).size(16).font(cosmic::font::bold()))
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(camera_dropdown)
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(fl!("settings-microphone"))
                    .size(16)
                    .font(cosmic::font::bold()),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(audio_dropdown)
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(fl!("settings-video-encoder"))
                    .size(16)
                    .font(cosmic::font::bold()),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(video_encoder_dropdown)
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::row()
                    .push(
                        widget::text(fl!("settings-video-quality"))
                            .size(16)
                            .font(cosmic::font::bold()),
                    )
                    .push(widget::horizontal_space().width(Length::Fixed(8.0)))
                    .push(info_button)
                    .align_y(cosmic::iced::Alignment::Center),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(bitrate_preset_dropdown)
            .push(self.build_bitrate_info_matrix(spacing.space_xxs))
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(fl!("settings-manual-override"))
                    .size(16)
                    .font(cosmic::font::bold()),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(mode_dropdown)
            .push(widget::vertical_space().height(spacing.space_l))
            .push(widget::divider::horizontal::default())
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::row()
                    .push(
                        widget::text(fl!("settings-mirror-preview"))
                            .size(16)
                            .font(cosmic::font::bold()),
                    )
                    .push(widget::horizontal_space().width(cosmic::iced::Length::Fill))
                    .push(mirror_toggle)
                    .align_y(cosmic::iced::Alignment::Center),
            )
            .push(widget::vertical_space().height(spacing.space_l))
            .push(widget::divider::horizontal::default())
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(fl!("virtual-camera-title"))
                    .size(16)
                    .font(cosmic::font::bold()),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(widget::text(fl!("virtual-camera-description")).size(12))
            .push(widget::vertical_space().height(spacing.space_xs))
            .push(
                widget::row()
                    .push(widget::text(fl!("virtual-camera-enable")).size(14))
                    .push(widget::horizontal_space().width(cosmic::iced::Length::Fill))
                    .push(virtual_camera_toggle)
                    .align_y(cosmic::iced::Alignment::Center),
            )
            .push(widget::vertical_space().height(spacing.space_l))
            .push(widget::divider::horizontal::default())
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(fl!("settings-bug-reports"))
                    .size(16)
                    .font(cosmic::font::bold()),
            )
            .push(widget::vertical_space().height(spacing.space_xxs))
            .push(bug_report_row)
            .push(widget::vertical_space().height(spacing.space_l))
            .push(widget::divider::horizontal::default())
            .push(widget::vertical_space().height(spacing.space_s))
            .push(
                widget::text(version_info)
                    .size(12)
                    .class(cosmic::theme::Text::Accent),
            )
            .spacing(0)
            .into();

        context_drawer::context_drawer(
            settings_column,
            Message::ToggleContextPage(crate::app::state::ContextPage::Settings),
        )
        .title(fl!("settings-title"))
    }

    /// Build the bitrate info matrix table (shown when info button is toggled)
    fn build_bitrate_info_matrix(&self, vertical_spacing: u16) -> Element<'_, Message> {
        if !self.bitrate_info_visible {
            return widget::vertical_space().height(Length::Fixed(0.0)).into();
        }

        // Build the matrix table
        let mut table_column = widget::column()
            .push(widget::vertical_space().height(vertical_spacing))
            .spacing(4);

        // Header row
        let header_row = widget::row()
            .push(
                widget::container(
                    widget::text(fl!("settings-resolution"))
                        .size(12)
                        .font(cosmic::font::bold()),
                )
                .width(Length::Fixed(70.0)),
            )
            .push(
                widget::container(widget::text(fl!("preset-low")).size(12).font(cosmic::font::bold()))
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
            )
            .push(
                widget::container(widget::text(fl!("preset-medium")).size(12).font(cosmic::font::bold()))
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
            )
            .push(
                widget::container(widget::text(fl!("preset-high")).size(12).font(cosmic::font::bold()))
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
            )
            .spacing(4);

        table_column = table_column.push(header_row);

        // Data rows for each resolution tier
        for tier in ResolutionTier::ALL.iter() {
            let row = widget::row()
                .push(
                    widget::container(widget::text(tier.display_name()).size(11))
                        .width(Length::Fixed(70.0)),
                )
                .push(
                    widget::container(
                        widget::text(format_bitrate(BitratePreset::Low.bitrate_for_tier(*tier)))
                            .size(11),
                    )
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
                )
                .push(
                    widget::container(
                        widget::text(format_bitrate(
                            BitratePreset::Medium.bitrate_for_tier(*tier),
                        ))
                        .size(11),
                    )
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
                )
                .push(
                    widget::container(
                        widget::text(format_bitrate(BitratePreset::High.bitrate_for_tier(*tier)))
                            .size(11),
                    )
                    .width(Length::Fixed(65.0))
                    .center_x(65.0),
                )
                .spacing(4);

            table_column = table_column.push(row);
        }

        // Wrap in a container with subtle background
        widget::container(table_column)
            .padding(8)
            .class(cosmic::theme::Container::Card)
            .into()
    }
}
