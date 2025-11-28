// SPDX-License-Identifier: MPL-2.0

//! Main application module for COSMIC Camera
//!
//! This module contains the application state, message handling, UI rendering,
//! and business logic for the camera application.
//!
//! # Architecture
//!
//! - `state`: Application state types (AppModel, Message, CameraMode, etc.)
//! - `camera_preview`: Camera preview display widget
//! - `controls`: Capture button and recording UI
//! - `bottom_bar`: Gallery, mode switcher, camera switcher
//! - `settings`: Settings drawer UI
//! - `format_picker`: Format/resolution picker UI and logic
//! - `dropdowns`: Dropdown management
//! - `camera_ops`: Camera operations (switching cameras, changing formats)
//! - `ui`: UI widget building (legacy)
//! - `view`: Main view rendering
//! - `update`: Message handling
//!
//! # Main Types
//!
//! - `AppModel`: Main application state with camera management
//! - `Message`: All possible user interactions and system events
//! - `CameraMode`: Photo or Video capture modes

mod bottom_bar;
mod camera_ops;
mod camera_preview;
mod controls;
mod dropdowns;
mod filter_picker;
mod format_picker;
pub mod frame_processor;
mod gallery_primitive;
mod gallery_widget;
pub mod qr_overlay;
pub mod settings;
mod state;
mod ui;
mod update;
mod utils;
mod video_primitive;
mod video_widget;
mod view;

// Re-export public API
use crate::config::Config;
use crate::fl;
use cosmic::app::context_drawer;
use cosmic::cosmic_config::{self, CosmicConfigEntry};
use cosmic::iced::Subscription;
use cosmic::widget::{self, about::About};
use cosmic::{Element, Task};
pub use state::{
    AppModel, CameraMode, ContextPage, FilterType, Message, RecordingState, TheatreState,
    VirtualCameraState,
};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Get the photo save directory (~/Pictures/cosmic-camera)
pub fn get_photo_directory() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::Path::new(&home)
        .join("Pictures")
        .join("cosmic-camera")
}

/// Ensure the photo directory exists, creating it if necessary
fn ensure_photo_directory() -> Result<std::path::PathBuf, std::io::Error> {
    let photo_dir = get_photo_directory();
    std::fs::create_dir_all(&photo_dir)?;
    info!(path = %photo_dir.display(), "Photo directory ready");
    Ok(photo_dir)
}

const REPOSITORY: &str = "https://github.com/FreddyFunk/cosmic-camera";
const APP_ICON: &[u8] = include_bytes!(
    "../../resources/icons/hicolor/scalable/apps/io.github.freddyfunk.cosmic-camera.svg"
);

impl cosmic::Application for AppModel {
    /// The async executor that will be used to run your application's commands.
    type Executor = cosmic::executor::Default;

    /// Data that your application receives to its init method.
    type Flags = ();

    /// Messages which the application and its widgets will emit.
    type Message = Message;

    /// Unique identifier in RDNN (reverse domain name notation) format.
    const APP_ID: &'static str = "io.github.freddyfunk.cosmic-camera";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    /// Initializes the application with any given flags and startup commands.
    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        // Create the about widget
        let about = About::default()
            .name(fl!("app-title"))
            .icon(widget::icon::from_svg_bytes(APP_ICON))
            .version(env!("GIT_VERSION"))
            .links([(fl!("repository"), REPOSITORY)])
            .license(env!("CARGO_PKG_LICENSE"));

        // Load configuration
        let (config_handler, config) =
            match cosmic_config::Config::new(Self::APP_ID, Config::VERSION) {
                Ok(handler) => {
                    let config = match Config::get_entry(&handler) {
                        Ok(config) => config,
                        Err((errors, config)) => {
                            error!(?errors, "Errors loading config");
                            config
                        }
                    };
                    (Some(handler), config)
                }
                Err(err) => {
                    error!(%err, "Failed to create config handler");
                    (None, Config::default())
                }
            };

        // Ensure photo directory exists
        if let Err(e) = ensure_photo_directory() {
            error!(error = %e, "Failed to create photo directory");
        }

        // Initialize GStreamer early (required before any GStreamer calls)
        // This is safe to do on the main thread as it's a one-time initialization
        if let Err(e) = gstreamer::init() {
            error!(error = %e, "Failed to initialize GStreamer");
        }

        // Start with empty camera list - will be populated by async task
        let available_cameras = Vec::new();
        let current_camera_index = 0;
        let available_formats = Vec::new();
        let initial_format = None;
        let camera_dropdown_options = Vec::new();

        // Enumerate audio devices synchronously (fast operation)
        let available_audio_devices = crate::backends::audio::enumerate_audio_devices();
        let current_audio_device_index = 0; // Default device is sorted first
        let audio_dropdown_options: Vec<String> = available_audio_devices
            .iter()
            .map(|dev| {
                if dev.is_default {
                    format!("{} (Default)", dev.name)
                } else {
                    dev.name.clone()
                }
            })
            .collect();

        // Enumerate video encoders synchronously
        let available_video_encoders = crate::media::encoders::video::enumerate_video_encoders();
        // Use saved encoder index, or default to 0 (best encoder is sorted first)
        let current_video_encoder_index = config
            .last_video_encoder_index
            .filter(|&idx| idx < available_video_encoders.len())
            .unwrap_or(0);
        let video_encoder_dropdown_options: Vec<String> = available_video_encoders
            .iter()
            .map(|enc| {
                // Replace (HW) with (hardware accelerated) and (SW) with (software)
                enc.display_name
                    .replace(" (HW)", " (hardware accelerated)")
                    .replace(" (SW)", " (software)")
            })
            .collect();

        // Create backend manager
        let backend_manager = crate::backends::camera::CameraBackendManager::new(config.backend);

        // Construct the app model with the runtime's core.
        let mut app = AppModel {
            core,
            context_page: ContextPage::default(),
            about,
            config,
            config_handler,
            mode: CameraMode::Photo,
            recording: RecordingState::default(),
            virtual_camera: VirtualCameraState::default(),
            is_capturing: false,
            format_picker_visible: false,
            theatre: TheatreState::default(),
            filter_picker_visible: false,
            selected_filter: FilterType::default(),
            flash_enabled: false,
            flash_active: false,
            last_bug_report_path: None,
            gallery_thumbnail: None,
            gallery_thumbnail_rgba: None,
            picker_selected_resolution: None,
            backend_manager: Some(backend_manager),
            camera_cancel_flag: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            current_frame: None,
            available_cameras,
            current_camera_index,
            available_formats: available_formats.clone(),
            active_format: initial_format,
            available_audio_devices,
            current_audio_device_index,
            available_video_encoders,
            current_video_encoder_index,
            mode_list: Vec::new(), // Will be updated below
            camera_dropdown_options,
            audio_dropdown_options,
            video_encoder_dropdown_options,
            mode_dropdown_options: Vec::new(), // Will be updated below
            pixel_format_dropdown_options: Vec::new(), // Will be updated below
            resolution_dropdown_options: Vec::new(), // Will be updated below
            framerate_dropdown_options: Vec::new(), // Will be updated below
            codec_dropdown_options: Vec::new(), // Will be updated below
            bitrate_preset_dropdown_options: crate::constants::BitratePreset::ALL
                .iter()
                .map(|p| p.display_name().to_string())
                .collect(),
            bitrate_info_visible: false,
            filter_picker_scroll_offset: 0.0,
            transition_state: crate::app::state::TransitionState::new(),
            // QR detection enabled by default
            qr_detection_enabled: true,
            qr_detections: Vec::new(),
            last_qr_detection_time: None,
        };

        // Update all dropdown options based on initial format
        app.update_mode_options();
        app.update_resolution_options();
        app.update_pixel_format_options();
        app.update_framerate_options();
        app.update_codec_options();

        // Initialize cameras and video encoders asynchronously (non-blocking)
        let backend_type = app.config.backend;
        let last_camera_path = app.config.last_camera_path.clone();

        let init_task = Task::perform(
            async move {
                // Check available video encoders (can be slow)
                crate::pipelines::video::check_available_encoders();

                // Enumerate cameras (can be slow, especially with multiple devices)
                info!(backend = %backend_type, "Enumerating cameras asynchronously");
                let backend = crate::backends::camera::get_backend();
                let cameras = backend.enumerate_cameras();
                info!(count = cameras.len(), backend = %backend_type, "Found camera(s)");

                // Find the last used camera or default to first
                let camera_index = if let Some(ref last_path) = last_camera_path {
                    info!(path = %last_path, "Attempting to restore last camera");
                    cameras
                        .iter()
                        .enumerate()
                        .find(|(_, cam)| &cam.path == last_path)
                        .map(|(idx, _)| {
                            info!(index = idx, "Found saved camera");
                            idx
                        })
                        .unwrap_or_else(|| {
                            info!("Saved camera not found, using first camera");
                            0
                        })
                } else {
                    info!("No saved camera, using first camera");
                    0
                };

                // Get formats for selected camera
                let formats = if let Some(camera) = cameras.get(camera_index) {
                    if !camera.path.is_empty() {
                        backend.get_formats(camera, false)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                (cameras, camera_index, formats)
            },
            |(cameras, index, formats)| {
                cosmic::Action::App(Message::CamerasInitialized(cameras, index, formats))
            },
        );

        // Load initial gallery thumbnail
        let load_thumbnail_task = Task::perform(
            async { crate::storage::load_latest_thumbnail(get_photo_directory()).await },
            |handle| cosmic::Action::App(Message::GalleryThumbnailLoaded(handle)),
        );

        (app, Task::batch([init_task, load_thumbnail_task]))
    }

    /// Elements to pack at the start of the header bar.
    fn header_start(&self) -> Vec<Element<'_, Self::Message>> {
        vec![]
    }

    /// Elements to pack at the end of the header bar.
    fn header_end(&self) -> Vec<Element<'_, Self::Message>> {
        let is_disabled = self.transition_state.ui_disabled;

        if is_disabled {
            // Disabled settings button during transitions
            let settings_button =
                widget::button::icon(widget::icon::from_name("preferences-system-symbolic"));
            vec![
                widget::container(settings_button)
                    .style(|_theme| widget::container::Style {
                        text_color: Some(cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                        ..Default::default()
                    })
                    .into(),
            ]
        } else {
            vec![
                widget::button::icon(widget::icon::from_name("preferences-system-symbolic"))
                    .on_press(Message::ToggleContextPage(ContextPage::Settings))
                    .into(),
            ]
        }
    }

    /// Display a context drawer if the context page is requested.
    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Self::Message>> {
        if !self.core.window.show_context {
            return None;
        }

        Some(match self.context_page {
            ContextPage::About => context_drawer::about(
                &self.about,
                |url| Message::LaunchUrl(url.to_string()),
                Message::ToggleContextPage(ContextPage::About),
            ),
            ContextPage::Settings => self.settings_view(),
        })
    }

    /// Describes the interface based on the current state of the application model.
    fn view(&self) -> Element<'_, Self::Message> {
        self.view()
    }

    /// Register subscriptions for this application.
    fn subscription(&self) -> Subscription<Self::Message> {
        use cosmic::iced::futures::{SinkExt, StreamExt};

        let config_sub = self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config));

        // Get current camera device path and format
        let current_camera = self
            .available_cameras
            .get(self.current_camera_index)
            .cloned();
        let camera_index = self.current_camera_index;
        let current_format = self.active_format.clone();
        let cancel_flag = Arc::clone(&self.camera_cancel_flag);

        // Create a unique ID based on format properties to trigger restart when format changes
        let format_id = current_format
            .as_ref()
            .map(|f| (f.width, f.height, f.framerate, f.pixel_format.clone()));

        // Include whether cameras are initialized in the subscription ID
        // This ensures the subscription restarts when cameras become available
        let cameras_initialized = !self.available_cameras.is_empty();

        let camera_sub = Subscription::run_with_id(
            (
                "camera",
                camera_index,
                format_id,
                // NOTE: is_recording is NOT included here!
                // This allows preview to continue during recording (PipeWire multi-consumer)
                // NOTE: mode is NOT included here!
                // Camera only needs to restart when actual format changes, not on mode switch
                cameras_initialized,
            ), // Camera restarts only when format_id or camera_index changes
            cosmic::iced::stream::channel(100, move |mut output| async move {
                info!(camera_index, "Camera subscription started (PipeWire)");

                // No artificial delay needed - PipelineManager serializes all operations
                // and ensures proper cleanup before creating new pipelines

                let mut frame_count = 0u64;
                loop {
                    // Check cancel flag at the start of each loop iteration
                    // This prevents creating new pipelines after mode switch
                    if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
                        info!("Cancel flag set - subscription loop exiting");
                        break;
                    }

                    // If no camera available yet (cameras not initialized), just exit the subscription
                    // The subscription will restart when cameras become available (cameras_initialized flag changes)
                    if current_camera.is_none() {
                        info!(
                            "No camera available - subscription will restart when cameras are initialized"
                        );
                        break;
                    }

                    let device_path = current_camera.as_ref().and_then(|cam| {
                        if cam.path.is_empty() {
                            None
                        } else {
                            Some(cam.path.as_str())
                        }
                    });

                    // Extract format parameters
                    let (width, height, framerate, pixel_format) =
                        if let Some(fmt) = &current_format {
                            (
                                Some(fmt.width),
                                Some(fmt.height),
                                fmt.framerate,
                                Some(fmt.pixel_format.as_str()),
                            )
                        } else {
                            (None, None, None, None)
                        };

                    if let Some(cam) = &current_camera {
                        info!(name = %cam.name, path = %cam.path, "Creating camera");
                    } else {
                        info!("Creating default camera...");
                    }

                    if let Some(fmt) = &current_format {
                        info!(format = %fmt, "Using format");
                    }

                    // PipeWire backend for any mode (Photo or Video)
                    {
                        // Check cancel flag before creating pipeline
                        if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
                            info!("Cancel flag set before pipeline creation - skipping");
                            break;
                        }

                        // Note: Preview continues during recording since VideoRecorder has its own pipeline
                        // Both can access the same PipeWire node simultaneously

                        // Give previous pipeline time to clean up (50ms should be enough)
                        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                        // Check cancel flag again after brief wait
                        if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
                            info!("Cancel flag set after cleanup wait - skipping");
                            break;
                        }

                        // Create camera pipeline using PipeWire backend
                        use crate::backends::camera::pipewire::PipeWirePipeline;
                        use crate::backends::camera::types::{CameraDevice, CameraFormat};

                        let (sender, mut receiver) =
                            cosmic::iced::futures::channel::mpsc::channel(100);

                        // Build device and format objects for backend
                        let device = CameraDevice {
                            name: current_camera
                                .as_ref()
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| "Default Camera".to_string()),
                            path: device_path.unwrap_or("").to_string(),
                            metadata_path: current_camera
                                .as_ref()
                                .and_then(|c| c.metadata_path.clone()),
                        };

                        let format = CameraFormat {
                            width: width.unwrap_or(640),
                            height: height.unwrap_or(480),
                            framerate: framerate,
                            hardware_accelerated: true, // Assume HW acceleration available
                            pixel_format: pixel_format.unwrap_or("MJPEG").to_string(),
                        };

                        let pipeline_opt = match PipeWirePipeline::new(&device, &format, sender) {
                            Ok(pipeline) => {
                                info!("Pipeline created successfully");
                                Some(pipeline)
                            }
                            Err(e) => {
                                error!(error = %e, "Failed to initialize pipeline");
                                None
                            }
                        };

                        if let Some(pipeline) = pipeline_opt {
                            info!("Waiting for frames from pipeline...");
                            // Keep pipeline alive and forward frames
                            loop {
                                // Check cancel flag first (set when switching cameras/modes)
                                if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
                                    info!(
                                        "Cancel flag set - PipeWire subscription being cancelled"
                                    );
                                    break;
                                }

                                // Check if subscription is still active before processing next frame
                                if output.is_closed() {
                                    info!(
                                        "Output channel closed - PipeWire subscription being cancelled"
                                    );
                                    break;
                                }

                                // Wait for next frame with a timeout to periodically check cancellation
                                // Use 16ms timeout (~60fps) to reduce frame delivery jitter
                                match tokio::time::timeout(
                                    tokio::time::Duration::from_millis(16),
                                    receiver.next(),
                                )
                                .await
                                {
                                    Ok(Some(frame)) => {
                                        frame_count += 1;
                                        // Calculate frame latency (time from capture to subscription delivery)
                                        let latency_us = frame.captured_at.elapsed().as_micros();

                                        if frame_count % 30 == 0 {
                                            info!(
                                                frame = frame_count,
                                                width = frame.width,
                                                height = frame.height,
                                                latency_ms = latency_us as f64 / 1000.0,
                                                "Received frame from pipeline"
                                            );
                                        }

                                        // Warn if latency exceeds 2 frame periods (>33ms at 60fps)
                                        if latency_us > 33_000 {
                                            tracing::warn!(
                                                frame = frame_count,
                                                latency_ms = latency_us as f64 / 1000.0,
                                                "High frame latency detected - possible stuttering"
                                            );
                                        }

                                        // Use try_send to avoid blocking the subscription when UI is busy
                                        // Dropping frames is fine for live preview - we want the latest frame
                                        match output.try_send(Message::CameraFrame(Arc::new(frame)))
                                        {
                                            Ok(_) => {
                                                if frame_count % 30 == 0 {
                                                    info!(
                                                        frame = frame_count,
                                                        "Frame forwarded to UI"
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                // Always log dropped frames for diagnostics
                                                tracing::warn!(
                                                    frame = frame_count,
                                                    error = ?e,
                                                    "Frame dropped (UI channel full) - stuttering likely"
                                                );
                                                // Check if channel is closed (subscription cancelled)
                                                if e.is_disconnected() {
                                                    info!(
                                                        "Output channel disconnected - PipeWire subscription being cancelled"
                                                    );
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    Ok(None) => {
                                        info!("PipeWire pipeline frame stream ended");
                                        break;
                                    }
                                    Err(_) => {
                                        // Timeout - continue loop to check if channel is closed
                                        continue;
                                    }
                                }
                            }
                            info!("Cleaning up PipeWire pipeline");
                            // Pipeline will be dropped here, stopping the camera
                            drop(pipeline);
                        } else {
                            error!("Failed to initialize pipeline");
                            info!("Waiting 5 seconds before retry...");
                            // Wait a bit before retrying
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        }
                    }
                }
            }),
        );

        // Camera hotplug monitoring subscription
        let backend_manager = self.backend_manager.clone();
        let current_cameras = self.available_cameras.clone();
        let hotplug_sub = Subscription::run_with_id(
            "camera_hotplug",
            cosmic::iced::stream::channel(10, move |mut output| async move {
                info!("Camera hotplug monitoring started");

                let mut last_cameras = current_cameras;

                // Only run if backend_manager is available
                let Some(backend_mgr) = backend_manager else {
                    warn!("No backend manager available for hotplug monitoring");
                    return;
                };

                loop {
                    // Wait 2 seconds between checks
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                    // Enumerate current cameras
                    if let Ok(new_cameras) = backend_mgr.enumerate_cameras() {
                        // Compare with last known list
                        // Check if camera list changed (different count or different cameras)
                        let cameras_changed = last_cameras.len() != new_cameras.len()
                            || !last_cameras.iter().all(|c| {
                                new_cameras
                                    .iter()
                                    .any(|nc| nc.path == c.path && nc.name == c.name)
                            });

                        if cameras_changed {
                            info!(
                                old_count = last_cameras.len(),
                                new_count = new_cameras.len(),
                                "Camera list changed - hotplug event detected"
                            );

                            last_cameras = new_cameras.clone();

                            // Send camera list changed message
                            if output
                                .send(Message::CameraListChanged(new_cameras))
                                .await
                                .is_err()
                            {
                                warn!(
                                    "Failed to send camera list changed message - channel closed"
                                );
                                break;
                            }
                        }
                    } else {
                        // No cameras available - treat as empty list
                        if !last_cameras.is_empty() {
                            info!("All cameras disconnected");
                            last_cameras = Vec::new();
                            if output
                                .send(Message::CameraListChanged(Vec::new()))
                                .await
                                .is_err()
                            {
                                warn!(
                                    "Failed to send camera list changed message - channel closed"
                                );
                                break;
                            }
                        }
                    }
                }

                info!("Camera hotplug monitoring stopped");
            }),
        );

        // QR detection subscription (samples frames at 1 FPS)
        let should_detect_qr = self.qr_detection_enabled
            && self
                .last_qr_detection_time
                .map(|t| t.elapsed() >= std::time::Duration::from_secs(1))
                .unwrap_or(true);

        let qr_detection_sub = match (should_detect_qr, &self.current_frame) {
            (true, Some(frame)) => {
                let frame = frame.clone();
                Subscription::run_with_id(
                    ("qr_detection", frame.captured_at),
                    cosmic::iced::stream::channel(1, move |mut output| async move {
                        let detector = frame_processor::tasks::QrDetector::new();
                        let detections = detector.detect(frame).await;
                        let _ = output.send(Message::QrDetectionsUpdated(detections)).await;
                    }),
                )
            }
            _ => Subscription::none(),
        };

        Subscription::batch([config_sub, camera_sub, hotplug_sub, qr_detection_sub])
    }

    /// Handles messages emitted by the application and its widgets.
    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        self.update(message)
    }
}
