// SPDX-License-Identifier: MPL-2.0

//! Application state management

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
#[derive(Debug)]
pub enum RecordingState {
    /// Not recording
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

impl Default for RecordingState {
    fn default() -> Self {
        RecordingState::Idle
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
    pub fn show_ui(&mut self) {
        if self.enabled {
            self.ui_visible = true;
            self.last_interaction = Some(Instant::now());
        }
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
    /// Whether a photo capture is in progress
    pub is_capturing: bool,
    /// Whether the format picker is visible (iOS-style popup)
    pub format_picker_visible: bool,
    /// Theatre mode state (enabled, UI visibility, auto-hide)
    pub theatre: TheatreState,
    /// Filter picker visible
    pub filter_picker_visible: bool,
    /// Currently selected filter
    pub selected_filter: FilterType,
    /// Flash enabled for photo capture
    pub flash_enabled: bool,
    /// Flash is currently active (showing white overlay)
    pub flash_active: bool,
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
    /// Whether the bitrate info matrix is visible
    pub bitrate_info_visible: bool,
    /// Filter picker scroll offset (for programmatic scrolling)
    pub filter_picker_scroll_offset: f32,

    /// Transition state for camera/settings changes
    pub transition_state: TransitionState,
}

/// State for smooth blur transitions when changing camera settings
#[derive(Debug, Clone)]
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
    /// Toggle filter picker visibility
    ToggleFilterPicker,
    /// Close filter picker
    CloseFilterPicker,
    /// Toggle theatre mode
    ToggleTheatreMode,
    /// Show UI in theatre mode (after user interaction)
    TheatreShowUI,
    /// Hide UI in theatre mode (auto-hide timer)
    TheatreHideUI,
    /// Toggle bitrate info matrix visibility
    ToggleBitrateInfo,
    /// Scroll filter picker (horizontal scroll from any scroll input)
    FilterPickerScroll(f32),

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
    /// Flash duration complete, now capture the photo
    FlashComplete,
    /// Photo was saved successfully with the given file path
    PhotoSaved(Result<String, String>),
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
    /// Select audio input device
    SelectAudioDevice(usize),
    /// Select video encoder
    SelectVideoEncoder(usize),

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
}

impl TransitionState {
    pub fn new() -> Self {
        Self {
            in_transition: false,
            transition_start_time: None,
            first_frame_time: None,
            ui_disabled: false,
        }
    }

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

// MenuAction removed - not currently used in the application
// Can be re-added if menu bar functionality is needed
