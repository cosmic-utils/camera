// SPDX-License-Identifier: GPL-3.0-only
//! Burst capture for burst mode
//!
//! Captures multiple frames in rapid succession for temporal noise reduction.
//!
//! Based on HDR+ paper Section 3:
//! "We use an auto-exposure algorithm to select the exposure time and ISO...
//! Short exposures are better for reducing motion blur, but require higher ISO
//! which means higher read noise. Longer exposures allow lower ISO but are
//! more susceptible to motion blur."
//!
//! Adaptive burst sizing based on scene brightness:
//! - Bright scenes (well-lit): fewer frames (4-6), lower ISO benefit is minimal
//! - Medium scenes: standard frames (6-8), good balance
//! - Dark scenes: more frames (8-15), need aggressive noise reduction

use crate::backends::camera::CameraBackendManager;
use crate::backends::camera::types::CameraFrame;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::BurstModeConfig;

/// Internal burst capture implementation
///
/// Both public capture functions delegate to this common implementation.
/// Progress callback receives (current_frame, total_frames).
async fn capture_burst_impl<F>(
    backend: &CameraBackendManager,
    frame_count: usize,
    frame_interval: Duration,
    mut progress_callback: F,
) -> Result<Vec<Arc<CameraFrame>>, String>
where
    F: FnMut(usize, usize),
{
    let mut frames = Vec::with_capacity(frame_count);

    for i in 0..frame_count {
        debug!(frame = i + 1, total = frame_count, "Capturing frame");

        let frame = backend
            .capture_photo()
            .map_err(|e| format!("Failed to capture frame {}: {}", i + 1, e))?;

        frames.push(Arc::new(frame));
        progress_callback(i + 1, frame_count);

        if i < frame_count - 1 {
            sleep(frame_interval).await;
        }
    }

    if frames.len() < 2 {
        return Err("Burst capture requires at least 2 frames".to_string());
    }

    Ok(frames)
}

/// Capture a burst of frames from the camera
///
/// # Arguments
/// * `backend` - Camera backend manager
/// * `config` - Burst mode configuration
/// * `progress_callback` - Called after each frame with the count captured so far
///
/// # Returns
/// Vector of captured frames (Arc-wrapped for efficient sharing)
pub async fn capture_burst<F>(
    backend: &CameraBackendManager,
    config: &BurstModeConfig,
    mut progress_callback: F,
) -> Result<Vec<Arc<CameraFrame>>, String>
where
    F: FnMut(usize),
{
    info!(
        frame_count = config.frame_count,
        interval_ms = config.frame_interval_ms,
        "Starting burst capture for burst mode"
    );

    let frame_interval = Duration::from_millis(config.frame_interval_ms as u64);
    let frames = capture_burst_impl(
        backend,
        config.frame_count,
        frame_interval,
        |current, _total| {
            progress_callback(current);
        },
    )
    .await?;

    info!(captured = frames.len(), "Burst capture complete");
    Ok(frames)
}

/// Burst capture configuration validation
pub fn validate_config(config: &BurstModeConfig) -> Result<(), String> {
    if config.frame_count < 2 {
        return Err("Frame count must be at least 2".to_string());
    }
    if config.frame_count > 50 {
        return Err("Frame count must not exceed 50".to_string());
    }
    if config.frame_interval_ms < 10 {
        return Err("Frame interval must be at least 10ms".to_string());
    }
    if config.frame_interval_ms > 500 {
        return Err("Frame interval should not exceed 500ms".to_string());
    }
    Ok(())
}

//=============================================================================
// Adaptive burst sizing based on scene brightness
//
// Based on HDR+ paper Section 3:
// "For low-light scenes we capture more frames and use longer exposures,
// trading off motion blur for noise reduction."
//=============================================================================

/// Scene brightness classification
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneBrightness {
    /// Well-lit scene (e.g., outdoor daylight, bright indoor)
    /// Average luminance > 0.3
    Bright,
    /// Medium lighting (e.g., indoor with lights on, overcast outdoor)
    /// Average luminance 0.1 - 0.3
    Medium,
    /// Low light scene (e.g., evening, dim indoor)
    /// Average luminance 0.03 - 0.1
    Low,
    /// Very dark scene (e.g., night, candlelight)
    /// Average luminance < 0.03
    VeryDark,
}

impl SceneBrightness {
    /// Classify scene brightness based on average luminance
    pub fn from_luminance(avg_luminance: f32) -> Self {
        if avg_luminance > 0.3 {
            SceneBrightness::Bright
        } else if avg_luminance > 0.1 {
            SceneBrightness::Medium
        } else if avg_luminance > 0.03 {
            SceneBrightness::Low
        } else {
            SceneBrightness::VeryDark
        }
    }
}

/// Recommended burst parameters based on scene brightness
#[derive(Debug, Clone)]
pub struct AdaptiveBurstParams {
    /// Number of frames to capture
    pub frame_count: usize,
    /// Robustness parameter for merge algorithm
    pub robustness: f32,
    /// Maximum motion tolerance before rejecting frame
    pub motion_threshold: f32,
}

/// Calculate adaptive burst parameters based on scene brightness
///
/// Based on HDR+ paper recommendations:
/// - Darker scenes benefit from more frames (more noise to average out)
/// - Brighter scenes need fewer frames (less noise, risk of motion blur)
/// - Robustness parameter scales with darkness (more aggressive denoising)
///
/// # Arguments
/// * `brightness` - Scene brightness classification
///
/// # Returns
/// Recommended burst parameters
pub fn calculate_adaptive_params(brightness: SceneBrightness) -> AdaptiveBurstParams {
    match brightness {
        SceneBrightness::Bright => AdaptiveBurstParams {
            frame_count: 4,        // Minimal frames, low noise already
            robustness: 0.5,       // Light denoising
            motion_threshold: 0.2, // Stricter motion rejection
        },
        SceneBrightness::Medium => AdaptiveBurstParams {
            frame_count: 6,  // Standard burst
            robustness: 0.8, // Moderate denoising
            motion_threshold: 0.25,
        },
        SceneBrightness::Low => AdaptiveBurstParams {
            frame_count: 10,       // More frames for low light
            robustness: 1.2,       // Stronger denoising
            motion_threshold: 0.3, // More lenient (motion blur less visible in dark)
        },
        SceneBrightness::VeryDark => AdaptiveBurstParams {
            frame_count: 15,        // Maximum frames
            robustness: 1.5,        // Aggressive denoising
            motion_threshold: 0.35, // Most lenient
        },
    }
}

/// Estimate scene brightness from a single frame
///
/// Computes average luminance from the frame using BT.601 coefficients.
///
/// # Arguments
/// * `frame` - Camera frame to analyze
///
/// # Returns
/// Tuple of (average_luminance, SceneBrightness)
pub fn estimate_scene_brightness(frame: &CameraFrame) -> (f32, SceneBrightness) {
    let pixels = frame.width as usize * frame.height as usize;
    if pixels == 0 {
        warn!("Empty frame for brightness estimation");
        return (0.0, SceneBrightness::VeryDark);
    }

    let mut total_luminance: f64 = 0.0;

    // Sample every Nth pixel for performance (full analysis not needed)
    let sample_stride = (pixels / 10000).max(1); // ~10k samples max

    for i in (0..pixels).step_by(sample_stride) {
        let idx = i * 4; // RGBA format
        if idx + 2 < frame.data.len() {
            let r = frame.data[idx] as f64 / 255.0;
            let g = frame.data[idx + 1] as f64 / 255.0;
            let b = frame.data[idx + 2] as f64 / 255.0;

            // BT.601 luminance
            let lum = 0.299 * r + 0.587 * g + 0.114 * b;
            total_luminance += lum;
        }
    }

    let samples_taken = (pixels + sample_stride - 1) / sample_stride;
    let avg_luminance = (total_luminance / samples_taken as f64) as f32;
    let brightness = SceneBrightness::from_luminance(avg_luminance);

    debug!(
        avg_luminance = avg_luminance,
        brightness = ?brightness,
        samples = samples_taken,
        "Scene brightness estimated"
    );

    (avg_luminance, brightness)
}

/// Capture a burst with adaptive sizing based on scene brightness
///
/// Takes a preview frame to estimate scene brightness, then adjusts
/// burst parameters accordingly.
///
/// # Arguments
/// * `backend` - Camera backend manager
/// * `preview_frame` - Recent preview frame for brightness estimation
/// * `base_config` - Base configuration (can be overridden by adaptive params)
/// * `progress_callback` - Called after each frame with count so far
///
/// # Returns
/// Vector of captured frames
pub async fn capture_burst_adaptive<F>(
    backend: &CameraBackendManager,
    preview_frame: &CameraFrame,
    base_config: &BurstModeConfig,
    progress_callback: F,
) -> Result<(Vec<Arc<CameraFrame>>, AdaptiveBurstParams), String>
where
    F: FnMut(usize, usize), // (current, total)
{
    // Estimate scene brightness from preview
    let (avg_lum, brightness) = estimate_scene_brightness(preview_frame);
    let adaptive_params = calculate_adaptive_params(brightness);

    info!(
        avg_luminance = avg_lum,
        brightness = ?brightness,
        adaptive_frame_count = adaptive_params.frame_count,
        adaptive_robustness = adaptive_params.robustness,
        "Using adaptive burst parameters"
    );

    // Use adaptive frame count, but respect config if user explicitly set it lower
    let frame_count = if base_config.frame_count < adaptive_params.frame_count {
        info!(
            user_limit = base_config.frame_count,
            "User config limits frame count below adaptive recommendation"
        );
        base_config.frame_count
    } else {
        adaptive_params.frame_count
    };

    let frame_interval = Duration::from_millis(base_config.frame_interval_ms as u64);
    let frames =
        capture_burst_impl(backend, frame_count, frame_interval, progress_callback).await?;

    info!(
        captured = frames.len(),
        brightness = ?brightness,
        "Adaptive burst capture complete"
    );

    Ok((frames, adaptive_params))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_config() {
        let mut config = BurstModeConfig::default();
        assert!(validate_config(&config).is_ok());

        config.frame_count = 1;
        assert!(validate_config(&config).is_err());

        config.frame_count = 8;
        config.frame_interval_ms = 5;
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_scene_brightness_classification() {
        // Very dark scene
        assert_eq!(
            SceneBrightness::from_luminance(0.01),
            SceneBrightness::VeryDark
        );

        // Low light scene
        assert_eq!(SceneBrightness::from_luminance(0.05), SceneBrightness::Low);

        // Medium scene
        assert_eq!(
            SceneBrightness::from_luminance(0.2),
            SceneBrightness::Medium
        );

        // Bright scene
        assert_eq!(
            SceneBrightness::from_luminance(0.5),
            SceneBrightness::Bright
        );
    }

    #[test]
    fn test_adaptive_params_scaling() {
        let bright = calculate_adaptive_params(SceneBrightness::Bright);
        let dark = calculate_adaptive_params(SceneBrightness::VeryDark);

        // Darker scenes should capture more frames
        assert!(dark.frame_count > bright.frame_count);

        // Darker scenes should use higher robustness
        assert!(dark.robustness > bright.robustness);

        // Darker scenes should have higher motion threshold (more lenient)
        assert!(dark.motion_threshold > bright.motion_threshold);
    }
}
