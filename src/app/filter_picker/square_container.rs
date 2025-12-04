// SPDX-License-Identifier: GPL-3.0-only

//! Square container widget for filter thumbnails
//!
//! Enforces 1:1 aspect ratio by setting height = available width in layout.

use cosmic::iced::advanced::widget::Tree;
use cosmic::iced::advanced::{Widget, layout};
use cosmic::iced::{Element, Length, Rectangle, Size};
use cosmic::{Renderer, Theme};

/// Container that enforces square aspect ratio (height = width)
///
/// Takes the available width from the parent layout and sets its height
/// to match, ensuring the content is always displayed in a square container.
pub struct SquareContainer<'a, Message> {
    content: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message> SquareContainer<'a, Message> {
    /// Create a new square container wrapping the given content
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl<'a, Message> Widget<Message, Theme, Renderer> for SquareContainer<'a, Message> {
    fn size(&self) -> Size<Length> {
        // Fill available width, height will be determined in layout
        Size::new(Length::Fill, Length::Shrink)
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // Get available width from parent
        let available_width = limits.max().width;

        // Force square: height = width
        let square_size = Size::new(available_width, available_width);
        let square_limits = layout::Limits::new(Size::ZERO, square_size);

        // Layout child within square bounds
        let mut child_node =
            self.content
                .as_widget()
                .layout(&mut tree.children[0], renderer, &square_limits);

        // Center the child within the square if it's smaller
        let child_size = child_node.size();
        let x_offset = (available_width - child_size.width) / 2.0;
        let y_offset = (available_width - child_size.height) / 2.0;
        child_node = child_node.move_to(cosmic::iced::Point::new(x_offset, y_offset));

        layout::Node::with_children(square_size, vec![child_node])
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &cosmic::iced::advanced::renderer::Style,
        layout: layout::Layout<'_>,
        cursor: cosmic::iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children().next().unwrap(),
            cursor,
            viewport,
        );
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: layout::Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn cosmic::iced::advanced::widget::Operation,
    ) {
        self.content.as_widget().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: cosmic::iced::Event,
        layout: layout::Layout<'_>,
        cursor: cosmic::iced::mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn cosmic::iced::advanced::Clipboard,
        shell: &mut cosmic::iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> cosmic::iced::event::Status {
        self.content.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: layout::Layout<'_>,
        cursor: cosmic::iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> cosmic::iced::mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }
}

impl<'a, Message: 'a> From<SquareContainer<'a, Message>> for Element<'a, Message, Theme, Renderer> {
    fn from(container: SquareContainer<'a, Message>) -> Self {
        Element::new(container)
    }
}

/// Create a square container that enforces 1:1 aspect ratio
pub fn square_container<'a, Message>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> SquareContainer<'a, Message> {
    SquareContainer::new(content)
}
