// SPDX-License-Identifier: GPL-3.0-only

//! Format picker UI view

use crate::app::state::{AppModel, Message};
use crate::app::view::overlay_container_style;
use crate::constants::ui::OVERLAY_BACKGROUND_ALPHA;
use crate::constants::{formats, ui};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget;

/// Create a container style for the picker panel background
///
/// Uses `radius_s` (slightly rounded) as the maximum roundness.
/// This ensures the picker panel is either square or slightly rounded,
/// even when the theme is set to "round".
/// Does not set text_color to allow buttons to use their native COSMIC theme colors.
fn picker_panel_style(theme: &cosmic::Theme) -> widget::container::Style {
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
            // Use radius_s to cap at "slightly rounded" for panel backgrounds
            radius: cosmic.corner_radii.radius_s.into(),
            ..Default::default()
        },
        // Don't set text_color - let buttons use their native COSMIC theme colors
        ..Default::default()
    }
}

impl AppModel {
    /// Build the iOS-style format picker overlay
    ///
    /// Shows resolution and framerate selection buttons in a semi-transparent overlay.
    /// Click outside the picker to close.
    pub fn build_format_picker(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Group formats by resolution label
        let (unique_resolutions, resolution_groups) = self.group_formats_by_label();

        // Determine currently selected resolution for picker
        let selected_res = self
            .picker_selected_resolution
            .or_else(|| self.active_format.as_ref().map(|f| f.width))
            .unwrap_or(formats::DEFAULT_PICKER_RESOLUTION);

        const BUTTON_WIDTH: f32 = ui::PICKER_BUTTON_WIDTH;

        // Build resolution row
        let mut res_row = widget::row()
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center)
            .push(
                widget::text(fl!("format-resolution"))
                    .size(ui::PICKER_LABEL_TEXT_SIZE)
                    .width(Length::Fixed(ui::PICKER_LABEL_WIDTH)),
            );

        for &(res_label, width) in &unique_resolutions {
            let centered_text = widget::container(widget::text(res_label))
                .width(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center);

            let is_selected = width == selected_res;

            // Use Suggested for selected, Text for unselected - COSMIC's native button highlighting
            let button = widget::button::custom(centered_text)
                .on_press(Message::PickerSelectResolution(width))
                .class(if is_selected {
                    cosmic::theme::Button::Suggested
                } else {
                    cosmic::theme::Button::Text
                })
                .width(Length::Fill);

            // Wrap in styled container with overlay background
            let styled_button = widget::container(button)
                .style(overlay_container_style)
                .width(Length::Fixed(BUTTON_WIDTH));

            res_row = res_row.push(styled_button);
        }

        // Build framerate row
        let mut fps_row = widget::row()
            .spacing(spacing.space_xxs)
            .align_y(Alignment::Center)
            .push(
                widget::text(fl!("format-framerate"))
                    .size(ui::PICKER_LABEL_TEXT_SIZE)
                    .width(Length::Fixed(ui::PICKER_LABEL_WIDTH)),
            );

        if let Some(formats) = resolution_groups.get(&selected_res) {
            use std::collections::HashSet;
            let mut seen_fps: HashSet<u32> = HashSet::new();
            let mut fps_buttons: Vec<(u32, usize)> = Vec::new();

            // Collect unique framerates
            for &(idx, fmt) in formats {
                if let Some(fps) = fmt.framerate {
                    if seen_fps.insert(fps) {
                        fps_buttons.push((fps, idx));
                    }
                }
            }

            fps_buttons.sort_by_key(|(fps, _)| *fps);

            // Create framerate buttons
            for (fps, idx) in fps_buttons {
                let centered_text = widget::container(widget::text(fps.to_string()))
                    .width(Length::Fill)
                    .align_x(cosmic::iced::alignment::Horizontal::Center);

                // Check if this framerate matches the active format's framerate
                // (not exact format match, since pixel format may differ)
                let is_selected = self
                    .active_format
                    .as_ref()
                    .and_then(|active| active.framerate)
                    .map(|active_fps| active_fps == fps)
                    .unwrap_or(false);

                // Use Suggested for selected, Text for unselected - COSMIC's native button highlighting
                let button = widget::button::custom(centered_text)
                    .on_press(Message::PickerSelectFormat(idx))
                    .class(if is_selected {
                        cosmic::theme::Button::Suggested
                    } else {
                        cosmic::theme::Button::Text
                    })
                    .width(Length::Fill);

                // Wrap in styled container with overlay background
                let styled_button = widget::container(button)
                    .style(overlay_container_style)
                    .width(Length::Fixed(BUTTON_WIDTH));

                fps_row = fps_row.push(styled_button);
            }
        }

        // Build picker panel with semi-transparent themed background
        // Uses picker_panel_style which caps roundness at "slightly rounded"
        let picker_panel = widget::container(
            widget::column()
                .push(res_row)
                .push(widget::vertical_space().height(spacing.space_s))
                .push(fps_row)
                .padding(spacing.space_xs),
        )
        .style(picker_panel_style);

        // Position picker and add click-outside-to-close
        let picker_positioned = widget::row()
            .push(picker_panel)
            .push(widget::Space::new(Length::Fill, Length::Shrink))
            .padding([spacing.space_xs, spacing.space_xs, 0, spacing.space_xs]);

        widget::mouse_area(
            widget::container(picker_positioned)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::CloseFormatPicker)
        .into()
    }
}
