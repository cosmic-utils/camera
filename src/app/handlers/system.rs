// SPDX-License-Identifier: GPL-3.0-only

//! System handlers
//!
//! Handles gallery operations, filter selection, settings, recovery, bug reports,
//! and QR code detection.

use crate::app::state::{AppModel, FilterType, Message, RecordingState};
use cosmic::Task;
use cosmic::cosmic_config::CosmicConfigEntry;
use tracing::{error, info};

impl AppModel {
    // =========================================================================
    // Gallery Handlers
    // =========================================================================

    pub(crate) fn handle_open_gallery(&self) -> Task<cosmic::Action<Message>> {
        // If we have a last media path, open the file manager with that file pre-selected
        if let Some(ref path) = self.last_media_path {
            info!(path = %path, "Opening gallery with file pre-selected");
            if Self::show_in_file_manager(path).is_ok() {
                return Task::none();
            }
            info!("show_in_file_manager failed, falling back to directory open");
        }

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
        data: Option<crate::storage::GalleryThumbnailData>,
    ) -> Task<cosmic::Action<Message>> {
        if let Some((handle, rgba, width, height, path)) = data {
            self.gallery_thumbnail = Some(handle);
            self.gallery_thumbnail_rgba = Some((rgba, width, height));
            self.last_media_path = Some(path.display().to_string());
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

        Task::none()
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

    pub(crate) fn handle_audio_list_changed(
        &mut self,
        new_devices: Vec<crate::backends::audio::AudioDevice>,
    ) -> Task<cosmic::Action<Message>> {
        info!(
            old_count = self.available_audio_devices.len(),
            new_count = new_devices.len(),
            "Audio device list changed (hotplug event)"
        );

        // Try to keep the current device selected if it's still available
        let current_still_available = if let Some(current) = self
            .available_audio_devices
            .get(self.current_audio_device_index)
        {
            new_devices
                .iter()
                .position(|d| d.serial == current.serial && d.name == current.name)
        } else {
            None
        };

        // Stop recording if the audio input used for recording was disconnected
        if current_still_available.is_none()
            && self.config.record_audio
            && self.recording.is_recording()
        {
            info!("Audio input disconnected during recording, stopping recording gracefully");
            if let Some(sender) = self.recording.take_stop_sender() {
                let _ = sender.send(());
            }
            self.recording = RecordingState::Idle;
        }

        self.available_audio_devices = new_devices;
        self.audio_dropdown_options = self
            .available_audio_devices
            .iter()
            .map(|dev| {
                if dev.is_default {
                    format!("{} (Default)", dev.name)
                } else {
                    dev.name.clone()
                }
            })
            .collect();

        if let Some(new_index) = current_still_available {
            self.current_audio_device_index = new_index;
        } else {
            // Reset to first device (default is sorted first)
            self.current_audio_device_index = 0;
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

    pub(crate) fn handle_toggle_record_audio(&mut self) -> Task<cosmic::Action<Message>> {
        if self.recording.is_recording() {
            return Task::none();
        }

        use cosmic::cosmic_config::CosmicConfigEntry;

        self.config.record_audio = !self.config.record_audio;
        info!(
            record_audio = self.config.record_audio,
            "Toggled record audio"
        );

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save record audio setting");
        }
        Task::none()
    }

    pub(crate) fn handle_select_audio_encoder(
        &mut self,
        index: usize,
    ) -> Task<cosmic::Action<Message>> {
        use crate::config::AudioEncoder;
        use cosmic::cosmic_config::CosmicConfigEntry;

        if index < AudioEncoder::ALL.len() {
            let encoder = AudioEncoder::ALL[index];
            info!(?encoder, "Selected audio encoder");
            self.config.audio_encoder = encoder;

            if let Some(handler) = self.config_handler.as_ref()
                && let Err(err) = self.config.write_entry(handler)
            {
                error!(?err, "Failed to save audio encoder selection");
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

    pub(crate) fn handle_reset_all_settings(&mut self) -> Task<cosmic::Action<Message>> {
        info!("Resetting all settings to defaults");
        self.config = crate::config::Config::default();

        if let Some(handler) = self.config_handler.as_ref()
            && let Err(err) = self.config.write_entry(handler)
        {
            error!(?err, "Failed to save reset settings");
        }

        self.update_all_dropdowns();
        cosmic::command::set_theme(self.config.app_theme.theme())
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
    pub(crate) fn show_in_file_manager(file_path: &str) -> Result<(), String> {
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
        use crate::backends::camera::types::CameraBackendType;

        let is_libcamera = self.config.backend == CameraBackendType::Libcamera;

        self.update_insights_pipeline(is_libcamera);
        self.update_insights_backend(is_libcamera);
        self.update_insights_format_chain(is_libcamera);
        self.update_insights_performance(is_libcamera);
        self.update_insights_frame_metadata();

        Task::none()
    }

    fn update_insights_pipeline(&mut self, is_libcamera: bool) {
        use crate::app::insights::InsightsState;
        use crate::backends::camera::libcamera::native::diagnostics as diag;

        let new_pipeline = if is_libcamera {
            diag::get_pipeline_string()
        } else {
            crate::media::get_full_pipeline_string()
        };

        let pixel_format = self.active_format.as_ref().map(|f| f.pixel_format.as_str());
        if new_pipeline != self.insights.full_pipeline_string {
            let mjpeg_decoder = if is_libcamera {
                diag::get_mjpeg_decoder()
            } else {
                None
            };
            if mjpeg_decoder.is_some() {
                self.insights.decoder_chain = vec![crate::app::insights::types::DecoderStatus {
                    name: "turbojpeg",
                    description: "libjpeg-turbo MJPEG decoder",
                    state: crate::app::insights::types::FallbackState::Selected,
                }];
            } else {
                self.insights.decoder_chain =
                    InsightsState::build_decoder_chain(pixel_format, new_pipeline.as_deref());
            }
            self.insights.full_pipeline_string = new_pipeline;
        }
    }

    fn update_insights_backend(&mut self, is_libcamera: bool) {
        use crate::app::insights::types::StreamInfo;
        use crate::backends::camera::libcamera::native::diagnostics as diag;

        if is_libcamera {
            self.insights.backend_type = "libcamera".to_string();
            if let Some(camera) = self.available_cameras.get(self.current_camera_index) {
                self.insights.pipeline_handler = camera.pipeline_handler.clone();
                self.insights.libcamera_version = camera.libcamera_version.clone();
                self.insights.sensor_model = camera.sensor_model.clone();
                self.insights.libcamera_multistream_capable = camera.pipeline_handler.is_some();
            }

            self.insights.mjpeg_decoder = diag::get_mjpeg_decoder();
            self.insights.is_multistream = diag::get_is_multistream();

            if let Some((resolution, pixel_fmt, role, frame_count)) =
                diag::get_preview_stream_info()
            {
                self.insights.preview_stream = Some(StreamInfo {
                    role,
                    resolution,
                    pixel_format: pixel_fmt,
                    frame_count,
                });
            }

            if let Some((resolution, pixel_fmt, role, frame_count)) =
                diag::get_capture_stream_info()
            {
                self.insights.capture_stream = Some(StreamInfo {
                    role,
                    resolution,
                    pixel_format: pixel_fmt,
                    frame_count,
                });
            } else if !self.insights.is_multistream {
                self.insights.capture_stream = None;
            }
        } else {
            self.insights.backend_type = "PipeWire".to_string();
            self.insights.pipeline_handler = None;
            self.insights.libcamera_version = None;
            self.insights.sensor_model = None;
            self.insights.is_multistream = false;
            self.insights.libcamera_multistream_capable = false;
            self.insights.preview_stream = None;
            self.insights.capture_stream = None;
        }
    }

    fn update_insights_format_chain(&mut self, is_libcamera: bool) {
        use crate::backends::camera::libcamera::native::diagnostics as diag;
        use crate::backends::camera::pipewire::pipeline;

        let Some(format) = &self.active_format else {
            return;
        };
        let codec = crate::media::Codec::from_fourcc(&format.pixel_format);
        let needs_decoder = codec.needs_decoder();

        let source = if is_libcamera {
            "libcamera (native)".to_string()
        } else {
            self.insights
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
                .to_string()
        };

        let gstreamer_output = if needs_decoder && !is_libcamera {
            pipeline::get_output_format()
        } else {
            None
        };

        let mjpeg_decoder = if is_libcamera {
            self.insights.mjpeg_decoder.as_ref()
        } else {
            None
        };

        let mjpeg_decoded_fmt = if mjpeg_decoder.is_some() {
            diag::get_mjpeg_decoded_format()
        } else {
            None
        };

        let gpu_input_format = if let Some(ref fmt) = mjpeg_decoded_fmt {
            fmt.as_str()
        } else if mjpeg_decoder.is_some() {
            "I420"
        } else if is_libcamera {
            self.insights
                .preview_stream
                .as_ref()
                .map(|s| s.pixel_format.as_str())
                .unwrap_or(&format.pixel_format)
        } else {
            gstreamer_output.as_deref().unwrap_or(&format.pixel_format)
        };

        let wgpu_processing = match gpu_input_format {
            "I420" => "I420 (YUV 4:2:0) \u{2192} RGBA (compute shader)".to_string(),
            "I422" => "I422 (YUV 4:2:2) \u{2192} RGBA (compute shader)".to_string(),
            "I444" => "I444 (YUV 4:4:4) \u{2192} RGBA (compute shader)".to_string(),
            "Nv12" | "NV12" => "NV12 \u{2192} RGBA (compute shader)".to_string(),
            "YUYV" | "YUY2" | "Yuy2" => "YUYV \u{2192} RGBA (compute shader)".to_string(),
            "Rgba" | "RGBA" => "Passthrough".to_string(),
            other => format!("{} \u{2192} RGBA (compute shader)", other),
        };

        self.insights.format_chain.source = source;
        self.insights.format_chain.resolution = if is_libcamera {
            self.insights
                .preview_stream
                .as_ref()
                .map(|s| s.resolution.clone())
                .unwrap_or_else(|| format!("{}x{}", format.width, format.height))
        } else {
            format!("{}x{}", format.width, format.height)
        };
        self.insights.format_chain.framerate = format
            .framerate
            .map(|fps| format!("{} fps", fps))
            .unwrap_or_else(|| "N/A".to_string());
        self.insights.format_chain.native_format = format.pixel_format.clone();
        self.insights.format_chain.gstreamer_output = gstreamer_output;
        self.insights.format_chain.wgpu_processing = wgpu_processing;

        if let Some(ref decoded) = mjpeg_decoded_fmt {
            let yuv_label = match decoded.as_str() {
                "I420" => "YUV 4:2:0",
                "I422" => "YUV 4:2:2",
                "I444" => "YUV 4:4:4",
                "I440" => "YUV 4:4:0",
                "Y800" => "Grayscale",
                other => other,
            };
            self.insights.cpu_processing =
                Some(format!("MJPEG \u{2192} {} (turbojpeg)", yuv_label));
        } else if mjpeg_decoder.is_some() {
            self.insights.cpu_processing = Some("MJPEG \u{2192} YUV (turbojpeg)".to_string());
        } else {
            self.insights.cpu_processing = None;
        }
    }

    fn update_insights_performance(&mut self, is_libcamera: bool) {
        use crate::app::video_primitive;
        use crate::backends::camera::libcamera::native::diagnostics as diag;
        use crate::backends::camera::pipewire::pipeline;

        if is_libcamera {
            self.insights.cpu_decode_time_us = diag::get_mjpeg_decode_time_us();

            if let Some(stream) = &self.insights.preview_stream
                && let Some((w, h)) = stream.resolution.split_once('x')
                && let (Ok(w), Ok(h)) = (w.parse::<usize>(), h.parse::<usize>())
            {
                let bpp = if stream.pixel_format.contains("I420")
                    || stream.pixel_format.contains("NV12")
                {
                    1.5_f64
                } else if stream.pixel_format.contains("RGBA")
                    || stream.pixel_format.contains("BGRA")
                {
                    4.0
                } else if stream.pixel_format.contains("YUYV") {
                    2.0
                } else if stream.pixel_format.contains("RGB24") {
                    3.0
                } else {
                    1.5
                };
                self.insights.frame_size_decoded = (w as f64 * h as f64 * bpp) as usize;
            }
        } else {
            self.insights.gstreamer_decode_time_us = pipeline::get_decode_time_us();
            self.insights.dropped_frames = pipeline::get_dropped_frame_count();
            self.insights.frame_size_decoded = pipeline::get_last_frame_size() as usize;
            self.insights.copy_time_us = pipeline::get_copy_time_us();
        }

        self.insights.gpu_conversion_time_us = video_primitive::get_gpu_upload_time_us();
        let gpu_frame_size = video_primitive::get_gpu_frame_size() as usize;

        if gpu_frame_size > 0 && self.insights.gpu_conversion_time_us > 10 {
            let bytes_per_sec = (gpu_frame_size as f64)
                / (self.insights.gpu_conversion_time_us as f64 / 1_000_000.0);
            self.insights.copy_bandwidth_mbps = bytes_per_sec / (1024.0 * 1024.0);
        } else {
            self.insights.copy_bandwidth_mbps = 0.0;
        }
    }

    fn update_insights_frame_metadata(&mut self) {
        self.insights.audio_levels = self
            .recording
            .audio_levels()
            .and_then(|levels| levels.lock().ok())
            .map(|l| l.clone());

        if let Some(frame) = &self.current_frame {
            self.insights.frame_latency_us = frame.captured_at.elapsed().as_micros() as u64;

            if let Some(meta) = &frame.libcamera_metadata {
                self.insights.has_libcamera_metadata = true;
                self.insights.meta_exposure_us = meta.exposure_time;
                self.insights.meta_analogue_gain = meta.analogue_gain;
                self.insights.meta_digital_gain = meta.digital_gain;
                self.insights.meta_colour_temperature = meta.colour_temperature;
                self.insights.meta_sequence = meta.sequence;
                self.insights.meta_colour_gains = meta.colour_gains;
                self.insights.meta_black_level = meta.black_level;
                self.insights.meta_lens_position = meta.lens_position;
                self.insights.meta_lux = meta.lux;
                self.insights.meta_focus_fom = meta.focus_fom;
            }
        }
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
