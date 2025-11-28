// SPDX-License-Identifier: MPL-2.0

//! Recording and streaming UI components (indicator and timer)

use crate::app::state::AppModel;
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget;

impl AppModel {
    /// Build the recording indicator and timer widget
    ///
    /// Shows a red dot and elapsed time when recording is active.
    /// Returns None when not recording.
    pub fn build_recording_indicator<'a>(&self) -> Option<Element<'a, crate::app::state::Message>> {
        if !self.recording.is_recording() {
            return None;
        }

        let spacing = cosmic::theme::spacing();

        let mut row = widget::row()
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        // Red recording dot
        let red_dot =
            widget::container(widget::Space::new(Length::Fixed(12.0), Length::Fixed(12.0))).style(
                |_theme| widget::container::Style {
                    background: Some(Background::Color(Color::from_rgb(1.0, 0.0, 0.0))),
                    border: cosmic::iced::Border {
                        radius: [6.0; 4].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );

        row = row.push(red_dot);

        // Get duration from recording state
        let duration = self.recording.elapsed_duration();

        let minutes = duration / 60;
        let seconds = duration % 60;
        let duration_text = format!("{:02}:{:02}", minutes, seconds);

        row = row.push(widget::horizontal_space().width(spacing.space_xxs));
        row = row.push(widget::text(duration_text).size(14));

        Some(row.into())
    }

    /// Build the virtual camera streaming indicator widget
    ///
    /// Shows a green dot and "LIVE" label when streaming is active.
    /// Returns None when not streaming.
    pub fn build_streaming_indicator<'a>(&self) -> Option<Element<'a, crate::app::state::Message>> {
        if !self.virtual_camera.is_streaming() {
            return None;
        }

        let spacing = cosmic::theme::spacing();

        let mut row = widget::row()
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        // Green streaming dot
        let green_dot =
            widget::container(widget::Space::new(Length::Fixed(12.0), Length::Fixed(12.0))).style(
                |_theme| widget::container::Style {
                    background: Some(Background::Color(Color::from_rgb(0.1, 0.7, 0.2))),
                    border: cosmic::iced::Border {
                        radius: [6.0; 4].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );

        row = row.push(green_dot);

        // "LIVE" label
        row = row.push(widget::horizontal_space().width(spacing.space_xxs));
        row = row.push(widget::text(fl!("streaming-live")).size(14));

        Some(row.into())
    }
}
