// SPDX-License-Identifier: MPL-2.0

//! Custom QR overlay widget
//!
//! This widget renders QR code detection overlays directly using the renderer,
//! calculating positions at render time when actual layout bounds are known.
//! Buttons are rendered as actual cosmic widgets for proper theming.

use super::{
    BUTTON_GAP, OVERLAY_BORDER_RADIUS, OVERLAY_BORDER_WIDTH, calculate_video_bounds,
    get_action_color, transform_detection_to_screen,
};
use crate::app::frame_processor::QrDetection;
use crate::app::state::Message;
use crate::app::video_widget::VideoContentFit;
use cosmic::iced::advanced::widget::{Operation, Tree};
use cosmic::iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, renderer};
use cosmic::iced::event::Status;
use cosmic::iced::{Border, Color, Element, Event, Length, Point, Rectangle, Size};
use cosmic::widget;
use cosmic::{Renderer, Theme};

/// Custom widget for rendering QR code detection overlays
pub struct QrOverlayWidget<'a> {
    detections: Vec<QrDetection>,
    frame_width: u32,
    frame_height: u32,
    content_fit: VideoContentFit,
    mirrored: bool,
    /// Child button elements (one per detection)
    buttons: Vec<Element<'a, Message, Theme, Renderer>>,
}

impl<'a> QrOverlayWidget<'a> {
    /// Create a new QR overlay widget
    pub fn new(
        detections: Vec<QrDetection>,
        frame_width: u32,
        frame_height: u32,
        content_fit: VideoContentFit,
        mirrored: bool,
    ) -> Self {
        // Create button elements for each detection
        let buttons: Vec<Element<'a, Message, Theme, Renderer>> = detections
            .iter()
            .map(|detection| {
                let label = detection.action.action_label();
                let message = super::action_button::action_to_message(&detection.action);

                widget::button::suggested(label).on_press(message).into()
            })
            .collect();

        Self {
            detections,
            frame_width,
            frame_height,
            content_fit,
            mirrored,
            buttons,
        }
    }

    /// Calculate button position for a detection
    fn get_button_position(
        &self,
        detection: &QrDetection,
        offset_x: f32,
        offset_y: f32,
        video_width: f32,
        video_height: f32,
    ) -> (f32, f32) {
        let (x, y, width, height) = transform_detection_to_screen(
            detection,
            offset_x,
            offset_y,
            video_width,
            video_height,
            self.mirrored,
        );

        // Button is centered below the QR box
        let button_x = x + width / 2.0;
        let button_y = y + height + BUTTON_GAP;

        (button_x, button_y)
    }
}

impl<'a> Widget<Message, Theme, Renderer> for QrOverlayWidget<'a> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn children(&self) -> Vec<Tree> {
        self.buttons.iter().map(Tree::new).collect()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.buttons);
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.max();

        // Calculate video bounds
        let (offset_x, offset_y, video_width, video_height) = calculate_video_bounds(
            size.width,
            size.height,
            self.frame_width,
            self.frame_height,
            self.content_fit,
        );

        // Layout each button at its calculated position
        let button_nodes: Vec<layout::Node> = self
            .detections
            .iter()
            .zip(self.buttons.iter())
            .zip(tree.children.iter_mut())
            .map(|((detection, button), child_tree)| {
                let (center_x, top_y) = self.get_button_position(
                    detection,
                    offset_x,
                    offset_y,
                    video_width,
                    video_height,
                );

                // Layout the button with its intrinsic size
                let button_limits = layout::Limits::new(Size::ZERO, Size::new(200.0, 50.0));
                let mut button_node =
                    button
                        .as_widget()
                        .layout(child_tree, renderer, &button_limits);

                // Center the button horizontally at the calculated position
                let button_size = button_node.size();
                let button_x = center_x - button_size.width / 2.0;

                button_node = button_node.move_to(Point::new(button_x, top_y));
                button_node
            })
            .collect();

        layout::Node::with_children(size, button_nodes)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use cosmic::iced::advanced::Renderer as _;

        let bounds = layout.bounds();

        // Calculate video bounds within container
        let (offset_x, offset_y, video_width, video_height) = calculate_video_bounds(
            bounds.width,
            bounds.height,
            self.frame_width,
            self.frame_height,
            self.content_fit,
        );

        // Draw QR detection boxes
        for detection in &self.detections {
            let (x, y, width, height) = transform_detection_to_screen(
                detection,
                offset_x,
                offset_y,
                video_width,
                video_height,
                self.mirrored,
            );

            let qr_bounds = Rectangle {
                x: bounds.x + x,
                y: bounds.y + y,
                width,
                height,
            };

            // Get color based on action type
            let border_color = get_action_color(&detection.action);

            // Draw semi-transparent background with colored border
            renderer.fill_quad(
                renderer::Quad {
                    bounds: qr_bounds,
                    border: Border {
                        color: border_color,
                        width: OVERLAY_BORDER_WIDTH,
                        radius: OVERLAY_BORDER_RADIUS.into(),
                    },
                    shadow: Default::default(),
                },
                Color::from_rgba(0.0, 0.0, 0.0, 0.3),
            );
        }

        // Draw child button widgets
        for ((button, child_tree), child_layout) in self
            .buttons
            .iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            button.as_widget().draw(
                child_tree,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> Status {
        // Forward events to child buttons
        for ((button, child_tree), child_layout) in self
            .buttons
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            let status = button.as_widget_mut().on_event(
                child_tree,
                event.clone(),
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );

            if status == Status::Captured {
                return Status::Captured;
            }
        }

        Status::Ignored
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        // Check child buttons for mouse interaction
        for ((button, child_tree), child_layout) in self
            .buttons
            .iter()
            .zip(tree.children.iter())
            .zip(layout.children())
        {
            let interaction = button.as_widget().mouse_interaction(
                child_tree,
                child_layout,
                cursor,
                viewport,
                renderer,
            );

            if interaction != mouse::Interaction::default() {
                return interaction;
            }
        }

        mouse::Interaction::default()
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        for ((button, child_tree), child_layout) in self
            .buttons
            .iter()
            .zip(tree.children.iter_mut())
            .zip(layout.children())
        {
            button
                .as_widget()
                .operate(child_tree, child_layout, renderer, operation);
        }
    }
}

impl<'a> From<QrOverlayWidget<'a>> for Element<'a, Message, Theme, Renderer> {
    fn from(widget: QrOverlayWidget<'a>) -> Self {
        Element::new(widget)
    }
}
