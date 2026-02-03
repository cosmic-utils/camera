// SPDX-License-Identifier: GPL-3.0-only

//! System handlers
//!
//! Handles gallery operations, filter selection, settings, recovery, bug reports,
//! and QR code detection.

use crate::app::state::{AppModel, FilterType, Message};
use cosmic::Task;
use cosmic::cosmic_config::CosmicConfigEntry;
use std::sync::Arc;
use tracing::{error, info};

impl AppModel {
    // =========================================================================
    // Gallery Handlers
    // =========================================================================

    pub(crate) fn handle_open_gallery(&self) -> Task<cosmic::Action<Message>> {
        let photo_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
        info!(path = %photo_dir.display(), "Opening gallery directory");

        if let Err(e) = open::that(&photo_dir) {
            error!(error = %e, path = %photo_dir.display(), "Failed to open gallery directory");
        } else {
            info!("Gallery opened successfully");
        }
        Task::none()
    }

    pub(crate) fn handle_refresh_gallery_thumbnail(&self) -> Task<cosmic::Action<Message>> {
        let photos_dir = crate::app::get_photo_directory(&self.config.save_folder_name);
        let videos_dir = crate::app::get_video_directory(&self.config.save_folder_name);
        Task::perform(
            async move { crate::storage::load_latest_thumbnail(photos_dir, videos_dir).await },
            |handle| cosmic::Action::App(Message::GalleryThumbnailLoaded(handle)),
        )
    }

    pub(crate) fn handle_gallery_thumbnail_loaded(
        &mut self,
        data: Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some((handle, rgba, width, height)) = data {
            self.gallery_thumbnail = Some(handle);
            self.gallery_thumbnail_rgba = Some((rgba, width, height));
        } else {
            self.gallery_thumbnail = None;
            self.gallery_thumbnail_rgba = None;
        }
        Task::none()
    }

    // =========================================================================
    // Filter Handlers
    // =========================================================================

    pub(crate) fn handle_select_filter(
        &mut self,
        filter: FilterType,
    ) -> Task<cosmic::Action<Message>> {
        self.selected_filter = filter;
        info!("Filter selected: {:?}", filter);

        // Update virtual camera filter if streaming
        if self.virtual_camera.is_streaming() {
            self.virtual_camera.set_filter(filter);
        }

        // Reset color settings to defaults when applying any filter (including Standard)
        // This ensures filters work with neutral camera settings
        self.reset_color_settings_to_defaults()
    }

    // =========================================================================
    // Settings Handlers
    // =========================================================================

    pub(crate) fn handle_update_config(
        &mut self,
        config: crate::config::Config,
    ) -> Task<cosmic::Action<Message>> {
        info!("UpdateConfig received");
        self.config = config;
        Task::none()
    }

    pub(crate) fn handle_set_app_theme(&mut self, index: usize) -> Task<cosmic::Action<Message>> {
        use crate::config::AppTheme;

        let app_theme = match index {
            0 => AppTheme::System,
            1 => AppTheme::Dark,
            2 => AppTheme::Light,
            _ => return Task::none(),
        };

        info!(?app_theme, "Setting application theme");
        self.config.app_theme = app_theme;

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save app theme setting");
        }

        cosmic::command::set_theme(app_theme.theme())
    }

    pub(crate) fn handle_select_audio_device(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        if index < self.available_audio_devices.len() {
            info!(index, "Selected audio device index");
            self.current_audio_device_index = index;
        }
        Task::none()
    }

    pub(crate) fn handle_select_video_encoder(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        if index < self.available_video_encoders.len() {
            info!(index, encoder = %self.available_video_encoders[index].display_name, "Selected video encoder");
            self.current_video_encoder_index = index;

            self.config.last_video_encoder_index = Some(index);
            if let Some(handler) = self.config_handler.as_ref()
                && let Err(err) = self.config.write_entry(handler)
            {
                error!(?err, "Failed to save encoder selection");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_select_photo_output_format(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        use crate::config::PhotoOutputFormat;

        if index < PhotoOutputFormat::ALL.len() {
            let format = PhotoOutputFormat::ALL[index];
            info!(?format, "Selected photo output format");
            self.config.photo_output_format = format;

            if let Some(handler) = self.config_handler.as_ref()
                && let Err(err) = self.config.write_entry(handler)
            {
                error!(?err, "Failed to save photo output format selection");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_toggle_save_burst_raw(&mut self) -> Task<cosmic::Action<Message>> {
        self.config.save_burst_raw = !self.config.save_burst_raw;
        info!(
            save_burst_raw = self.config.save_burst_raw,
            "Toggled save burst raw frames"
        );

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save burst raw setting");
        }
        Task::none()
    }

    // =========================================================================
    // System & Recovery Handlers
    // =========================================================================

    pub(crate) fn handle_camera_recovery_started(
        &self,
        attempt: u32,
        max_attempts: u32,
    ) -> Task<cosmic::Action<Message>> {
        info!(attempt, max_attempts, "Camera backend recovery started");
        Task::none()
    }

    pub(crate) fn handle_camera_recovery_succeeded(&self) -> Task<cosmic::Action<Message>> {
        info!("Camera backend recovery succeeded");
        Task::none()
    }

    pub(crate) fn handle_camera_recovery_failed(
        &self,
        error: String,
    ) -> Task<cosmic::Action<Message>> {
        error!(error = %error, "Camera backend recovery failed");
        Task::none()
    }

    pub(crate) fn handle_audio_recovery_started(
        &self,
        attempt: u32,
        max_attempts: u32,
    ) -> Task<cosmic::Action<Message>> {
        info!(attempt, max_attempts, "Audio backend recovery started");
        Task::none()
    }

    pub(crate) fn handle_audio_recovery_succeeded(&self) -> Task<cosmic::Action<Message>> {
        info!("Audio backend recovery succeeded");
        Task::none()
    }

    pub(crate) fn handle_audio_recovery_failed(
        &self,
        error: String,
    ) -> Task<cosmic::Action<Message>> {
        error!(error = %error, "Audio backend recovery failed");
        Task::none()
    }

    pub(crate) fn handle_generate_bug_report(&self) -> Task<cosmic::Action<Message>> {
        info!("Generating bug report...");

        let video_devices = self.available_cameras.clone();
        let audio_devices = self.available_audio_devices.clone();
        let video_encoders = self.available_video_encoders.clone();
        let selected_encoder_index = self.current_video_encoder_index;
        let save_folder_name = self.config.save_folder_name.clone();

        Task::perform(
            async move {
                crate::bug_report::BugReportGenerator::generate(
                    &video_devices,
                    &audio_devices,
                    &video_encoders,
                    selected_encoder_index,
                    None,
                    &save_folder_name,
                )
                .await
                .map(|p| p.display().to_string())
            },
            |result| cosmic::Action::App(Message::BugReportGenerated(result)),
        )
    }

    pub(crate) fn handle_bug_report_generated(
        &mut self,
        result: Result<String, String>,
    ) -> Task<cosmic::Action<Message>> {
        match result {
            Ok(path) => {
                info!(path = %path, "Bug report generated successfully");
                self.last_bug_report_path = Some(path);

                let url = &self.config.bug_report_url;
                if let Err(e) = open::that(url) {
                    error!(error = %e, url = %url, "Failed to open bug report URL");
                } else {
                    info!(url = %url, "Opened bug report URL");
                }
            }
            Err(err) => {
                error!(error = %err, "Failed to generate bug report");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_show_bug_report(&self) -> Task<cosmic::Action<Message>> {
        if let Some(report_path) = &self.last_bug_report_path {
            info!(path = %report_path, "Showing bug report in file manager");
            if let Err(e) = Self::show_in_file_manager(report_path) {
                error!(error = %e, path = %report_path, "Failed to show bug report in file manager");
            }
        }
        Task::none()
    }

    // =========================================================================
    // Helper Functions
    // =========================================================================

    /// Show a file in the file manager with pre-selection
    fn show_in_file_manager(file_path: &str) -> Result<(), String> {
        use std::process::Command;

        let path = std::path::Path::new(file_path);
        let file_uri = format!("file://{}", path.display());

        // Method 1: Try D-Bus FileManager1.ShowItems
        let dbus_result = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{}", file_uri),
                "string:",
            ])
            .output();

        if let Ok(output) = dbus_result
            && output.status.success()
        {
            info!("Opened file manager via D-Bus");
            return Ok(());
        }

        // Method 2: Try file manager-specific commands
        let file_managers = [
            ("nautilus", vec!["--select", file_path]),
            ("dolphin", vec!["--select", file_path]),
            ("nemo", vec![file_path]),
            ("caja", vec![file_path]),
            ("thunar", vec![file_path]),
        ];

        for (fm_name, args) in &file_managers {
            if let Ok(output) = Command::new(fm_name).args(args).spawn() {
                info!(file_manager = fm_name, "Opened file manager");
                drop(output);
                return Ok(());
            }
        }

        // Method 3: Fallback to opening the parent directory
        if let Some(parent) = path.parent()
            && let Ok(child) = Command::new("xdg-open").arg(parent).spawn()
        {
            info!("Opened parent directory as fallback");
            drop(child);
            return Ok(());
        }

        Err("Failed to open file manager".to_string())
    }

    // =========================================================================
    // QR Code Detection Handlers
    // =========================================================================

    pub(crate) fn handle_toggle_qr_detection(&mut self) -> Task<cosmic::Action<Message>> {
        self.qr_detection_enabled = !self.qr_detection_enabled;
        info!(enabled = self.qr_detection_enabled, "QR detection toggled");

        // Clear detections when disabling
        if !self.qr_detection_enabled {
            self.qr_detections.clear();
        }

        Task::none()
    }

    pub(crate) fn handle_qr_detections_updated(
        &mut self,
        detections: Vec<crate::app::frame_processor::QrDetection>,
    ) -> Task<cosmic::Action<Message>> {
        let count = detections.len();
        self.qr_detections = detections;
        self.last_qr_detection_time = Some(std::time::Instant::now());

        if count > 0 {
            info!(count, "QR detections updated");
        }

        Task::none()
    }

    pub(crate) fn handle_qr_open_url(&self, url: String) -> Task<cosmic::Action<Message>> {
        info!(url = %url, "Opening URL from QR code");
        match open::that_detached(&url) {
            Ok(()) => {
                info!("URL opened successfully");
            }
            Err(err) => {
                error!(url = %url, error = %err, "Failed to open URL");
            }
        }
        Task::none()
    }

    pub(crate) fn handle_qr_connect_wifi(
        &self,
        ssid: String,
        password: Option<String>,
        security: String,
        hidden: bool,
    ) -> Task<cosmic::Action<Message>> {
        // Use NetworkManager D-Bus API - works in both native and flatpak
        Task::perform(
            crate::network_manager::connect_wifi(ssid, password, security, hidden),
            |_| cosmic::Action::App(Message::Noop),
        )
    }

    pub(crate) fn handle_qr_copy_text(&self, text: String) -> Task<cosmic::Action<Message>> {
        info!(
            text_length = text.len(),
            "Copying text from QR code to clipboard"
        );

        // Use iced/cosmic clipboard API - works in both native and flatpak
        cosmic::iced::clipboard::write(text).map(|_: ()| cosmic::Action::App(Message::Noop))
    }

    // =========================================================================
    // Insights Handlers
    // =========================================================================

    pub(crate) fn handle_update_insights_metrics(&mut self) -> Task<cosmic::Action<Message>> {
        use crate::app::insights::InsightsState;
        use crate::app::video_primitive;
        use crate::backends::camera::pipewire::pipeline;

        // Update pipeline info and rebuild decoder chain only if changed
        let new_pipeline = crate::media::get_full_pipeline_string();
        let pixel_format = self.active_format.as_ref().map(|f| f.pixel_format.as_str());
        if new_pipeline != self.insights.full_pipeline_string {
            self.insights.decoder_chain =
                InsightsState::build_decoder_chain(pixel_format, new_pipeline.as_deref());
            self.insights.full_pipeline_string = new_pipeline;
        }

        // Update format chain from active format and pipeline
        if let Some(format) = &self.active_format {
            let codec = crate::media::Codec::from_fourcc(&format.pixel_format);
            let needs_decoder = codec.needs_decoder();

            // Determine source type from pipeline
            let source = self
                .insights
                .full_pipeline_string
                .as_ref()
                .map(|p| {
                    if !p.contains("pipewiresrc") {
                        "Unknown"
                    } else if p.contains("v4l2:") || p.contains("path=v4l2") {
                        "V4L2 via PipeWire"
                    } else if p.contains("libcamera") {
                        "libcamera via PipeWire"
                    } else {
                        "PipeWire"
                    }
                })
                .unwrap_or("Unknown")
                .to_string();

            // Get GStreamer output format (if decoding is involved)
            let gstreamer_output = if needs_decoder {
                pipeline::get_output_format()
            } else {
                None
            };

            // Determine WGPU processing based on the format reaching the GPU
            let gpu_input_format = gstreamer_output.as_deref().unwrap_or(&format.pixel_format);
            let wgpu_processing = match gpu_input_format {
                "I420" => "I420 → RGBA (compute shader)".to_string(),
                "NV12" => "NV12 → RGBA (compute shader)".to_string(),
                "YUYV" | "YUY2" => "YUYV → RGBA (compute shader)".to_string(),
                "RGBA" => "Passthrough".to_string(),
                other => format!("{} → RGBA (compute shader)", other),
            };

            self.insights.format_chain.source = source;
            self.insights.format_chain.resolution = format!("{}x{}", format.width, format.height);
            self.insights.format_chain.framerate = format
                .framerate
                .map(|fps| format!("{} fps", fps))
                .unwrap_or_else(|| "N/A".to_string());
            self.insights.format_chain.native_format = format.pixel_format.clone();
            self.insights.format_chain.gstreamer_output = gstreamer_output;
            self.insights.format_chain.wgpu_processing = wgpu_processing;
        }

        // Update performance metrics
        self.insights.gstreamer_decode_time_us = pipeline::get_decode_time_us();
        self.insights.dropped_frames = pipeline::get_dropped_frame_count();
        self.insights.frame_size_decoded = pipeline::get_last_frame_size() as usize;
        self.insights.copy_time_us = pipeline::get_copy_time_us();

        // Get GPU upload metrics from video_primitive
        self.insights.gpu_conversion_time_us = video_primitive::get_gpu_upload_time_us();
        let gpu_frame_size = video_primitive::get_gpu_frame_size() as usize;

        // Calculate GPU upload bandwidth if we have meaningful upload time (> 10us)
        if gpu_frame_size > 0 && self.insights.gpu_conversion_time_us > 10 {
            let bytes_per_sec = (gpu_frame_size as f64)
                / (self.insights.gpu_conversion_time_us as f64 / 1_000_000.0);
            self.insights.copy_bandwidth_mbps = bytes_per_sec / (1024.0 * 1024.0);
        } else {
            self.insights.copy_bandwidth_mbps = 0.0;
        }

        // Update frame latency from last frame capture time
        if let Some(frame) = &self.current_frame {
            self.insights.frame_latency_us = frame.captured_at.elapsed().as_micros() as u64;
        }

        Task::none()
    }

    pub(crate) fn handle_copy_pipeline_string(&self) -> Task<cosmic::Action<Message>> {
        if let Some(pipeline) = &self.insights.full_pipeline_string {
            info!("Copying pipeline string to clipboard");
            cosmic::iced::clipboard::write(pipeline.clone())
                .map(|_: ()| cosmic::Action::App(Message::Noop))
        } else {
            Task::none()
        }
    }
}
