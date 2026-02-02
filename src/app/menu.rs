// SPDX-License-Identifier: GPL-3.0-only

use cosmic::widget::menu::{Item as MenuItem, ItemHeight, ItemWidth};
use cosmic::{Element, app::Core, widget::responsive_menu_bar};
use std::sync::LazyLock;

use super::{ContextPage, Message};
use crate::fl;

static MENU_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("responsive-menu"));

pub fn menu_bar<'a>(core: &Core) -> Element<'a, Message> {
    responsive_menu_bar()
        .item_height(ItemHeight::Dynamic(40))
        .item_width(ItemWidth::Uniform(240))
        .spacing(4.0)
        .into_element(
            core,
            &std::collections::HashMap::new(),
            MENU_ID.clone(),
            Message::Surface,
            vec![(
                fl!("view"),
                vec![
                    MenuItem::Button(fl!("settings-title"), None, MenuAction::Settings),
                    MenuItem::Divider,
                    MenuItem::Button(fl!("about"), None, MenuAction::About),
                ],
            )],
        )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    Settings,
    About,
}

impl cosmic::widget::menu::Action for MenuAction {
    type Message = Message;

    fn message(&self) -> Self::Message {
        match self {
            MenuAction::Settings => Message::ToggleContextPage(ContextPage::Settings),
            MenuAction::About => Message::ToggleContextPage(ContextPage::About),
        }
    }
}
