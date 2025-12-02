// SPDX-License-Identifier: GPL-3.0-only

//! Custom gallery widget for rendering thumbnails with rounded corners using GPU primitives

use std::sync::Arc;

use crate::app::gallery_primitive::GalleryPrimitive;
use cosmic::iced::advanced::widget::Tree;
use cosmic::iced::advanced::{Widget, layout};
use cosmic::iced::{Element, Length, Rectangle, Size};
use cosmic::iced_wgpu::primitive::Renderer as PrimitiveRenderer;
use cosmic::{Renderer, Theme};

/// Gallery widget that renders thumbnails using a custom GPU primitive with rounded corners
pub struct GalleryWidget {
    primitive: GalleryPrimitive,
    width: Length,
    height: Length,
}

impl GalleryWidget {
    /// Create a new gallery widget from image data
    pub fn new(
        image_handle: cosmic::widget::image::Handle,
        rgba_data: Arc<Vec<u8>>,
        width: u32,
        height: u32,
        corner_radius: f32,
    ) -> Self {
        let primitive =
            GalleryPrimitive::new(image_handle, rgba_data, width, height, corner_radius);

        Self {
            primitive,
            width: Length::Fixed(40.0),
            height: Length::Fixed(40.0),
        }
    }
}

impl Widget<crate::app::Message, Theme, Renderer> for GalleryWidget {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.width(self.width).height(self.height).resolve(
            self.width,
            self.height,
            Size::ZERO,
        );

        layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &cosmic::iced::advanced::renderer::Style,
        layout: layout::Layout<'_>,
        _cursor: cosmic::iced::mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // Draw the custom primitive using the wgpu renderer's primitive support
        renderer.draw_primitive(bounds, self.primitive.clone());
    }
}

impl<'a> From<GalleryWidget> for Element<'a, crate::app::Message, Theme, Renderer> {
    fn from(widget: GalleryWidget) -> Self {
        Element::new(widget)
    }
}

/// Create a gallery widget from image data
pub fn gallery_widget<'a>(
    image_handle: cosmic::widget::image::Handle,
    rgba_data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    corner_radius: f32,
) -> Element<'a, crate::app::Message, Theme, Renderer> {
    Element::new(GalleryWidget::new(
        image_handle,
        rgba_data,
        width,
        height,
        corner_radius,
    ))
}
