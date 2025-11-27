// SPDX-License-Identifier: MPL-2.0
// Error types prepared for future unified error handling
#![allow(dead_code)]

//! Error types for the camera application

use std::fmt;

/// Result type alias using AppError
pub type AppResult<T> = Result<T, AppError>;

/// Main application error type
#[derive(Debug, Clone)]
pub enum AppError {
    /// Camera-related errors
    Camera(CameraError),
    /// Recording-related errors
    Recording(RecordingError),
    /// Photo capture errors
    Photo(PhotoError),
    /// Configuration errors
    Config(String),
    /// Storage/filesystem errors
    Storage(String),
    /// Generic error with message
    Other(String),
}

/// Camera-specific errors
#[derive(Debug, Clone)]
pub enum CameraError {
    /// No camera devices found
    NoCameraFound,
    /// Camera initialization failed
    InitializationFailed(String),
    /// Camera disconnected during operation
    Disconnected,
    /// Invalid camera format
    InvalidFormat(String),
    /// Backend error (e.g., PipeWire)
    BackendError(String),
    /// Camera is busy or in use
    Busy,
}

/// Recording-specific errors
#[derive(Debug, Clone)]
pub enum RecordingError {
    /// Failed to start recording
    StartFailed(String),
    /// Failed to stop recording
    StopFailed(String),
    /// Encoder not available
    EncoderNotAvailable(String),
    /// No audio device available
    NoAudioDevice,
    /// Recording already in progress
    AlreadyRecording,
    /// Pipeline error during recording
    PipelineError(String),
}

/// Photo capture errors
#[derive(Debug, Clone)]
pub enum PhotoError {
    /// No frame available for capture
    NoFrameAvailable,
    /// Capture failed
    CaptureFailed(String),
    /// Encoding failed
    EncodingFailed(String),
    /// Save failed
    SaveFailed(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Camera(e) => write!(f, "Camera error: {}", e),
            AppError::Recording(e) => write!(f, "Recording error: {}", e),
            AppError::Photo(e) => write!(f, "Photo error: {}", e),
            AppError::Config(msg) => write!(f, "Configuration error: {}", msg),
            AppError::Storage(msg) => write!(f, "Storage error: {}", msg),
            AppError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CameraError::NoCameraFound => write!(f, "No camera devices found"),
            CameraError::InitializationFailed(msg) => write!(f, "Initialization failed: {}", msg),
            CameraError::Disconnected => write!(f, "Camera disconnected"),
            CameraError::InvalidFormat(msg) => write!(f, "Invalid format: {}", msg),
            CameraError::BackendError(msg) => write!(f, "Backend error: {}", msg),
            CameraError::Busy => write!(f, "Camera is busy"),
        }
    }
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordingError::StartFailed(msg) => write!(f, "Failed to start recording: {}", msg),
            RecordingError::StopFailed(msg) => write!(f, "Failed to stop recording: {}", msg),
            RecordingError::EncoderNotAvailable(msg) => write!(f, "Encoder not available: {}", msg),
            RecordingError::NoAudioDevice => write!(f, "No audio device available"),
            RecordingError::AlreadyRecording => write!(f, "Recording already in progress"),
            RecordingError::PipelineError(msg) => write!(f, "Pipeline error: {}", msg),
        }
    }
}

impl fmt::Display for PhotoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhotoError::NoFrameAvailable => write!(f, "No frame available for capture"),
            PhotoError::CaptureFailed(msg) => write!(f, "Capture failed: {}", msg),
            PhotoError::EncodingFailed(msg) => write!(f, "Encoding failed: {}", msg),
            PhotoError::SaveFailed(msg) => write!(f, "Save failed: {}", msg),
        }
    }
}

impl std::error::Error for AppError {}
impl std::error::Error for CameraError {}
impl std::error::Error for RecordingError {}
impl std::error::Error for PhotoError {}

// Conversions from sub-errors to AppError
impl From<CameraError> for AppError {
    fn from(err: CameraError) -> Self {
        AppError::Camera(err)
    }
}

impl From<RecordingError> for AppError {
    fn from(err: RecordingError) -> Self {
        AppError::Recording(err)
    }
}

impl From<PhotoError> for AppError {
    fn from(err: PhotoError) -> Self {
        AppError::Photo(err)
    }
}

// Conversion from String for backward compatibility
impl From<String> for AppError {
    fn from(msg: String) -> Self {
        AppError::Other(msg)
    }
}

impl From<&str> for AppError {
    fn from(msg: &str) -> Self {
        AppError::Other(msg.to_string())
    }
}

// Conversions for I/O errors
impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Storage(err.to_string())
    }
}

impl From<std::io::Error> for PhotoError {
    fn from(err: std::io::Error) -> Self {
        PhotoError::SaveFailed(err.to_string())
    }
}
