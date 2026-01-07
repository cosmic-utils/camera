// SPDX-License-Identifier: GPL-3.0-only

//! Application state management

use crate::app::exposure_picker::{
    AvailableExposureControls, ColorSettings, ExposureMode, ExposureSettings, MeteringMode,
};
use crate::app::frame_processor::QrDetection;
use crate::backends::audio::AudioDevice;
use crate::backends::camera::CameraBackendManager;
use crate::backends::camera::types::{CameraDevice, CameraFormat, CameraFrame};
use crate::config::Config;
use crate::media::encoders::video::EncoderInfo;
use cosmic::cosmic_config;
use cosmic::widget::about::About;
use std::sync::Arc;
use std::time::Instant;

/// Recording state machine
///
/// Simple two-state design: either recording or not.
#[derive(Debug, Default)]
pub enum RecordingState {
    /// Not recording
    #[default]
    Idle,
    /// Actively recording
    Recording {
        /// When recording started
        start_time: Instant,
        /// Output file path
        file_path: String,
        /// Channel to signal stop
        stop_sender: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

impl RecordingState {
    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        matches!(self, RecordingState::Recording { .. })
    }

    /// Get the recording file path if recording
    pub fn file_path(&self) -> Option<&str> {
        match self {
            RecordingState::Idle => None,
            RecordingState::Recording { file_path, .. } => Some(file_path),
        }
    }

    /// Get the elapsed recording duration
    pub fn elapsed_duration(&self) -> u64 {
        match self {
            RecordingState::Idle => 0,
            RecordingState::Recording { start_time, .. } => start_time.elapsed().as_secs(),
        }
    }

    /// Take the stop sender (consumes it)
    pub fn take_stop_sender(&mut self) -> Option<tokio::sync::oneshot::Sender<()>> {
        match self {
            RecordingState::Idle => None,
            RecordingState::Recording { stop_sender, .. } => stop_sender.take(),
        }
    }

    /// Start recording
    pub fn start(file_path: String, stop_sender: tokio::sync::oneshot::Sender<()>) -> Self {
        RecordingState::Recording {
            start_time: Instant::now(),
            file_path,
            stop_sender: Some(stop_sender),
        }
    }

    /// Stop recording (returns Idle)
    pub fn stop(&mut self) -> Self {
        std::mem::replace(self, RecordingState::Idle)
    }
}

/// Virtual camera streaming state machine
#[derive(Default)]
pub enum VirtualCameraState {
    /// Not streaming
    #[default]
    Idle,
    /// Actively streaming to virtual camera
    Streaming {
        /// When streaming started
        start_time: Instant,
        /// Channel to signal stop
        stop_sender: Option<tokio::sync::oneshot::Sender<()>>,
        /// Channel to send frames to the virtual camera pipeline
        frame_sender: tokio::sync::mpsc::UnboundedSender<Arc<CameraFrame>>,
        /// Channel to send filter updates to the virtual camera pipeline
        filter_sender: tokio::sync::watch::Sender<FilterType>,
        /// Whether streaming from a file source (image/video)
        is_file_source: bool,
    },
}

impl std::fmt::Debug for VirtualCameraState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VirtualCameraState::Idle => write!(f, "Idle"),
            VirtualCameraState::Streaming { start_time, .. } => {
                write!(f, "Streaming {{ elapsed: {:?} }}", start_time.elapsed())
            }
        }
    }
}

impl VirtualCameraState {
    /// Check if currently streaming
    pub fn is_streaming(&self) -> bool {
        matches!(self, VirtualCameraState::Streaming { .. })
    }

    /// Get the elapsed streaming duration
    pub fn elapsed_duration(&self) -> u64 {
        match self {
            VirtualCameraState::Idle => 0,
            VirtualCameraState::Streaming { start_time, .. } => start_time.elapsed().as_secs(),
        }
    }

    /// Take the stop sender (consumes it)
    pub fn take_stop_sender(&mut self) -> Option<tokio::sync::oneshot::Sender<()>> {
        match self {
            VirtualCameraState::Idle => None,
            VirtualCameraState::Streaming { stop_sender, .. } => stop_sender.take(),
        }
    }

    /// Send a frame to the virtual camera pipeline
    pub fn send_frame(&self, frame: Arc<CameraFrame>) -> bool {
        match self {
            VirtualCameraState::Idle => false,
            VirtualCameraState::Streaming { frame_sender, .. } => frame_sender.send(frame).is_ok(),
        }
    }

    /// Start streaming from camera
    pub fn start(
        stop_sender: tokio::sync::oneshot::Sender<()>,
        frame_sender: tokio::sync::mpsc::UnboundedSender<Arc<CameraFrame>>,
        filter_sender: tokio::sync::watch::Sender<FilterType>,
    ) -> Self {
        VirtualCameraState::Streaming {
            start_time: Instant::now(),
            stop_sender: Some(stop_sender),
            frame_sender,
            filter_sender,
            is_file_source: false,
        }
    }

    /// Start streaming from file source
    pub fn start_file_source(
        stop_sender: tokio::sync::oneshot::Sender<()>,
        frame_sender: tokio::sync::mpsc::UnboundedSender<Arc<CameraFrame>>,
        filter_sender: tokio::sync::watch::Sender<FilterType>,
    ) -> Self {
        VirtualCameraState::Streaming {
            start_time: Instant::now(),
            stop_sender: Some(stop_sender),
            frame_sender,
            filter_sender,
            is_file_source: true,
        }
    }

    /// Check if streaming from a file source
    pub fn is_file_source(&self) -> bool {
        match self {
            VirtualCameraState::Idle => false,
            VirtualCameraState::Streaming { is_file_source, .. } => *is_file_source,
        }
    }

    /// Update the filter for virtual camera streaming
    pub fn set_filter(&self, filter: FilterType) -> bool {
        match self {
            VirtualCameraState::Idle => false,
            VirtualCameraState::Streaming { filter_sender, .. } => {
                filter_sender.send(filter).is_ok()
            }
        }
    }

    /// Stop streaming (returns Idle)
    pub fn stop(&mut self) -> Self {
        std::mem::replace(self, VirtualCameraState::Idle)
    }
}

/// Theatre mode state
///
/// Consolidates theatre mode UI visibility state.
#[derive(Debug, Clone)]
pub struct TheatreState {
    /// Theatre mode enabled
    pub enabled: bool,
    /// UI currently visible
    pub ui_visible: bool,
    /// Last interaction time (for auto-hide)
    pub last_interaction: Option<Instant>,
}

impl Default for TheatreState {
    fn default() -> Self {
        Self {
            enabled: false,
            ui_visible: true,
            last_interaction: None,
        }
    }
}

impl TheatreState {
    /// Enter theatre mode
    pub fn enter(&mut self) {
        self.enabled = true;
        self.ui_visible = true;
        self.last_interaction = Some(Instant::now());
    }

    /// Exit theatre mode
    pub fn exit(&mut self) {
        self.enabled = false;
        self.ui_visible = true;
        self.last_interaction = None;
    }

    /// Show UI (on interaction)
    ///
    /// Returns `true` if a new hide timer should be scheduled (UI was hidden or
    /// interaction time was stale). Returns `false` if interaction was too recent
    /// to warrant a new timer (debouncing).
    pub fn show_ui(&mut self) -> bool {
        if !self.enabled {
            return false;
        }

        let now = Instant::now();

        // Debounce: if UI is already visible and last interaction was very recent,
        // skip the state update entirely to avoid unnecessary re-renders
        if self.ui_visible {
            if let Some(last) = self.last_interaction {
                if now.duration_since(last) < std::time::Duration::from_millis(100) {
                    return false;
                }
            }
        }

        // UI was hidden, or enough time has passed - update state
        self.ui_visible = true;
        self.last_interaction = Some(now);

        // Spawn a new hide timer to reset the countdown
        true
    }

    /// Try to hide UI (only if enough time has passed)
    pub fn try_hide_ui(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(last) = self.last_interaction {
            if last.elapsed() >= std::time::Duration::from_secs(1) {
                self.ui_visible = false;
                return true;
            }
        }
        false
    }
}

/// Burst mode state for multi-frame burst capture
///
/// Tracks the state of burst mode photo capture and processing.
/// Encapsulates all burst mode related state including the async processing
/// communication primitives.
#[derive(Debug)]
pub struct BurstModeState {
    /// Whether night mode is enabled
    pub enabled: bool,
    /// Current processing stage
    pub stage: BurstModeStage,
    /// Progress during Processing stage (0.0 - 1.0)
    /// During Capturing, progress is derived from frame_buffer.len()
    pub processing_progress: f32,
    /// Frame buffer for collecting burst frames (private - use add_frame/take_frames)
    frame_buffer: Vec<Arc<CameraFrame>>,
    /// Target frame count for current capture (set from config at capture start)
    pub target_frame_count: usize,
    /// Shared atomic for processing progress updates (progress * 1000 for 0.1% precision)
    /// Only present during Processing stage
    progress_atomic: Option<Arc<std::sync::atomic::AtomicU32>>,
    /// Channel receiver for processing result
    /// Only present during Processing stage
    result_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

/// Burst mode processing stages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BurstModeStage {
    /// Waiting to start
    #[default]
    Idle,
    /// Capturing burst frames
    Capturing,
    /// Processing frames (aligning, merging, tone mapping)
    Processing,
    /// Processing complete
    Complete,
    /// Error occurred
    Error,
}

impl BurstModeState {
    /// Toggle night mode enabled state, returns new state
    pub fn toggle_enabled(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }

    /// Add a frame to the capture buffer
    ///
    /// Returns `true` if the target frame count has been reached.
    pub fn add_frame(&mut self, frame: Arc<CameraFrame>) -> bool {
        self.frame_buffer.push(frame);
        self.frame_buffer.len() >= self.target_frame_count
    }

    /// Take all frames from the buffer, leaving it empty
    pub fn take_frames(&mut self) -> Vec<Arc<CameraFrame>> {
        std::mem::take(&mut self.frame_buffer)
    }

    /// Check if capture/processing is in progress
    pub fn is_active(&self) -> bool {
        matches!(
            self.stage,
            BurstModeStage::Capturing | BurstModeStage::Processing
        )
    }

    /// Check if we're actively collecting frames (derived from stage)
    pub fn is_collecting_frames(&self) -> bool {
        self.stage == BurstModeStage::Capturing
    }

    /// Number of frames captured so far (derived from buffer)
    pub fn frames_captured(&self) -> usize {
        self.frame_buffer.len()
    }

    /// Get current progress (0.0 - 1.0)
    ///
    /// During Capturing: derived from frame_buffer.len() / target_frame_count
    /// During Processing: from processing_progress field
    /// Complete: 1.0
    /// Other stages: 0.0
    pub fn progress(&self) -> f32 {
        match self.stage {
            BurstModeStage::Capturing => {
                if self.target_frame_count == 0 {
                    0.0
                } else {
                    self.frame_buffer.len() as f32 / self.target_frame_count as f32
                }
            }
            BurstModeStage::Processing => self.processing_progress,
            BurstModeStage::Complete => 1.0,
            _ => 0.0,
        }
    }

    /// Start capture - clears buffer and sets state to Capturing
    pub fn start_capture(&mut self, target_frame_count: usize) {
        self.frame_buffer.clear();
        self.stage = BurstModeStage::Capturing;
        self.processing_progress = 0.0;
        self.target_frame_count = target_frame_count;
    }

    /// Start processing
    pub fn start_processing(&mut self) {
        self.stage = BurstModeStage::Processing;
        self.processing_progress = 0.0;
    }

    /// Mark complete
    pub fn complete(&mut self) {
        self.stage = BurstModeStage::Complete;
    }

    /// Mark error
    pub fn error(&mut self) {
        self.stage = BurstModeStage::Error;
    }

    /// Reset to idle
    pub fn reset(&mut self) {
        self.stage = BurstModeStage::Idle;
        self.processing_progress = 0.0;
        self.frame_buffer.clear();
        self.progress_atomic = None;
        self.result_rx = None;
    }

    /// Start processing and set up communication channels.
    /// Returns the atomic counter that the processing task should update.
    pub fn start_processing_task(
        &mut self,
    ) -> (
        Arc<std::sync::atomic::AtomicU32>,
        std::sync::mpsc::Sender<Result<String, String>>,
    ) {
        self.stage = BurstModeStage::Processing;
        self.processing_progress = 0.0;

        // Create shared atomic for progress updates (progress * 1000 for 0.1% precision)
        let progress_atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        self.progress_atomic = Some(Arc::clone(&progress_atomic));

        // Create channel for result
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        self.result_rx = Some(result_rx);

        (progress_atomic, result_tx)
    }

    /// Poll progress from the processing task.
    /// Updates internal progress and returns true if still processing.
    pub fn poll_progress(&mut self) -> bool {
        if self.stage != BurstModeStage::Processing {
            return false;
        }

        // Update progress from atomic
        if let Some(atomic) = &self.progress_atomic {
            let progress_raw = atomic.load(std::sync::atomic::Ordering::Relaxed);
            self.processing_progress = progress_raw as f32 / 1000.0;
        }

        true
    }

    /// Try to get the processing result.
    /// Returns Some(result) if complete, None if still processing or not in processing state.
    pub fn try_get_result(&mut self) -> Option<Result<String, String>> {
        if let Some(rx) = &self.result_rx {
            match rx.try_recv() {
                Ok(result) => {
                    // Clear processing state
                    self.progress_atomic = None;
                    self.result_rx = None;
                    Some(result)
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // Channel closed unexpectedly
                    self.progress_atomic = None;
                    self.result_rx = None;
                    Some(Err("Processing task terminated unexpectedly".to_string()))
                }
            }
        } else {
            None
        }
    }

    /// Clear processing state (called when not in Processing stage)
    pub fn clear_processing_state(&mut self) {
        self.progress_atomic = None;
        self.result_rx = None;
    }
}

impl Default for BurstModeState {
    fn default() -> Self {
        Self {
            enabled: false,
            stage: BurstModeStage::default(),
            processing_progress: 0.0,
            frame_buffer: Vec::new(),
            target_frame_count: 8, // Will be overwritten when capture starts
            progress_atomic: None,
            result_rx: None,
        }
    }
}

/// Kinect device state
///
/// Consolidates all Kinect-specific state including device detection,
/// motor control, calibration, and native backend streaming.
#[derive(Default)]
pub struct KinectState {
    /// Whether the current camera is a Kinect device
    pub is_device: bool,
    /// Current tilt angle (see freedepth::TILT_MIN_DEGREES/TILT_MAX_DEGREES)
    pub tilt_angle: i8,
    /// Path to the Kinect depth device (found when 3D mode enabled on RGB)
    pub depth_device_path: Option<String>,
    /// Native depth camera backend for simultaneous RGB+depth streaming
    pub native_backend: Option<crate::backends::camera::NativeDepthBackend>,
    /// Whether native Kinect streaming is active
    pub streaming: bool,
    /// Current calibration info from depth device (for display)
    /// Uses generic RegistrationSummary for device-agnostic access
    #[cfg(all(target_arch = "x86_64", feature = "freedepth"))]
    pub calibration_info: Option<freedepth::RegistrationSummary>,
    #[cfg(not(all(target_arch = "x86_64", feature = "freedepth")))]
    pub calibration_info: Option<()>,
    /// Whether calibration dialog is visible
    pub calibration_dialog_visible: bool,
    /// Registration data for depth-to-RGB alignment (used by scene capture)
    pub registration_data: Option<crate::pipelines::scene::RegistrationData>,
}

impl KinectState {
    /// Check if native backend should be shut down
    pub fn shutdown_backend(&mut self) {
        if self.native_backend.is_some() {
            tracing::info!("KinectState: shutting down native Kinect backend");
            self.native_backend = None; // Drop will handle LED cleanup
            self.streaming = false;
        }
    }
}

/// 3D preview state for depth camera visualization
///
/// Consolidates all state related to 3D point cloud/mesh rendering
/// including rotation, zoom, and render caching.
#[derive(Clone)]
pub struct Preview3DState {
    /// Whether to show 3D preview (for depth cameras)
    pub enabled: bool,
    /// Scene view mode (point cloud or mesh)
    pub view_mode: SceneViewMode,
    /// Current rotation angles (pitch, yaw in radians)
    pub rotation: (f32, f32),
    /// Base rotation when drag started (for path independence)
    pub base_rotation: (f32, f32),
    /// Whether the mouse is currently dragging to rotate
    pub dragging: bool,
    /// Mouse position when drag started
    pub drag_start_pos: Option<(f32, f32)>,
    /// Last mouse position during drag
    pub last_mouse_pos: Option<(f32, f32)>,
    /// Zoom level (1.0 = default, higher = closer)
    pub zoom: f32,
    /// Rendered point cloud RGBA data (width, height, data)
    pub rendered_preview: Option<(u32, u32, Arc<Vec<u8>>)>,
    /// Most recent depth data (width, height, depth_u16_data)
    pub latest_depth_data: Option<(u32, u32, Arc<[u16]>)>,
    /// Last video frame timestamp rendered in scene view
    pub last_render_video_timestamp: Option<u32>,
    /// Last time a scene render was requested (for throttling)
    pub last_render_request_time: Option<Instant>,
}

impl Default for Preview3DState {
    fn default() -> Self {
        Self {
            enabled: false,
            view_mode: SceneViewMode::default(),
            rotation: (0.0, 0.0),
            base_rotation: (0.0, 0.0),
            dragging: false,
            drag_start_pos: None,
            last_mouse_pos: None,
            zoom: 1.0,
            rendered_preview: None,
            latest_depth_data: None,
            last_render_video_timestamp: None,
            last_render_request_time: None,
        }
    }
}

impl Preview3DState {
    /// Reset rotation to default view
    pub fn reset_rotation(&mut self) {
        self.rotation = (0.0, 0.0);
        self.base_rotation = (0.0, 0.0);
    }

    /// Start drag operation
    pub fn start_drag(&mut self, x: f32, y: f32) {
        self.dragging = true;
        self.drag_start_pos = Some((x, y));
        self.base_rotation = self.rotation;
    }

    /// End drag operation
    pub fn end_drag(&mut self) {
        self.dragging = false;
        self.drag_start_pos = None;
        self.last_mouse_pos = None;
    }
}

/// Depth visualization settings
///
/// Controls how depth data is displayed in the camera preview.
#[derive(Debug, Clone, Default)]
pub struct DepthVisualizationState {
    /// Whether to show depth overlay on camera preview
    pub overlay_enabled: bool,
    /// Whether to use grayscale instead of colormap
    pub grayscale_mode: bool,
}

/// The application model stores app-specific state used to describe its interface and
/// drive its logic.
pub struct AppModel {
    /// Application state which is managed by the COSMIC runtime.
    pub core: cosmic::Core,
    /// Display a context drawer with the designated page if defined.
    pub context_page: ContextPage,
    /// The about page for this app.
    pub about: About,
    /// Configuration data that persists between application runs.
    pub config: Config,
    /// Configuration handler for saving settings
    pub config_handler: Option<cosmic_config::Config>,
    /// Current camera mode (Photo or Video)
    pub mode: CameraMode,
    /// Recording state (idle, recording, or paused)
    pub recording: RecordingState,
    /// Virtual camera state (idle or streaming)
    pub virtual_camera: VirtualCameraState,
    /// File source for virtual camera (image or video to stream instead of camera)
    pub virtual_camera_file_source: Option<FileSource>,
    /// Whether the current frame is from a file source (vs camera)
    pub current_frame_is_file_source: bool,
    /// Video file playback progress (position_secs, duration_secs, progress 0.0-1.0)
    pub video_file_progress: Option<(f64, f64, f64)>,
    /// Video preview seek position (used when not streaming to store desired start position)
    pub video_preview_seek_position: f64,
    /// Whether video file playback is paused
    pub video_file_paused: bool,
    /// Channel to send playback control commands to the streaming thread
    pub video_playback_control_tx: Option<tokio::sync::mpsc::UnboundedSender<VideoPlaybackCommand>>,
    /// Channel to send playback control commands to the preview thread (when not streaming)
    pub video_preview_control_tx: Option<tokio::sync::mpsc::UnboundedSender<VideoPlaybackCommand>>,
    /// Stop sender for preview playback thread
    pub video_preview_stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Receiver for preview frames from file source streaming
    pub file_source_preview_receiver: Option<
        std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Arc<CameraFrame>>>>,
    >,
    /// Whether a photo capture is in progress
    pub is_capturing: bool,
    /// Whether the format picker is visible (iOS-style popup)
    pub format_picker_visible: bool,
    /// Whether the exposure picker is visible (iOS-style popup)
    pub exposure_picker_visible: bool,
    /// Whether the color picker is visible (iOS-style popup)
    pub color_picker_visible: bool,
    /// Whether the tools menu is visible (iOS-style popup)
    pub tools_menu_visible: bool,

    // ===== Motor/PTZ Controls =====
    /// Whether motor controls picker is visible
    pub motor_picker_visible: bool,

    /// Current exposure settings for active camera
    pub exposure_settings: Option<ExposureSettings>,
    /// Current color/image adjustment settings for active camera
    pub color_settings: Option<ColorSettings>,
    /// Available exposure controls for current camera (queried from V4L2)
    pub available_exposure_controls: AvailableExposureControls,
    /// Segmented button model for exposure mode (Auto/Manual)
    pub exposure_mode_model: cosmic::widget::segmented_button::SingleSelectModel,
    /// Base exposure time (in 100µs units) captured when entering manual mode
    /// Used to calculate EV-based adjustments in non-advanced mode
    pub base_exposure_time: Option<i32>,
    /// Theatre mode state (enabled, UI visibility, auto-hide)
    pub theatre: TheatreState,
    /// Burst mode state (enabled, capture/processing progress)
    pub burst_mode: BurstModeState,
    /// Currently selected filter
    pub selected_filter: FilterType,
    /// Flash enabled for photo capture
    pub flash_enabled: bool,
    /// Flash is currently active (showing white overlay)
    pub flash_active: bool,
    /// Photo timer setting (off, 3s, 5s, 10s)
    pub photo_timer_setting: PhotoTimerSetting,
    /// Photo timer countdown (remaining seconds, None when not counting)
    pub photo_timer_countdown: Option<u8>,
    /// When the current countdown second started (for fade animation)
    pub photo_timer_tick_start: Option<Instant>,
    /// Photo aspect ratio (native, 4:3, 16:9, 1:1)
    pub photo_aspect_ratio: PhotoAspectRatio,
    /// Current zoom level (1.0 = no zoom, 2.0 = 2x zoom, etc.)
    pub zoom_level: f32,
    /// Path to last generated bug report
    pub last_bug_report_path: Option<String>,
    /// Latest gallery thumbnail (cached)
    pub gallery_thumbnail: Option<cosmic::widget::image::Handle>,
    /// Gallery thumbnail RGBA data for custom rendering (Arc for cheap cloning)
    pub gallery_thumbnail_rgba: Option<(Arc<Vec<u8>>, u32, u32)>,
    /// Currently selected resolution in the picker (width for grouping)
    pub picker_selected_resolution: Option<u32>,
    /// Camera backend manager (PipeWire)
    pub backend_manager: Option<CameraBackendManager>,
    /// Flag to cancel camera subscription (used when switching backends/cameras)
    pub camera_cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Current camera frame
    pub current_frame: Option<Arc<CameraFrame>>,
    /// Available camera devices
    pub available_cameras: Vec<CameraDevice>,
    /// Current camera index
    pub current_camera_index: usize,
    /// Available formats for current camera
    pub available_formats: Vec<CameraFormat>,
    /// Currently active format being used by camera
    pub active_format: Option<CameraFormat>,
    /// Available audio input devices
    pub available_audio_devices: Vec<AudioDevice>,
    /// Current audio device index
    pub current_audio_device_index: usize,
    /// Available video encoders
    pub available_video_encoders: Vec<EncoderInfo>,
    /// Current video encoder index
    pub current_video_encoder_index: usize,
    /// Cached mode information (for consolidated dropdown)
    pub mode_list: Vec<crate::backends::camera::types::CameraFormat>,
    /// Dropdown options (cached for UI)
    pub camera_dropdown_options: Vec<String>,
    pub audio_dropdown_options: Vec<String>,
    pub video_encoder_dropdown_options: Vec<String>,
    pub mode_dropdown_options: Vec<String>,
    pub pixel_format_dropdown_options: Vec<String>,
    pub resolution_dropdown_options: Vec<String>,
    pub framerate_dropdown_options: Vec<String>,
    pub codec_dropdown_options: Vec<String>,
    /// Bitrate preset dropdown options
    pub bitrate_preset_dropdown_options: Vec<String>,
    /// Theme dropdown options (Match Desktop, Dark, Light)
    pub theme_dropdown_options: Vec<String>,
    /// Burst mode merge mode dropdown options (Quality FFT, Fast Spatial)
    pub burst_mode_merge_dropdown_options: Vec<String>,
    /// Burst mode frame count dropdown options (Auto, 4, 6, 8 frames)
    pub burst_mode_frame_count_dropdown_options: Vec<String>,
    /// Photo output format dropdown options (JPEG, PNG, DNG)
    pub photo_output_format_dropdown_options: Vec<String>,
    /// Whether the device info panel is visible
    pub device_info_visible: bool,

    /// Transition state for camera/settings changes
    pub transition_state: TransitionState,

    // ===== QR Code Detection =====
    /// Whether QR code detection is enabled
    pub qr_detection_enabled: bool,
    /// Current QR code detections (updated at 1 FPS)
    pub qr_detections: Vec<QrDetection>,
    /// Last time QR detection was processed
    pub last_qr_detection_time: Option<Instant>,

    // ===== Privacy Cover Detection =====
    /// Whether the camera privacy cover is closed (blocking the camera)
    pub privacy_cover_closed: bool,

    // ===== Kinect State =====
    /// Kinect device state (detection, motor, calibration, native streaming)
    pub kinect: KinectState,

    // ===== Depth Visualization =====
    /// Depth visualization settings (overlay, grayscale mode)
    pub depth_viz: DepthVisualizationState,

    // ===== 3D Preview =====
    /// 3D preview state (rotation, zoom, rendering)
    pub preview_3d: Preview3DState,
}

impl Drop for AppModel {
    fn drop(&mut self) {
        // Ensure Kinect native backend is shut down (LED will be turned off)
        self.kinect.shutdown_backend();
    }
}

/// State for smooth blur transitions when changing camera settings
#[derive(Debug, Clone, Default)]
pub struct TransitionState {
    /// Whether we're currently in a transition (blur is active)
    pub in_transition: bool,
    /// Timestamp when transition started (for detecting camera restart)
    pub transition_start_time: Option<Instant>,
    /// Timestamp when first new frame arrived (for 1-second blur countdown)
    pub first_frame_time: Option<Instant>,
    /// Whether UI should be disabled during transition (to prevent user interaction)
    pub ui_disabled: bool,
}

/// Camera modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraMode {
    Photo,
    Video,
    /// Virtual camera mode - streams filtered video to a PipeWire virtual camera
    Virtual,
    /// Scene mode - 3D point cloud view for depth cameras (Kinect, RealSense, etc.)
    Scene,
}

/// Scene mode view type (point cloud vs mesh)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneViewMode {
    /// Point cloud - individual points rendered
    PointCloud,
    /// Mesh - triangulated surface with RGB texture (default)
    #[default]
    Mesh,
}

/// File source for virtual camera streaming
///
/// When set, the virtual camera streams from this file instead of the camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSource {
    /// Stream from an image file (static frame)
    Image(std::path::PathBuf),
    /// Stream from a video file (loops automatically, no audio)
    Video(std::path::PathBuf),
}

/// Application initialization flags
///
/// These are passed from the command line to configure the app on startup.
#[derive(Debug, Clone, Default)]
pub struct AppFlags {
    /// Optional file to use as the camera preview source instead of a real camera.
    /// Can be an image (PNG, JPG, JPEG, WEBP) or video (MP4, WEBM, MKV).
    pub preview_source: Option<std::path::PathBuf>,
}

/// Commands for controlling video file playback
#[derive(Debug, Clone)]
pub enum VideoPlaybackCommand {
    /// Seek to a specific position in seconds
    Seek(f64),
    /// Toggle play/pause
    TogglePause,
    /// Set paused state explicitly
    SetPaused(bool),
}

/// Photo timer settings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhotoTimerSetting {
    /// No timer (immediate capture)
    #[default]
    Off,
    /// 3 second countdown
    Sec3,
    /// 5 second countdown
    Sec5,
    /// 10 second countdown
    Sec10,
}

impl PhotoTimerSetting {
    /// Get the number of seconds for this setting
    pub fn seconds(&self) -> u8 {
        match self {
            PhotoTimerSetting::Off => 0,
            PhotoTimerSetting::Sec3 => 3,
            PhotoTimerSetting::Sec5 => 5,
            PhotoTimerSetting::Sec10 => 10,
        }
    }

    /// Cycle to next setting: Off -> 3s -> 5s -> 10s -> Off
    pub fn next(&self) -> Self {
        match self {
            PhotoTimerSetting::Off => PhotoTimerSetting::Sec3,
            PhotoTimerSetting::Sec3 => PhotoTimerSetting::Sec5,
            PhotoTimerSetting::Sec5 => PhotoTimerSetting::Sec10,
            PhotoTimerSetting::Sec10 => PhotoTimerSetting::Off,
        }
    }
}

/// Photo aspect ratio settings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhotoAspectRatio {
    /// Native camera aspect ratio (no cropping)
    #[default]
    Native,
    /// 4:3 aspect ratio
    Ratio4x3,
    /// 16:9 aspect ratio
    Ratio16x9,
    /// 1:1 square aspect ratio
    Ratio1x1,
}

impl PhotoAspectRatio {
    /// Tolerance for aspect ratio matching (allows for minor pixel rounding differences)
    const RATIO_TOLERANCE: f32 = 0.02;

    /// Detect which defined aspect ratio matches the given frame dimensions
    /// Returns None if the native ratio doesn't match any defined ratio
    pub fn from_frame_dimensions(width: u32, height: u32) -> Option<Self> {
        if height == 0 {
            return None;
        }
        let frame_ratio = width as f32 / height as f32;

        // Check each defined ratio
        if (frame_ratio - 4.0 / 3.0).abs() < Self::RATIO_TOLERANCE {
            Some(PhotoAspectRatio::Ratio4x3)
        } else if (frame_ratio - 16.0 / 9.0).abs() < Self::RATIO_TOLERANCE {
            Some(PhotoAspectRatio::Ratio16x9)
        } else if (frame_ratio - 1.0).abs() < Self::RATIO_TOLERANCE {
            Some(PhotoAspectRatio::Ratio1x1)
        } else {
            None
        }
    }

    /// Get the default aspect ratio for given frame dimensions
    /// If native matches a defined ratio, use that; otherwise use Native
    pub fn default_for_frame(width: u32, height: u32) -> Self {
        Self::from_frame_dimensions(width, height).unwrap_or(PhotoAspectRatio::Native)
    }

    /// Cycle to next aspect ratio, skipping Native if it matches a defined ratio
    pub fn next_for_frame(&self, frame_width: u32, frame_height: u32) -> Self {
        let native_matches_defined =
            Self::from_frame_dimensions(frame_width, frame_height).is_some();

        let next = match self {
            PhotoAspectRatio::Native => PhotoAspectRatio::Ratio4x3,
            PhotoAspectRatio::Ratio4x3 => PhotoAspectRatio::Ratio16x9,
            PhotoAspectRatio::Ratio16x9 => PhotoAspectRatio::Ratio1x1,
            PhotoAspectRatio::Ratio1x1 => {
                if native_matches_defined {
                    // Skip Native, go directly to 4:3
                    PhotoAspectRatio::Ratio4x3
                } else {
                    PhotoAspectRatio::Native
                }
            }
        };

        next
    }

    /// Get the aspect ratio as a float (width / height), or None for native
    pub fn ratio(&self) -> Option<f32> {
        match self {
            PhotoAspectRatio::Native => None,
            PhotoAspectRatio::Ratio4x3 => Some(4.0 / 3.0),
            PhotoAspectRatio::Ratio16x9 => Some(16.0 / 9.0),
            PhotoAspectRatio::Ratio1x1 => Some(1.0),
        }
    }

    /// Calculate crop rectangle for a given frame size
    /// Returns (x, y, width, height) for the crop region
    pub fn crop_rect(&self, frame_width: u32, frame_height: u32) -> (u32, u32, u32, u32) {
        let Some(target_ratio) = self.ratio() else {
            return (0, 0, frame_width, frame_height);
        };

        let frame_ratio = frame_width as f32 / frame_height as f32;

        if frame_ratio > target_ratio {
            // Frame is wider than target - crop sides
            let new_width = (frame_height as f32 * target_ratio) as u32;
            let x = (frame_width - new_width) / 2;
            (x, 0, new_width, frame_height)
        } else {
            // Frame is taller than target - crop top/bottom
            let new_height = (frame_width as f32 / target_ratio) as u32;
            let y = (frame_height - new_height) / 2;
            (0, y, frame_width, new_height)
        }
    }

    /// Calculate crop UV coordinates for shader use
    /// Returns (u_min, v_min, u_max, v_max) in 0-1 range
    pub fn crop_uv(&self, frame_width: u32, frame_height: u32) -> Option<(f32, f32, f32, f32)> {
        let Some(target_ratio) = self.ratio() else {
            return None; // Native - no cropping
        };

        let frame_ratio = frame_width as f32 / frame_height as f32;

        if frame_ratio > target_ratio {
            // Frame is wider than target - crop sides
            let scale = target_ratio / frame_ratio;
            let offset = (1.0 - scale) / 2.0;
            Some((offset, 0.0, 1.0 - offset, 1.0))
        } else {
            // Frame is taller than target - crop top/bottom
            let scale = frame_ratio / target_ratio;
            let offset = (1.0 - scale) / 2.0;
            Some((0.0, offset, 1.0, 1.0 - offset))
        }
    }
}

/// Filter types for camera preview
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterType {
    /// No filter applied (displays as "ORIGINAL")
    #[default]
    Standard,
    /// Black & white / monochrome filter
    Mono,
    /// Sepia tone filter (warm brownish tint)
    Sepia,
    /// Noir filter (high contrast black & white)
    Noir,
    /// Vivid - boosted saturation and contrast
    Vivid,
    /// Cool - blue color temperature shift
    Cool,
    /// Warm - orange/amber color temperature
    Warm,
    /// Fade - lifted blacks with muted colors
    Fade,
    /// Duotone - two-color gradient mapping
    Duotone,
    /// Vignette - darkened edges
    Vignette,
    /// Negative - inverted colors
    Negative,
    /// Posterize - reduced color levels (pop-art)
    Posterize,
    /// Solarize - partially inverted tones
    Solarize,
    /// Chromatic Aberration - RGB channel split
    ChromaticAberration,
    /// Pencil - pencil sketch drawing
    Pencil,
}

/// The context page to display in the context drawer.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    Settings,
    Filters,
}

/// Messages emitted by the application and its widgets.
///
/// Messages are organized into logical groups:
/// - **UI Navigation**: Toggle context pages, pickers, and UI states
/// - **Camera Control**: Camera selection, frames, transitions
/// - **Format Selection**: Resolution, framerate, codec, format picker
/// - **Capture Operations**: Photo capture, video recording
/// - **Gallery**: Thumbnail loading, gallery opening
/// - **Filters**: Filter selection and picker
/// - **Settings**: Configuration, audio/video encoder selection
/// - **System**: Bug reports, recovery, external URLs
#[derive(Debug, Clone)]
pub enum Message {
    // ===== UI Navigation =====
    /// Open external URL (repository, etc.)
    LaunchUrl(String),
    /// Toggle context drawer page (About, Settings)
    ToggleContextPage(ContextPage),
    /// Toggle format picker visibility
    ToggleFormatPicker,
    /// Close format picker
    CloseFormatPicker,
    /// Toggle theatre mode
    ToggleTheatreMode,
    /// Show UI in theatre mode (after user interaction)
    TheatreShowUI,
    /// Hide UI in theatre mode (auto-hide timer)
    TheatreHideUI,
    /// Toggle device info panel visibility
    ToggleDeviceInfo,

    // ===== Tools Menu =====
    /// Toggle tools menu visibility
    ToggleToolsMenu,
    /// Close tools menu (click outside)
    CloseToolsMenu,

    // ===== Exposure Controls =====
    /// Toggle exposure picker visibility
    ToggleExposurePicker,
    /// Close exposure picker (click outside)
    CloseExposurePicker,
    /// Set exposure mode (Auto, Manual, Shutter Priority, Aperture Priority)
    SetExposureMode(ExposureMode),
    /// Set exposure compensation (EV bias) - value in 0.001 EV units
    SetExposureCompensation(i32),
    /// Reset exposure compensation to 0 and return to aperture priority mode
    ResetExposureCompensation,
    /// Set exposure time (100us units, only in manual mode)
    SetExposureTime(i32),
    /// Set gain value
    SetGain(i32),
    /// Set ISO sensitivity
    SetIsoSensitivity(i32),
    /// Set metering mode
    SetMeteringMode(MeteringMode),
    /// Toggle auto exposure priority (allow frame rate variation)
    ToggleAutoExposurePriority,
    /// Exposure controls queried from camera
    ExposureControlsQueried(AvailableExposureControls, ExposureSettings, ColorSettings),
    /// Exposure control change applied successfully
    ExposureControlApplied,
    /// White balance toggled, with optional temperature value when switching to manual
    WhiteBalanceToggled(Option<i32>),
    /// Exposure control change failed
    ExposureControlFailed(String),
    /// Base exposure time captured (for non-advanced EV slider)
    ExposureBaseTimeCaptured(i32),
    /// Set backlight compensation value
    SetBacklightCompensation(i32),
    /// Reset all exposure settings to defaults
    ResetExposureSettings,
    /// Exposure mode selected via segmented button
    ExposureModeSelected(cosmic::widget::segmented_button::Entity),

    // ===== Color Controls =====
    /// Toggle color picker visibility
    ToggleColorPicker,
    /// Close color picker (click outside)
    CloseColorPicker,
    /// Set contrast value
    SetContrast(i32),
    /// Set saturation value
    SetSaturation(i32),
    /// Set sharpness value
    SetSharpness(i32),
    /// Set hue value
    SetHue(i32),
    /// Toggle auto white balance
    ToggleAutoWhiteBalance,
    /// Set white balance temperature (Kelvin)
    SetWhiteBalanceTemperature(i32),
    /// Reset all color settings to defaults
    ResetColorSettings,

    // ===== Camera Control =====
    /// Switch to next camera
    SwitchCamera,
    /// Select specific camera by index
    SelectCamera(usize),
    /// New camera frame received from pipeline
    CameraFrame(Arc<CameraFrame>),
    /// Cameras initialized asynchronously during startup
    CamerasInitialized(
        Vec<crate::backends::camera::types::CameraDevice>,
        usize,
        Vec<crate::backends::camera::types::CameraFormat>,
    ),
    /// Camera list changed (hotplug event)
    CameraListChanged(Vec<crate::backends::camera::types::CameraDevice>),
    /// Start camera transition (capture last frame and show blur)
    StartCameraTransition,
    /// Clear blur transition after delay
    ClearTransitionBlur,
    /// Toggle mirror preview (horizontal flip)
    ToggleMirrorPreview,

    // ===== Motor/PTZ Controls =====
    /// Toggle motor controls picker visibility
    ToggleMotorPicker,
    /// Close motor controls picker
    CloseMotorPicker,
    /// Set V4L2 pan absolute position
    SetPanAbsolute(i32),
    /// Set V4L2 tilt absolute position (V4L2 cameras)
    SetTiltAbsolute(i32),
    /// Set V4L2 zoom absolute position
    SetZoomAbsolute(i32),
    /// Reset pan/tilt to center position
    ResetPanTilt,

    // ===== Depth Camera Controls =====
    /// Set depth camera tilt angle (see freedepth::TILT_MIN_DEGREES/TILT_MAX_DEGREES)
    SetKinectTilt(i8),
    /// Kinect state updated (tilt) - response from async operation
    KinectStateUpdated(i8),
    /// Kinect control operation failed
    KinectControlFailed(String),
    /// Kinect controller initialized (called after lazy init when motor picker opens)
    KinectInitialized(Result<i8, String>),

    // ===== Format Selection =====
    /// Switch between Photo/Video mode
    SetMode(CameraMode),
    /// Select mode from dropdown by index
    SelectMode(usize),
    /// Select pixel format from dropdown
    SelectPixelFormat(String),
    /// Select resolution from dropdown
    SelectResolution(String),
    /// Select framerate from dropdown
    SelectFramerate(String),
    /// Select codec from dropdown
    SelectCodec(String),
    /// Select resolution in picker (grouped view)
    PickerSelectResolution(u32),
    /// Select specific format in picker
    PickerSelectFormat(usize),
    /// Select bitrate preset
    SelectBitratePreset(usize),

    // ===== Capture Operations =====
    /// Capture photo
    Capture,
    /// Toggle flash for photo capture
    ToggleFlash,
    /// Toggle burst mode for photo capture (multi-frame HDR+ burst)
    ToggleBurstMode,
    /// Set burst mode frame count (0 = Auto, 1 = 4 frames, 2 = 6 frames, 3 = 8 frames)
    SetBurstModeFrameCount(usize),
    /// Burst mode capture progress update (overall_progress 0.0-1.0)
    BurstModeProgress(f32),
    /// Burst mode frames collected, ready to process
    BurstModeFramesCollected,
    /// Burst mode capture complete (path or error)
    BurstModeComplete(Result<String, String>),
    /// Poll burst mode processing progress (timer-based)
    PollBurstModeProgress,
    /// Reset burst mode state after completion/error
    ResetBurstModeState,
    /// Cycle photo aspect ratio (native -> 4:3 -> 16:9 -> 1:1 -> native)
    CyclePhotoAspectRatio,
    /// Flash duration complete, now capture the photo
    FlashComplete,
    /// Cycle photo timer setting (off -> 3s -> 5s -> 10s -> off)
    CyclePhotoTimer,
    /// Photo timer tick (countdown)
    PhotoTimerTick,
    /// Photo timer animation frame (for fade effect)
    PhotoTimerAnimationFrame,
    /// Abort photo timer countdown
    AbortPhotoTimer,
    /// Zoom in (increase zoom level)
    ZoomIn,
    /// Zoom out (decrease zoom level)
    ZoomOut,
    /// Reset zoom to 1.0
    ResetZoom,
    /// Photo was saved successfully with the given file path
    PhotoSaved(Result<String, String>),
    /// Scene was captured successfully with the scene directory path
    SceneSaved(Result<String, String>),
    /// Clear capture animation after brief delay
    ClearCaptureAnimation,
    /// Toggle video recording
    ToggleRecording,
    /// Video recording started successfully
    RecordingStarted(String),
    /// Video recording stopped successfully
    RecordingStopped(Result<String, String>),
    /// Update recording duration (every second)
    UpdateRecordingDuration,
    /// Start recording after camera is released
    StartRecordingAfterDelay,

    // ===== Virtual Camera =====
    /// Toggle virtual camera streaming (start/stop)
    ToggleVirtualCamera,
    /// Virtual camera streaming started successfully
    VirtualCameraStarted,
    /// Virtual camera streaming stopped
    VirtualCameraStopped(Result<(), String>),
    /// Update virtual camera streaming duration (every second)
    UpdateVirtualCameraDuration,
    /// Open file picker to select image/video for virtual camera
    OpenVirtualCameraFile,
    /// File selected for virtual camera streaming
    VirtualCameraFileSelected(Option<FileSource>),
    /// Clear the virtual camera file source (use camera instead)
    ClearVirtualCameraFile,
    /// File source preview frame loaded (for displaying before streaming starts)
    /// For videos, includes optional duration in seconds
    FileSourcePreviewLoaded(Option<Arc<CameraFrame>>, Option<f64>),
    /// Video file source playback progress update (position_secs, duration_secs, progress 0.0-1.0)
    VideoFileProgress(f64, f64, f64),
    /// Seek video file to a specific position in seconds
    VideoFileSeek(f64),
    /// Preview frame loaded after seeking while not streaming
    VideoSeekPreviewLoaded(Option<Arc<CameraFrame>>),
    /// Preview playback frame update (frame and progress info)
    VideoPreviewPlaybackUpdate(Arc<CameraFrame>, f64, f64, f64),
    /// Preview playback stopped (thread finished)
    VideoPreviewPlaybackStopped,
    /// Toggle video file play/pause
    ToggleVideoPlayPause,
    /// Start video preview playback (triggered after streaming stops)
    StartVideoPreviewPlayback,

    // ===== Gallery =====
    /// Open gallery in file manager
    OpenGallery,
    /// Refresh the gallery thumbnail
    RefreshGalleryThumbnail,
    /// Gallery thumbnail loaded (Handle, RGBA data wrapped in Arc, width, height)
    GalleryThumbnailLoaded(Option<(cosmic::widget::image::Handle, Arc<Vec<u8>>, u32, u32)>),

    // ===== Filters =====
    /// Select a filter
    SelectFilter(FilterType),

    // ===== Settings & Device Selection =====
    /// Configuration updated
    UpdateConfig(Config),
    /// Set application theme (System, Dark, Light)
    SetAppTheme(usize),
    /// Select audio input device
    SelectAudioDevice(usize),
    /// Select video encoder
    SelectVideoEncoder(usize),
    /// Select photo output format (JPEG, PNG, DNG)
    SelectPhotoOutputFormat(usize),
    /// Toggle saving raw burst frames as DNG (debugging feature)
    ToggleSaveBurstRaw,
    /// Toggle virtual camera feature enabled
    ToggleVirtualCameraEnabled,

    // ===== System & Recovery =====
    /// Camera backend recovery started
    CameraRecoveryStarted { attempt: u32, max_attempts: u32 },
    /// Camera backend recovery succeeded
    CameraRecoverySucceeded,
    /// Camera backend recovery failed
    CameraRecoveryFailed(String),
    /// Audio backend recovery started
    AudioRecoveryStarted { attempt: u32, max_attempts: u32 },
    /// Audio backend recovery succeeded
    AudioRecoverySucceeded,
    /// Audio backend recovery failed
    AudioRecoveryFailed(String),
    /// Generate bug report
    GenerateBugReport,
    /// Bug report generated successfully with path
    BugReportGenerated(Result<String, String>),
    /// Show bug report in file manager
    ShowBugReport,

    // ===== QR Code Detection =====
    /// Toggle QR code detection on/off
    ToggleQrDetection,
    /// QR detection results updated
    QrDetectionsUpdated(Vec<QrDetection>),
    /// Open URL from QR code
    QrOpenUrl(String),
    /// Connect to WiFi network from QR code
    QrConnectWifi {
        ssid: String,
        password: Option<String>,
        security: String,
        hidden: bool,
    },
    /// Copy text from QR code to clipboard
    QrCopyText(String),

    // ===== Privacy Cover Detection =====
    /// Privacy cover status changed (true = cover closed/camera blocked)
    PrivacyCoverStatusChanged(bool),

    // ===== Depth Visualization =====
    /// Toggle depth overlay visibility
    ToggleDepthOverlay,
    /// Toggle grayscale depth mode (grayscale instead of colormap)
    ToggleDepthGrayscale,

    // ===== Calibration =====
    /// Show calibration status dialog (explains current calibration state)
    ShowCalibrationDialog,
    /// Close calibration dialog
    CloseCalibrationDialog,
    /// Start calibration procedure
    StartCalibration,

    // ===== 3D Preview =====
    /// Toggle 3D point cloud preview (for depth cameras)
    Toggle3DPreview,
    /// Toggle scene view mode (point cloud vs mesh)
    ToggleSceneViewMode,
    /// 3D preview mouse pressed (start dragging)
    Preview3DMousePressed(f32, f32),
    /// 3D preview mouse moved (update rotation while dragging)
    Preview3DMouseMoved(f32, f32),
    /// 3D preview mouse released (stop dragging)
    Preview3DMouseReleased,
    /// Reset 3D preview rotation to default view
    Reset3DPreviewRotation,
    /// Zoom 3D preview (delta: positive = zoom in, negative = zoom out)
    Zoom3DPreview(f32),
    /// Point cloud preview rendered (width, height, rgba_data)
    PointCloudRendered(u32, u32, Arc<Vec<u8>>),
    /// Request point cloud render (triggered on rotation change while 3D mode active)
    RequestPointCloudRender,
    /// Secondary depth frame received (for 3D preview on RGB camera)
    SecondaryDepthFrame(u32, u32, Arc<[u16]>),

    // ===== Native Kinect Streaming =====
    /// Start native Kinect streaming (bypasses V4L2 for simultaneous RGB+depth)
    StartNativeKinectStreaming,
    /// Stop native Kinect streaming
    StopNativeKinectStreaming,
    /// Native Kinect streaming started successfully
    NativeKinectStreamingStarted,
    /// Native Kinect streaming failed to start
    NativeKinectStreamingFailed(String),
    /// Poll native Kinect backend for new frames
    PollNativeKinectFrames,

    /// No-op message for async tasks that don't need a response
    Noop,
}

impl TransitionState {
    /// Start a transition - enable blur, disable UI, and wait for first frame
    pub fn start(&mut self) -> cosmic::Task<Message> {
        self.in_transition = true;
        self.ui_disabled = true; // Disable UI during transition
        self.transition_start_time = Some(Instant::now());
        self.first_frame_time = None; // Reset - waiting for first new frame

        cosmic::Task::none()
    }

    /// Called when a new frame arrives during transition
    /// Returns a task to clear blur after 1 second if this is the first frame
    pub fn on_frame_received(&mut self) -> Option<cosmic::Task<Message>> {
        if !self.in_transition {
            return None;
        }

        // If this is the first frame since transition started
        if self.first_frame_time.is_none() {
            self.first_frame_time = Some(Instant::now());

            // Schedule blur removal after 1 second from NOW
            return Some(cosmic::Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                },
                |_| Message::ClearTransitionBlur,
            ));
        }

        None
    }

    /// Check if blur should still be active
    pub fn should_blur(&self) -> bool {
        if !self.in_transition {
            return false;
        }

        // If we haven't received a frame yet, keep blurring the old frame
        // (or show black if no old frame exists)
        let Some(first_frame_time) = self.first_frame_time else {
            return true;
        };

        // Once first frame arrives, blur for 1 second
        first_frame_time.elapsed() < std::time::Duration::from_millis(1000)
    }

    /// Clear the blur and end transition
    pub fn clear(&mut self) {
        self.in_transition = false;
        self.ui_disabled = false; // Re-enable UI
        self.transition_start_time = None;
        self.first_frame_time = None;
    }
}
