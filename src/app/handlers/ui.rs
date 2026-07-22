// SPDX-License-Identifier: GPL-3.0-only

//! UI Navigation handlers
//!
//! Handles context pages, pickers, and tools menu.

use crate::app::state::{AppModel, ContextPage, Message, SettingsPage};
use cosmic::Task;
use cosmic::iced::core::widget::Id;
use cosmic::iced::core::widget::operation::{
    Operation,
    scrollable::{AbsoluteOffset, Scrollable},
};
use cosmic::iced::core::{Rectangle, Vector};
use tracing::{error, info};

/// Widget operation that snaps every scrollable it visits back to the top.
struct ResetScroll;

impl<T> Operation<T> for ResetScroll {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        // Keep descending so any nested scrollable is reset too.
        operate(self);
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
        _translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        state.scroll_to(AbsoluteOffset {
            x: Some(0.0),
            y: Some(0.0),
        });
    }
}

/// Reset the context drawer's scroll position to the top.
///
/// libcosmic renders every context-drawer page inside one shared, id-less
/// scrollable, so switching pages otherwise inherits the previous page's
/// offset. There is no id to target with `scrollable::scroll_to`, hence the
/// bare `Operation`.
///
/// The reset is deliberately unscoped (resets every scrollable it visits). In
/// condensed windows — e.g. on a phone — the drawer is rendered as an iced
/// overlay, which iced operates in a pass separate from the base widget tree.
/// Scoping to the `COSMIC_context_drawer` container id only works in the base
/// pass, so it would miss the overlay drawer. Resetting everything is safe
/// because this only runs on drawer navigation, when the drawer scrollable is
/// the only one that matters.
fn reset_context_drawer_scroll() -> Task<cosmic::Action<Message>> {
    cosmic::iced::runtime::task::widget(ResetScroll)
}

impl AppModel {
    // =========================================================================
    // UI Navigation Handlers
    // =========================================================================

    pub(crate) fn handle_launch_url(&self, url: String) -> Task<cosmic::Action<Message>> {
        match open::that_detached(&url) {
            Ok(()) => {}
            Err(err) => {
                error!(url = %url, error = %err, "Failed to open URL");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_toggle_context_page(
        &mut self,
        context_page: ContextPage,
    ) -> Task<cosmic::Action<Message>> {
        // Close tools menu when opening a context page
        self.tools_menu_visible = false;

        if self.context_page == context_page {
            self.core.window.show_context = !self.core.window.show_context;
        } else {
            self.context_page = context_page;
            self.core.window.show_context = true;
        }
        // The Settings drawer always opens at its top-level category menu.
        if context_page == ContextPage::Settings && self.core.window.show_context {
            self.settings_page = SettingsPage::Root;
        }
        self.sync_audio_probe();
        // Reset the shared drawer scrollable so the new page starts at the top.
        if self.core.window.show_context {
            reset_context_drawer_scroll()
        } else {
            Task::none()
        }
    }

    /// Close all picker overlays
    pub(crate) fn close_all_pickers(&mut self) {
        self.format_picker_visible = false;
        self.exposure_picker_visible = false;
        self.color_picker_visible = false;
        self.tools_menu_visible = false;
        self.motor_picker_visible = false;
    }

    pub(crate) fn handle_toggle_format_picker(&mut self) -> Task<cosmic::Action<Message>> {
        let opening = !self.format_picker_visible;
        self.close_all_pickers();
        self.format_picker_visible = opening;
        if opening {
            self.picker_selected_resolution = self.active_format.as_ref().map(|f| f.width);
        }
        Task::none()
    }

    pub(crate) fn handle_close_format_picker(&mut self) -> Task<cosmic::Action<Message>> {
        self.format_picker_visible = false;
        Task::none()
    }

    pub(crate) fn handle_toggle_device_info(&mut self) -> Task<cosmic::Action<Message>> {
        self.device_info_visible = !self.device_info_visible;
        info!(visible = self.device_info_visible, "Device info toggled");
        Task::none()
    }

    // =========================================================================
    // Tools Menu Handlers
    // =========================================================================

    pub(crate) fn handle_toggle_tools_menu(&mut self) -> Task<cosmic::Action<Message>> {
        // The top bar hides the tools button entirely in View mode and disables
        // it mid-transition, so before the keyboard shortcut existed these
        // states were simply unreachable. Opening the menu here anyway gives a
        // panel with no buttons in it, since every tool is gated on a mode or a
        // camera capability View mode has none of, on top of a full-window
        // overlay that swallows the next click wherever the user aims it.
        // Only opening is blocked: closing has to keep working, or a menu left
        // open by a mode switch could never be dismissed with the keyboard.
        let opening = !self.tools_menu_visible;
        if opening && (self.mode.is_view_only() || self.transition_state.ui_disabled) {
            return Task::none();
        }

        self.close_all_pickers();
        self.tools_menu_visible = opening;
        info!(visible = self.tools_menu_visible, "Tools menu toggled");
        Task::none()
    }

    pub(crate) fn handle_close_tools_menu(&mut self) -> Task<cosmic::Action<Message>> {
        self.tools_menu_visible = false;
        Task::none()
    }

    // =========================================================================
    // Settings Drawer Navigation
    // =========================================================================

    pub(crate) fn handle_open_settings_page(
        &mut self,
        page: SettingsPage,
    ) -> Task<cosmic::Action<Message>> {
        // Sub-pages live inside the Settings drawer, so ensure it is the active
        // context page — this also lets Insights/Shortcuts back-navigate here.
        self.context_page = ContextPage::Settings;
        self.core.window.show_context = true;
        self.settings_page = page;
        self.sync_audio_probe();
        reset_context_drawer_scroll()
    }
}
