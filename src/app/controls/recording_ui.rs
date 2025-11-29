// SPDX-License-Identifier: GPL-3.0-only

//! Recording and streaming UI components (indicator and timer)

use crate::app::state::{AppModel, CameraMode, FileSource, Message};
use crate::fl;
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Color, Length};
use cosmic::widget;

/// Create a colored indicator dot (12x12 circle)
fn indicator_dot<'a>(color: Color) -> Element<'a, Message> {
    widget::container(widget::Space::new(Length::Fixed(12.0), Length::Fixed(12.0)))
        .style(move |_theme| widget::container::Style {
            background: Some(Background::Color(color)),
            border: cosmic::iced::Border {
                radius: [6.0; 4].into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// Format duration as MM:SS
fn format_duration(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

impl AppModel {
    /// Check if we have a video file source in Virtual mode
    fn has_video_file_source(&self) -> bool {
        matches!(
            (&self.mode, &self.virtual_camera_file_source),
            (CameraMode::Virtual, Some(FileSource::Video(_)))
        )
    }

    /// Build the recording indicator and timer widget
    ///
    /// Shows a red dot and elapsed time when recording is active.
    /// Returns None when not recording.
    pub fn build_recording_indicator<'a>(&self) -> Option<Element<'a, Message>> {
        if !self.recording.is_recording() {
            return None;
        }

        let spacing = cosmic::theme::spacing();
        let duration_text = format_duration(self.recording.elapsed_duration());

        let row = widget::row()
            .push(indicator_dot(Color::from_rgb(1.0, 0.0, 0.0)))
            .push(widget::horizontal_space().width(spacing.space_xxs))
            .push(widget::text(duration_text).size(14))
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        Some(row.into())
    }

    /// Build the virtual camera streaming indicator widget
    ///
    /// Shows a green dot and "LIVE" label when streaming is active.
    /// Returns None when not streaming.
    pub fn build_streaming_indicator<'a>(&self) -> Option<Element<'a, Message>> {
        if !self.virtual_camera.is_streaming() {
            return None;
        }

        let spacing = cosmic::theme::spacing();

        let row = widget::row()
            .push(indicator_dot(Color::from_rgb(0.1, 0.7, 0.2)))
            .push(widget::horizontal_space().width(spacing.space_xxs))
            .push(widget::text(fl!("streaming-live")).size(14))
            .align_y(Alignment::Center)
            .spacing(spacing.space_xxs);

        Some(row.into())
    }

    /// Build a full-width video progress bar for video file streaming
    ///
    /// Shows a slider-style progress bar with current time and duration labels,
    /// like a video player. Positioned between camera preview and capture button.
    /// Returns None when not in Virtual mode with a video file selected.
    pub fn build_video_progress_bar<'a>(&self) -> Option<Element<'a, Message>> {
        if !self.has_video_file_source() {
            return None;
        }

        let (position, duration) = self
            .video_file_progress
            .map(|(pos, dur, _)| (pos, dur))
            .unwrap_or((0.0, 0.0));

        let spacing = cosmic::theme::spacing();
        let slider_max = if duration > 0.0 { duration } else { 1.0 };

        let progress_row = widget::row()
            .push(widget::text(format_duration(position as u64)).size(12))
            .push(widget::horizontal_space().width(spacing.space_xs))
            .push(
                widget::slider(0.0..=slider_max, position, Message::VideoFileSeek)
                    .width(Length::Fill),
            )
            .push(widget::horizontal_space().width(spacing.space_xs))
            .push(widget::text(format_duration(duration as u64)).size(12))
            .align_y(Alignment::Center)
            .padding([spacing.space_xxs, spacing.space_s])
            .width(Length::Fill);

        Some(progress_row.into())
    }

    /// Build a play/pause toggle button for video file sources
    ///
    /// Shows a play or pause icon depending on current state.
    /// Returns None when not in Virtual mode with a video file selected.
    pub fn build_video_play_pause_button<'a>(&self) -> Option<Element<'a, Message>> {
        if !self.has_video_file_source() {
            return None;
        }

        let icon_name = if self.video_file_paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        };

        let button = widget::button::icon(widget::icon::from_name(icon_name))
            .on_press(Message::ToggleVideoPlayPause)
            .class(cosmic::theme::Button::Standard);

        Some(button.into())
    }
}
