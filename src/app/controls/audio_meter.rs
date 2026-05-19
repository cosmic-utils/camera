// SPDX-License-Identifier: GPL-3.0-only

//! Live audio level meter widget shared by the Settings drawer, the
//! recording-active chip, and the Insights drawer.
//!
//! Renders a horizontal RMS bar with an optional peak line. dB→fraction
//! mapping, colour thresholds, and peak-line suppression live here so they
//! can be unit-tested without an iced runtime.

use cosmic::Element;
use cosmic::iced::{Background, Border, Color, Length};
use cosmic::widget;

/// Style knobs for the meter.
#[derive(Debug, Clone, Copy)]
pub struct AudioMeterStyle {
    /// Total bar width in logical pixels.
    pub width: f32,
    /// Bar height in logical pixels.
    pub height: f32,
    /// When `true`, draw a 2 px peak indicator line on top of the RMS bar.
    pub show_peak: bool,
}

/// Map a dB value to a 0..1 fraction of the bar width.
///
/// −60 dB and below → 0.0, 0 dB and above → 1.0.
pub(crate) fn fraction_for_db(db: f64) -> f32 {
    (((db + 60.0) / 60.0).clamp(0.0, 1.0)) as f32
}

/// Colour for an RMS / peak value, matching the Insights drawer convention
/// (green safe, yellow approaching, red clipping).
pub(crate) fn color_for_db(db: f64) -> Color {
    if db < -12.0 {
        Color::from_rgb(0.2, 0.8, 0.3) // green
    } else if db < -3.0 {
        Color::from_rgb(0.9, 0.8, 0.1) // yellow
    } else {
        Color::from_rgb(0.9, 0.2, 0.2) // red
    }
}

/// X offset (in logical pixels, left-aligned within the bar) of the peak
/// indicator. Returns `None` when the peak is below the meter's noise
/// floor — drawing the line in that case would pin it to the left edge.
pub(crate) fn peak_offset(peak_db: f64, width: f32) -> Option<f32> {
    if peak_db <= -60.0 {
        return None;
    }
    Some(fraction_for_db(peak_db) * width)
}

/// Build the audio level meter as an iced `Element`.
///
/// Returns just the bar — callers that want a numeric dB readout add
/// their own text widget next to it (see `settings/view.rs` and the
/// `recording_ui.rs` chip for examples).
pub fn audio_meter<'a, Msg: 'a>(
    peak_db: f64,
    rms_db: f64,
    style: AudioMeterStyle,
) -> Element<'a, Msg> {
    let AudioMeterStyle {
        width,
        height,
        show_peak,
    } = style;

    let rms_color = color_for_db(rms_db);
    let rms_width = (fraction_for_db(rms_db) * width).max(0.0);

    // Solid RMS rectangle, drawn over a subtle background.
    let rms_bar = widget::container(widget::Space::new().width(rms_width).height(height))
        .class(cosmic::style::Container::custom(move |_theme| {
            widget::container::Style {
                background: Some(Background::Color(rms_color)),
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .width(Length::Fixed(rms_width))
        .height(Length::Fixed(height));

    // Background (drawn first, behind the RMS bar) provides "empty space" visibility.
    let background = widget::container(widget::Space::new().width(width).height(height))
        .class(cosmic::style::Container::custom(|theme| {
            let mut bg: Color = theme.cosmic().button_bg_color().into();
            bg.a = 0.35;
            widget::container::Style {
                background: Some(Background::Color(bg)),
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .width(Length::Fixed(width))
        .height(Length::Fixed(height));

    let mut layers: Vec<Element<'a, Msg>> = vec![background.into(), rms_bar.into()];

    if show_peak && let Some(peak_x) = peak_offset(peak_db, width) {
        let peak_color = brighten(color_for_db(peak_db));
        let line_x = (peak_x - 1.0).clamp(0.0, (width - 2.0).max(0.0));
        let spacer = widget::Space::new()
            .width(Length::Fixed(line_x))
            .height(height);
        let line = widget::container(widget::Space::new().width(2.0).height(height))
            .class(cosmic::style::Container::custom(move |_theme| {
                widget::container::Style {
                    background: Some(Background::Color(peak_color)),
                    ..Default::default()
                }
            }))
            .width(Length::Fixed(2.0))
            .height(Length::Fixed(height));
        let row = widget::Row::new()
            .push(spacer)
            .push(line)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height));
        layers.push(row.into());
    }

    // Stack overlays layers in z-order: background, RMS bar, optional peak line.
    cosmic::iced::widget::Stack::with_children(layers)
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        .into()
}

fn brighten(c: Color) -> Color {
    Color::from_rgb(
        ((c.r + 1.0) * 0.5).min(1.0),
        ((c.g + 1.0) * 0.5).min(1.0),
        ((c.b + 1.0) * 0.5).min(1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_for_db_table() {
        assert!((fraction_for_db(-100.0) - 0.0).abs() < 1e-6);
        assert!((fraction_for_db(-60.0) - 0.0).abs() < 1e-6);
        assert!((fraction_for_db(-30.0) - 0.5).abs() < 1e-6);
        assert!((fraction_for_db(0.0) - 1.0).abs() < 1e-6);
        assert!((fraction_for_db(5.0) - 1.0).abs() < 1e-6);
    }

    fn rgb(c: Color) -> (f32, f32, f32) {
        (c.r, c.g, c.b)
    }

    #[test]
    fn color_for_db_boundaries() {
        // Just below -12 → green.
        assert_eq!(rgb(color_for_db(-12.001)), (0.2, 0.8, 0.3));
        // Exactly -12 → yellow (band is inclusive at the lower edge).
        assert_eq!(rgb(color_for_db(-12.0)), (0.9, 0.8, 0.1));
        // Just below -3 → yellow.
        assert_eq!(rgb(color_for_db(-3.001)), (0.9, 0.8, 0.1));
        // Exactly -3 → red.
        assert_eq!(rgb(color_for_db(-3.0)), (0.9, 0.2, 0.2));
    }

    #[test]
    fn peak_offset_suppresses_silent_peak() {
        assert!(peak_offset(-60.0, 100.0).is_none());
        assert!(peak_offset(-100.0, 100.0).is_none());
    }

    #[test]
    fn peak_offset_scales_with_width() {
        // -30 dB → 50% of width.
        let off = peak_offset(-30.0, 80.0).expect("not silent");
        assert!((off - 40.0).abs() < 1e-4);
    }
}
