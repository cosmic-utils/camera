// SPDX-License-Identifier: GPL-3.0-only

//! Settings drawer view

use crate::app::state::{AppModel, Message};
use crate::config::{AppTheme, PhotoOutputFormat};
use crate::constants::BitratePreset;
use crate::fl;
use cosmic::Element;
use cosmic::app::context_drawer;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;
use cosmic::widget::icon;

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

        // Theme index (System = 0, Dark = 1, Light = 2)
        let current_theme_index = match self.config.app_theme {
            AppTheme::System => 0,
            AppTheme::Dark => 1,
            AppTheme::Light => 2,
        };

        // Current backend index for dropdown (find in available backends list)
        let current_backend_index = self
            .available_backends
            .iter()
            .position(|b| *b == self.config.backend)
            .unwrap_or(0);

        // Appearance section
        let appearance_section = widget::settings::section()
            .title(fl!("settings-appearance"))
            .add(
                widget::settings::item::builder(fl!("settings-theme")).control(widget::dropdown(
                    &self.theme_dropdown_options,
                    Some(current_theme_index),
                    Message::SetAppTheme,
                )),
            );

        // Camera section
        // Custom device row with label, info button, and dropdown
        let device_label_with_info = widget::row()
            .push(widget::text::body(fl!("settings-device")))
            .push(widget::horizontal_space().width(Length::Fixed(4.0)))
            .push(
                widget::button::icon(icon::from_name("dialog-information-symbolic").symbolic(true))
                    .extra_small()
                    .on_press(Message::ToggleDeviceInfo),
            )
            .push(widget::horizontal_space())
            .push(widget::dropdown(
                &self.camera_dropdown_options,
                Some(self.current_camera_index),
                Message::SelectCamera,
            ))
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let mut camera_section = widget::settings::section()
            .title(fl!("settings-camera"))
            .add(widget::settings::item_row(vec![
                device_label_with_info.into(),
            ]));

        // Add device info panel if visible
        if self.device_info_visible {
            camera_section = camera_section.add(self.build_device_info_panel());
        }

        // Only show backend dropdown if multiple backends are available
        if self.available_backends.len() > 1 {
            camera_section = camera_section.add(
                widget::settings::item::builder(fl!("settings-backend"))
                    .description(fl!("settings-backend-description"))
                    .control(widget::dropdown(
                        &self.backend_dropdown_options,
                        Some(current_backend_index),
                        Message::SelectBackend,
                    )),
            );
        }

        camera_section = camera_section.add(
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

        // Photo section (output format and HDR+ settings)
        use crate::config::BurstModeSetting;
        // Index 0 = Off, 1 = Auto, 2 = 4 frames, 3 = 6 frames, 4 = 8 frames, 5 = 50 frames
        let current_hdr_index = match self.config.burst_mode_setting {
            BurstModeSetting::Off => 0,
            BurstModeSetting::Auto => 1,
            BurstModeSetting::Frames4 => 2,
            BurstModeSetting::Frames6 => 3,
            BurstModeSetting::Frames8 => 4,
            BurstModeSetting::Frames50 => 5,
        };

        // Photo output format index
        let current_photo_format_index = PhotoOutputFormat::ALL
            .iter()
            .position(|f| *f == self.config.photo_output_format)
            .unwrap_or(0); // Default to JPEG (index 0)

        let photo_section = widget::settings::section()
            .title(fl!("settings-photo"))
            .add(
                widget::settings::item::builder(fl!("settings-photo-format"))
                    .description(fl!("settings-photo-format-description"))
                    .control(widget::dropdown(
                        &self.photo_output_format_dropdown_options,
                        Some(current_photo_format_index),
                        Message::SelectPhotoOutputFormat,
                    )),
            )
            .add(
                widget::settings::item::builder(fl!("settings-hdr-plus"))
                    .description(fl!("settings-hdr-plus-description"))
                    .control(widget::dropdown(
                        &self.burst_mode_frame_count_dropdown_options,
                        Some(current_hdr_index),
                        Message::SetBurstModeFrameCount,
                    )),
            )
            .add(
                widget::settings::item::builder(fl!("settings-save-burst-raw"))
                    .description(fl!("settings-save-burst-raw-description"))
                    .toggler(self.config.save_burst_raw, |_| Message::ToggleSaveBurstRaw),
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
        let sections = vec![
            appearance_section.into(),
            camera_section.into(),
            photo_section.into(),
            video_section.into(),
            mirror_section.into(),
            virtual_camera_section.into(),
            bug_reports_section.into(),
        ];

        let settings_content: Element<'_, Message> = widget::settings::view_column(sections).into();

        context_drawer::context_drawer(
            settings_content,
            Message::ToggleContextPage(crate::app::state::ContextPage::Settings),
        )
        .title(fl!("settings-title"))
    }

    /// Build the device info panel (shown when info button is clicked)
    fn build_device_info_panel(&self) -> Element<'_, Message> {
        // Helper to build a label: value row
        fn info_row<'a>(label: String, value: &str) -> Element<'a, Message> {
            widget::row()
                .push(widget::text(label).size(12).font(cosmic::font::bold()))
                .push(widget::horizontal_space().width(Length::Fixed(8.0)))
                .push(widget::text(value.to_string()).size(12))
                .into()
        }

        let device_info = self
            .available_cameras
            .get(self.current_camera_index)
            .and_then(|c| c.device_info.as_ref());

        let mut info_column = widget::column().spacing(4);

        if let Some(info) = device_info {
            if !info.card.is_empty() {
                info_column = info_column.push(info_row(fl!("device-info-card"), &info.card));
            }
            if !info.driver.is_empty() {
                info_column = info_column.push(info_row(fl!("device-info-driver"), &info.driver));
            }
            if !info.path.is_empty() {
                info_column = info_column.push(info_row(fl!("device-info-path"), &info.path));
            }
            if !info.real_path.is_empty() && info.real_path != info.path {
                info_column =
                    info_column.push(info_row(fl!("device-info-real-path"), &info.real_path));
            }
        } else {
            info_column =
                info_column.push(widget::text("No device information available").size(12));
        }

        widget::container(info_column)
            .padding(8)
            .class(cosmic::theme::Container::Card)
            .into()
    }
}
