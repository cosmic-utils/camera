// SPDX-License-Identifier: GPL-3.0-only

//! Keyboard-shortcuts rebinding sub-page within the Settings drawer.

use super::{Action, ActionCategory, format_keybind};
use crate::app::state::{AppModel, Message};
use crate::fl;
use cosmic::Element;
use cosmic::app::context_drawer;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;

pub fn view<'a>(app: &'a AppModel) -> context_drawer::ContextDrawer<'a, Message> {
    let spacing = cosmic::theme::spacing();
    let mut column = widget::column::with_capacity(8).spacing(spacing.space_m);

    for &cat in ActionCategory::ALL {
        let mut section = widget::settings::section().title(cat.label());
        for &action in Action::ALL.iter().filter(|a| a.category() == cat) {
            let combo_text = app
                .bindings
                .keybind_for(action)
                .map(format_keybind)
                .unwrap_or_else(|| fl!("shortcuts-help-unbound"));

            let pill =
                widget::button::text(combo_text).on_press(Message::StartRecordingKeyBind(action));

            let mut controls = widget::row::with_capacity(2)
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center)
                .push(pill);

            if has_override(app, action) {
                controls = controls.push(
                    widget::button::icon(
                        widget::icon::from_name("edit-undo-symbolic").symbolic(true),
                    )
                    .extra_small()
                    .on_press(Message::ResetKeyBindToDefault(action)),
                );
            }

            section =
                section.add(widget::settings::item::builder(action.label()).control(controls));
        }
        column = column.push(section);
    }

    // Page footer: reset-all button.
    column = column.push(
        widget::button::destructive(fl!("keybindings-page-reset-all"))
            .on_press(Message::ResetAllKeyBindings),
    );

    // Recording overlay — rendered above the list when active.
    let body: Element<'a, Message> = if let Some(rec) = &app.recording_keybind {
        let mut col = widget::column::with_capacity(4)
            .spacing(spacing.space_m)
            .push(widget::text::heading(fl!("keybindings-record-title")))
            .push(widget::text::body(fl!("keybindings-record-hint")));

        if let Some(combo) = &rec.captured {
            col = col.push(widget::text::body(format_keybind(combo)));
        }

        if let Some(other) = rec.conflict_with {
            col = col.push(widget::text::body(fl!(
                "keybindings-record-conflict",
                other = other.label()
            )));
        }

        let mut buttons = widget::row::with_capacity(2)
            .spacing(spacing.space_xs)
            .push(
                widget::button::standard(fl!("keybindings-record-cancel"))
                    .on_press(Message::CancelKeyBindRecording),
            );
        if rec.captured.is_some() {
            let save_label = if rec.conflict_with.is_some() {
                fl!("keybindings-record-replace")
            } else {
                fl!("keybindings-record-save")
            };
            buttons =
                buttons.push(widget::button::suggested(save_label).on_press(Message::KeyBindSave));
        }
        col = col.push(buttons);

        widget::container(col)
            .padding(spacing.space_l)
            .width(Length::Fill)
            .into()
    } else {
        widget::scrollable(column).width(Length::Fill).into()
    };

    context_drawer::context_drawer(
        body,
        Message::ToggleContextPage(crate::app::state::ContextPage::KeyBindings),
    )
    .title(fl!("keybindings-page-title"))
}

fn has_override(app: &AppModel, action: Action) -> bool {
    app.bindings.keybind_for(action).cloned() != action.default_keybind()
}
