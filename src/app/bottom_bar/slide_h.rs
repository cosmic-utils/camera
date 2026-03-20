// SPDX-License-Identifier: GPL-3.0-only

//! A wrapper widget that shifts its child horizontally during draw
//! without changing layout. Reads the offset from a shared atomic so
//! the position updates every frame, even when view() isn't called.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use cosmic::iced::advanced::layout;
use cosmic::iced::advanced::renderer;
use cosmic::iced::advanced::widget::tree::Tree;
use cosmic::iced::advanced::{Clipboard, Layout, Shell, Widget};
use cosmic::iced::{Event, Length, Rectangle, Size, Vector};
use cosmic::iced_core::mouse;

use crate::app::state::Message;
use cosmic::Theme;
type Renderer = cosmic::Renderer;

/// Wrapper that draws its child shifted horizontally. The offset magnitude
/// is read from a shared atomic (f32 bits) during draw(), and the sign
/// is determined by `sign` (+1.0 or -1.0).
pub struct SlideH<'a> {
    child: cosmic::Element<'a, Message>,
    slide_shared: Arc<AtomicU32>,
    sign: f32,
}

impl<'a> SlideH<'a> {
    pub fn new(
        child: cosmic::Element<'a, Message>,
        slide_shared: Arc<AtomicU32>,
        sign: f32,
    ) -> Self {
        Self {
            child,
            slide_shared,
            sign,
        }
    }

    /// Compute the expanded viewport rectangle that covers the translated position.
    fn expanded_viewport(&self, bounds: Rectangle) -> Rectangle {
        let offset = f32::from_bits(self.slide_shared.load(std::sync::atomic::Ordering::Relaxed))
            * self.sign;
        if offset < 0.0 {
            Rectangle {
                x: bounds.x + offset,
                width: bounds.width - offset,
                ..bounds
            }
        } else {
            Rectangle {
                width: bounds.width + offset,
                ..bounds
            }
        }
    }
}

impl<'a> Widget<Message, Theme, Renderer> for SlideH<'a> {
    fn tag(&self) -> cosmic::iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> cosmic::iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn children(&self) -> Vec<cosmic::iced::advanced::widget::Tree> {
        self.child.as_widget().children()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.child.as_widget_mut().layout(tree, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use cosmic::iced::advanced::Renderer as _;
        let offset = f32::from_bits(self.slide_shared.load(std::sync::atomic::Ordering::Relaxed))
            * self.sign;
        // Adjust cursor to match the visual translation so the child's
        // hover detection aligns with where the button is drawn.
        let adjusted_cursor = cursor.position().map_or(cursor, |pos| {
            mouse::Cursor::Available(cosmic::iced::Point::new(pos.x - offset, pos.y))
        });
        // Use with_layer() to expand the clipping region to cover the
        // translated position. Parent containers clip viewport to their
        // bounds, so with_translation alone can't render outside them.
        let expanded = self.expanded_viewport(layout.bounds());
        renderer.with_layer(expanded, |renderer| {
            renderer.with_translation(Vector::new(offset, 0.0), |renderer| {
                self.child.as_widget().draw(
                    tree,
                    renderer,
                    theme,
                    style,
                    layout,
                    adjusted_cursor,
                    &expanded,
                );
            });
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        // Offset cursor to match the visual translation so the child's
        // hit-testing aligns with where the button is actually drawn.
        let offset = f32::from_bits(self.slide_shared.load(std::sync::atomic::Ordering::Relaxed))
            * self.sign;
        let adjusted_cursor = cursor.position().map_or(cursor, |pos| {
            mouse::Cursor::Available(cosmic::iced::Point::new(pos.x - offset, pos.y))
        });
        let expanded = self.expanded_viewport(layout.bounds());
        self.child.as_widget_mut().update(
            tree,
            event,
            layout,
            adjusted_cursor,
            renderer,
            clipboard,
            shell,
            &expanded,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let offset = f32::from_bits(self.slide_shared.load(std::sync::atomic::Ordering::Relaxed))
            * self.sign;
        let adjusted_cursor = cursor.position().map_or(cursor, |pos| {
            mouse::Cursor::Available(cosmic::iced::Point::new(pos.x - offset, pos.y))
        });
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, adjusted_cursor, viewport, renderer)
    }
}

impl<'a> From<SlideH<'a>> for cosmic::Element<'a, Message> {
    fn from(slide: SlideH<'a>) -> Self {
        cosmic::Element::new(slide)
    }
}
