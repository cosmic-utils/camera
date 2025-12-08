// SPDX-License-Identifier: GPL-3.0-only

//! UI Navigation handlers
//!
//! Handles context pages, pickers, theatre mode, and tools menu.

use crate::app::state::{AppModel, ContextPage, Message};
use cosmic::Task;
use tracing::{error, info};

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
        Task::none()
    }

    /// Close all picker overlays
    pub(crate) fn close_all_pickers(&mut self) {
        self.format_picker_visible = false;
        self.exposure_picker_visible = false;
        self.color_picker_visible = false;
        self.tools_menu_visible = false;
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

    pub(crate) fn handle_toggle_theatre_mode(&mut self) -> Task<cosmic::Action<Message>> {
        if self.theatre.enabled {
            info!("Exiting theatre mode");
            self.theatre.exit();
        } else {
            info!("Entering theatre mode - UI will hide after 1 second");
            self.theatre.enter();

            return Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::TheatreHideUI),
            );
        }
        Task::none()
    }

    pub(crate) fn handle_theatre_show_ui(&mut self) -> Task<cosmic::Action<Message>> {
        // show_ui() returns true only if state changed (debounces rapid mouse moves)
        if self.theatre.show_ui() {
            tracing::debug!("Theatre mode: showing UI");
            return Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                },
                |_| cosmic::Action::App(Message::TheatreHideUI),
            );
        }
        Task::none()
    }

    pub(crate) fn handle_theatre_hide_ui(&mut self) -> Task<cosmic::Action<Message>> {
        if self.theatre.try_hide_ui() {
            info!("Theatre mode: hiding UI");
            self.close_all_pickers();
        }
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
        let opening = !self.tools_menu_visible;
        self.close_all_pickers();
        self.tools_menu_visible = opening;
        info!(visible = self.tools_menu_visible, "Tools menu toggled");
        Task::none()
    }

    pub(crate) fn handle_close_tools_menu(&mut self) -> Task<cosmic::Action<Message>> {
        self.tools_menu_visible = false;
        Task::none()
    }
}
