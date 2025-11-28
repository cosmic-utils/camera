// SPDX-License-Identifier: MPL-2.0

//! Filter picker UI view
//!
//! iOS-style horizontal filter selector with live camera preview thumbnails.

use crate::app::state::{AppModel, FilterType, Message};
use crate::app::video_widget::{self, VideoContentFit};
use cosmic::Element;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::iced_widget::scrollable::{Direction, Scrollbar};
use cosmic::widget;
use std::sync::Arc;

/// Filter thumbnail size (same as gallery button - 40x40)
const FILTER_THUMBNAIL_SIZE: f32 = 40.0;
/// Spacing between filter thumbnails
const FILTER_SPACING: u16 = 8;
/// Minimum height for filter picker to match bottom bar
const FILTER_PICKER_MIN_HEIGHT: f32 = 68.0;
/// Scrollable ID for filter picker
const FILTER_PICKER_SCROLLABLE_ID: &str = "filter-picker-scrollable";

impl AppModel {
    /// Build the iOS-style filter picker
    ///
    /// Shows a horizontal list of filter options with live camera preview thumbnails.
    /// Each thumbnail shows the current camera frame with its respective filter applied.
    pub fn build_filter_picker(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::spacing();

        // Define available filters
        let filters: Vec<FilterType> = vec![
            FilterType::Standard,
            FilterType::Vivid,
            FilterType::Warm,
            FilterType::Cool,
            FilterType::Mono,
            FilterType::Sepia,
            FilterType::Noir,
            FilterType::Fade,
            FilterType::Duotone,
            FilterType::Vignette,
            FilterType::Negative,
            FilterType::Posterize,
            FilterType::Solarize,
            FilterType::ChromaticAberration,
            FilterType::Pencil,
        ];

        // Build filter buttons
        let mut filter_row = widget::row()
            .spacing(FILTER_SPACING)
            .align_y(Alignment::Center);

        for filter_type in filters {
            let is_selected = self.selected_filter == filter_type;

            // Create preview thumbnail with camera frame and filter applied
            let thumbnail: Element<'_, Message> = if let Some(frame) = &self.current_frame {
                // Use video widget with the specific filter type
                // All filter previews use video_id = 99 (shared source texture)
                // The filter_type parameter controls which filter shader is applied
                let video_elem = video_widget::video_widget(
                    Arc::clone(frame),
                    99, // Shared source texture ID for all filter previews
                    VideoContentFit::Cover,
                    filter_type,
                    8.0,
                    self.config.mirror_preview,
                );

                // Wrap in fixed-size container
                widget::container(video_elem)
                    .width(Length::Fixed(FILTER_THUMBNAIL_SIZE))
                    .height(Length::Fixed(FILTER_THUMBNAIL_SIZE))
                    .into()
            } else {
                // Fallback: colored placeholder when no camera frame
                let color = match filter_type {
                    FilterType::Standard => Color::from_rgb(0.3, 0.3, 0.3),
                    FilterType::Mono => Color::from_rgb(0.5, 0.5, 0.5),
                    FilterType::Sepia => Color::from_rgb(0.5, 0.4, 0.3),
                    FilterType::Noir => Color::from_rgb(0.2, 0.2, 0.2),
                    FilterType::Vivid => Color::from_rgb(0.4, 0.5, 0.6),
                    FilterType::Cool => Color::from_rgb(0.3, 0.4, 0.5),
                    FilterType::Warm => Color::from_rgb(0.5, 0.4, 0.3),
                    FilterType::Fade => Color::from_rgb(0.45, 0.45, 0.45),
                    FilterType::Duotone => Color::from_rgb(0.3, 0.3, 0.6),
                    FilterType::Vignette => Color::from_rgb(0.35, 0.35, 0.35),
                    FilterType::Negative => Color::from_rgb(0.85, 0.85, 0.85),
                    FilterType::Posterize => Color::from_rgb(0.7, 0.3, 0.5),
                    FilterType::Solarize => Color::from_rgb(0.5, 0.6, 0.35),
                    FilterType::ChromaticAberration => Color::from_rgb(0.6, 0.4, 0.5),
                    FilterType::Pencil => Color::from_rgb(0.9, 0.9, 0.85),
                };
                widget::container(widget::Space::new(
                    Length::Fixed(FILTER_THUMBNAIL_SIZE),
                    Length::Fixed(FILTER_THUMBNAIL_SIZE),
                ))
                .style(move |_theme| widget::container::Style {
                    background: Some(Background::Color(color)),
                    border: Border {
                        radius: [4.0; 4].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            };

            // Wrap thumbnail in a container with selection border
            // Container is slightly larger to show the border without clipping
            let border_width = if is_selected { 2.0 } else { 0.0 };
            let container_size = FILTER_THUMBNAIL_SIZE + border_width * 2.0;
            let bordered_thumbnail = widget::container(thumbnail)
                .width(Length::Fixed(container_size))
                .height(Length::Fixed(container_size))
                .center(container_size)
                .style(move |_theme| widget::container::Style {
                    background: Some(Background::Color(Color::TRANSPARENT)),
                    border: Border {
                        radius: [8.0; 4].into(),
                        width: border_width,
                        color: Color::WHITE,
                    },
                    ..Default::default()
                });

            // Wrap in button for interaction
            let filter_button = widget::button::custom(bordered_thumbnail)
                .on_press(Message::SelectFilter(filter_type))
                .padding(0)
                .class(cosmic::theme::Button::Image);

            filter_row = filter_row.push(filter_button);
        }

        // Wrap filter row in horizontal scrollable for narrow displays
        // Use Direction::Horizontal with programmatic scrolling via mouse_area
        // Use Length::Shrink so content centers when it fits
        // Scrollbar floats over content and only appears when scrolling is needed
        let scrollable_filters = widget::scrollable(filter_row)
            .id(cosmic::widget::Id::new(FILTER_PICKER_SCROLLABLE_ID))
            .direction(Direction::Horizontal(
                Scrollbar::new().width(4).scroller_width(4),
            ))
            .width(Length::Shrink);

        // Wrap in mouse_area to capture all scroll events (vertical and horizontal)
        // and convert them to horizontal scrolling
        let scroll_area = widget::mouse_area(scrollable_filters).on_scroll(|delta| {
            // Convert both vertical and horizontal scroll to horizontal movement
            // delta.y is vertical scroll (mouse wheel), delta.x is horizontal (trackpad)
            // Combine both and use for horizontal scrolling
            use cosmic::iced::mouse::ScrollDelta;
            let scroll_delta = match delta {
                ScrollDelta::Lines { x, y } => x + y,
                ScrollDelta::Pixels { x, y } => (x + y) / 10.0, // Normalize pixels to be similar to lines
            };
            Message::FilterPickerScroll(scroll_delta * 30.0) // Scale for smoother scrolling
        });

        // Center the scrollable horizontally when content fits, allow scrolling when it doesn't
        let centered_scroll = widget::container(scroll_area)
            .width(Length::Fill)
            .center_x(Length::Fill);

        // Container with fixed height to match bottom bar and prevent UI jump
        widget::container(centered_scroll)
            .width(Length::Fill)
            .height(Length::Fixed(FILTER_PICKER_MIN_HEIGHT))
            .padding(spacing.space_xs)
            .center_y(FILTER_PICKER_MIN_HEIGHT)
            .style(|_theme| widget::container::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                ..Default::default()
            })
            .into()
    }

    /// Get the scrollable ID for the filter picker
    pub fn filter_picker_scrollable_id() -> cosmic::widget::Id {
        cosmic::widget::Id::new(FILTER_PICKER_SCROLLABLE_ID)
    }

    /// Get the display name of the currently selected filter
    pub fn selected_filter_name(&self) -> &'static str {
        match self.selected_filter {
            FilterType::Standard => "ORIGINAL",
            FilterType::Mono => "MONO",
            FilterType::Sepia => "SEPIA",
            FilterType::Noir => "NOIR",
            FilterType::Vivid => "VIVID",
            FilterType::Cool => "COOL",
            FilterType::Warm => "WARM",
            FilterType::Fade => "FADE",
            FilterType::Duotone => "DUOTONE",
            FilterType::Vignette => "VIGNETTE",
            FilterType::Negative => "NEGATIVE",
            FilterType::Posterize => "POSTER",
            FilterType::Solarize => "SOLAR",
            FilterType::ChromaticAberration => "CHROMA",
            FilterType::Pencil => "PENCIL",
        }
    }
}
