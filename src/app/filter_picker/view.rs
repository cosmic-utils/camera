// SPDX-License-Identifier: GPL-3.0-only

//! Filter picker UI view
//!
//! Grid-style filter selector using COSMIC context drawer with live camera preview thumbnails.

use crate::app::state::{AppModel, ContextPage, FilterType, Message};
use crate::app::video_widget::{self, VideoContentFit};
use crate::fl;
use cosmic::Element;
use cosmic::app::context_drawer;
use cosmic::iced::{Alignment, Background, Border, Color, Length};
use cosmic::widget;
use std::sync::Arc;

/// Spacing between filter thumbnails in grid
const FILTER_GRID_SPACING: f32 = 6.0;
/// Border width for selected filter
const FILTER_BORDER_WIDTH: f32 = 2.0;
/// Number of columns in the filter grid
const FILTER_GRID_COLUMNS: usize = 3;
/// Context drawer content width
const DRAWER_CONTENT_WIDTH: f32 = 420.0;
/// Calculated thumbnail size: (drawer_width - (columns-1) * spacing) / columns
const FILTER_THUMBNAIL_SIZE: f32 = (DRAWER_CONTENT_WIDTH
    - (FILTER_GRID_COLUMNS as f32 - 1.0) * FILTER_GRID_SPACING)
    / FILTER_GRID_COLUMNS as f32;

impl AppModel {
    /// Build the filter picker as a COSMIC context drawer
    ///
    /// Shows a grid of filter options with live camera preview thumbnails
    /// and filter names below each thumbnail.
    pub fn filters_view(&self) -> context_drawer::ContextDrawer<'_, Message> {
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

        // Build filter grid with calculated thumbnail sizes
        let spacing = FILTER_GRID_SPACING as u16;
        let mut grid_column = widget::column().spacing(spacing);
        let mut current_row = widget::row().spacing(spacing);
        let mut items_in_row = 0;

        let inner_size = FILTER_THUMBNAIL_SIZE - FILTER_BORDER_WIDTH * 2.0;

        for filter_type in filters {
            let is_selected = self.selected_filter == filter_type;

            // Create preview thumbnail with camera frame and filter applied
            let thumbnail: Element<'_, Message> = if let Some(frame) = &self.current_frame {
                // Use video widget with the specific filter type
                let video_elem = video_widget::video_widget(
                    Arc::clone(frame),
                    99, // Shared source texture ID for all filter previews
                    VideoContentFit::Cover,
                    filter_type,
                    8.0,
                    self.config.mirror_preview,
                );

                widget::container(video_elem)
                    .width(Length::Fixed(inner_size))
                    .height(Length::Fixed(inner_size))
                    .into()
            } else {
                // Fallback: colored placeholder when no camera frame
                let color = Self::filter_placeholder_color(filter_type);
                widget::container(widget::Space::new(
                    Length::Fixed(inner_size),
                    Length::Fixed(inner_size),
                ))
                .style(move |_theme| widget::container::Style {
                    background: Some(Background::Color(color)),
                    border: Border {
                        radius: [8.0; 4].into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            };

            // Wrap thumbnail in container with selection border
            let bordered_thumbnail = widget::container(thumbnail)
                .width(Length::Fixed(FILTER_THUMBNAIL_SIZE))
                .height(Length::Fixed(FILTER_THUMBNAIL_SIZE))
                .center(FILTER_THUMBNAIL_SIZE)
                .style(move |_theme| widget::container::Style {
                    background: Some(Background::Color(Color::TRANSPARENT)),
                    border: Border {
                        radius: [10.0; 4].into(),
                        width: if is_selected {
                            FILTER_BORDER_WIDTH
                        } else {
                            0.0
                        },
                        color: if is_selected {
                            Color::from_rgb(0.3, 0.6, 1.0) // Accent blue for selection
                        } else {
                            Color::TRANSPARENT
                        },
                    },
                    ..Default::default()
                });

            // Wrap only thumbnail in button for interaction (hover applies only to preview)
            let thumbnail_button = widget::button::custom(bordered_thumbnail)
                .on_press(Message::SelectFilter(filter_type))
                .padding(0)
                .class(cosmic::theme::Button::Image);

            // Filter name label below thumbnail (outside button, no hover effect)
            let name_label = widget::text(Self::filter_display_name(filter_type))
                .width(Length::Fixed(FILTER_THUMBNAIL_SIZE))
                .align_x(cosmic::iced::alignment::Horizontal::Center);

            // Column with button (thumbnail only) and name below
            let filter_button = widget::column()
                .push(thumbnail_button)
                .push(widget::vertical_space().height(Length::Fixed(4.0)))
                .push(name_label)
                .align_x(Alignment::Center);

            current_row = current_row.push(filter_button);
            items_in_row += 1;

            // Start new row after FILTER_GRID_COLUMNS items
            if items_in_row >= FILTER_GRID_COLUMNS {
                grid_column = grid_column.push(current_row);
                current_row = widget::row().spacing(spacing);
                items_in_row = 0;
            }
        }

        // Push remaining items in last row
        if items_in_row > 0 {
            grid_column = grid_column.push(current_row);
        }

        // Grid content
        let content: Element<'_, Message> = grid_column.into();

        context_drawer::context_drawer(content, Message::ToggleContextPage(ContextPage::Filters))
            .title(fl!("filters-title"))
    }

    /// Get placeholder color for a filter type
    fn filter_placeholder_color(filter_type: FilterType) -> Color {
        match filter_type {
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
        }
    }

    /// Get display name for a filter type (used in filter picker grid)
    fn filter_display_name(filter_type: FilterType) -> &'static str {
        match filter_type {
            FilterType::Standard => "Original",
            FilterType::Mono => "Mono",
            FilterType::Sepia => "Sepia",
            FilterType::Noir => "Noir",
            FilterType::Vivid => "Vivid",
            FilterType::Cool => "Cool",
            FilterType::Warm => "Warm",
            FilterType::Fade => "Fade",
            FilterType::Duotone => "Duotone",
            FilterType::Vignette => "Vignette",
            FilterType::Negative => "Negative",
            FilterType::Posterize => "Posterize",
            FilterType::Solarize => "Solarize",
            FilterType::ChromaticAberration => "Chroma",
            FilterType::Pencil => "Pencil",
        }
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
