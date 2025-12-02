// SPDX-License-Identifier: GPL-3.0-only

//! Settings drawer view

use crate::app::state::{AppModel, Message};
use crate::constants::{BitratePreset, ResolutionTier, format_bitrate};
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

        // Bitrate preset index
        let current_bitrate_index = BitratePreset::ALL
            .iter()
            .position(|p| *p == self.config.bitrate_preset)
            .unwrap_or(1); // Default to Medium (index 1)

        // Camera section
        let camera_section = widget::settings::section()
            .title(fl!("settings-camera"))
            .add(
                widget::settings::item::builder(fl!("settings-device")).control(widget::dropdown(
                    &self.camera_dropdown_options,
                    Some(self.current_camera_index),
                    Message::SelectCamera,
                )),
            )
            .add(
                widget::settings::item::builder(fl!("settings-format")).control(widget::dropdown(
                    &self.mode_dropdown_options,
                    current_mode_index,
                    Message::SelectMode,
                )),
            );

        // Video section
        let video_section = widget::settings::section()
            .title(fl!("settings-video"))
            .add(
                widget::settings::item::builder(fl!("settings-encoder")).control(widget::dropdown(
                    &self.video_encoder_dropdown_options,
                    Some(self.current_video_encoder_index),
                    Message::SelectVideoEncoder,
                )),
            )
            .add(
                widget::settings::item::builder(fl!("settings-quality")).control(widget::dropdown(
                    &self.bitrate_preset_dropdown_options,
                    Some(current_bitrate_index),
                    Message::SelectBitratePreset,
                )),
            )
            .add(
                widget::settings::item::builder(fl!("settings-microphone")).control(
                    widget::dropdown(
                        &self.audio_dropdown_options,
                        Some(self.current_audio_device_index),
                        Message::SelectAudioDevice,
                    ),
                ),
            );

        // Mirror preview section
        let mirror_section = widget::settings::section().add(
            widget::settings::item::builder(fl!("settings-mirror-preview"))
                .description(fl!("settings-mirror-preview-description"))
                .toggler(self.config.mirror_preview, |_| Message::ToggleMirrorPreview),
        );

        // Virtual camera section
        let virtual_camera_section = widget::settings::section().add(
            widget::settings::item::builder(fl!("virtual-camera-title"))
                .description(fl!("virtual-camera-description"))
                .toggler(self.config.virtual_camera_enabled, |_| {
                    Message::ToggleVirtualCameraEnabled
                }),
        );

        // Bug reports section
        let bug_report_button = widget::button::standard(fl!("settings-report-bug"))
            .on_press(Message::GenerateBugReport);

        let bug_report_control = if self.last_bug_report_path.is_some() {
            let show_report_button = widget::button::standard(fl!("settings-show-report"))
                .on_press(Message::ShowBugReport);

            widget::row()
                .push(bug_report_button)
                .push(widget::horizontal_space().width(Length::Fixed(8.0)))
                .push(show_report_button)
                .into()
        } else {
            bug_report_button.into()
        };

        let bug_reports_section = widget::settings::section()
            .title(fl!("settings-bug-reports"))
            .add(widget::settings::item_row(vec![bug_report_control]));

        // Combine all sections
        let settings_content: Element<'_, Message> = widget::settings::view_column(vec![
            camera_section.into(),
            video_section.into(),
            mirror_section.into(),
            virtual_camera_section.into(),
            bug_reports_section.into(),
        ])
        .into();

        context_drawer::context_drawer(
            settings_content,
            Message::ToggleContextPage(crate::app::state::ContextPage::Settings),
        )
        .title(fl!("settings-title"))
    }

    /// Build the bitrate info matrix table (shown when info button is toggled)
    #[allow(dead_code)]
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
                widget::container(
                    widget::text(fl!("preset-low"))
                        .size(12)
                        .font(cosmic::font::bold()),
                )
                .width(Length::Fixed(65.0))
                .center_x(65.0),
            )
            .push(
                widget::container(
                    widget::text(fl!("preset-medium"))
                        .size(12)
                        .font(cosmic::font::bold()),
                )
                .width(Length::Fixed(65.0))
                .center_x(65.0),
            )
            .push(
                widget::container(
                    widget::text(fl!("preset-high"))
                        .size(12)
                        .font(cosmic::font::bold()),
                )
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
