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
use crate::backends::camera::v4l2_controls::ExposureMetadata;
use crate::shaders::{BrightnessMetrics as GpuBrightnessMetrics, analyze_brightness_gpu};
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
///
/// Based on HDR+ paper (Hasinoff et al.):
/// - "In bright scenes, capturing 1-2 images is usually sufficient"
/// - "In practice, we limit our bursts to 2-8 images"
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneBrightness {
    /// Very bright scene (e.g., direct sunlight, well-exposed outdoor)
    /// Average luminance > 0.5 - no HDR+ benefit, single frame sufficient
    VeryBright,
    /// Well-lit scene (e.g., outdoor daylight, bright indoor)
    /// Average luminance 0.3 - 0.5
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
    ///
    /// Thresholds based on HDR+ paper recommendations for frame count selection
    pub fn from_luminance(avg_luminance: f32) -> Self {
        if avg_luminance > 0.5 {
            SceneBrightness::VeryBright
        } else if avg_luminance > 0.3 {
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
    /// Shadow boost strength for tone mapping (0.0 - 1.0)
    /// Higher values lift shadows more aggressively (for dark scenes)
    pub shadow_boost: f32,
    /// Local contrast enhancement strength (0.0 - 1.0)
    /// Higher values add more local contrast (for dark scenes)
    pub local_contrast: f32,
}

/// Calculate adaptive burst parameters based on scene brightness
///
/// Based on HDR+ paper (Hasinoff et al.) recommendations:
/// - "In bright scenes, capturing 1-2 images is usually sufficient"
/// - "In practice, we limit our bursts to 2-8 images"
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
        SceneBrightness::VeryBright => AdaptiveBurstParams {
            frame_count: 1,        // No HDR+ needed - single frame sufficient
            robustness: 0.0,       // Not used for single frame
            motion_threshold: 0.0, // Not used for single frame
            shadow_boost: 0.0,     // No shadow boost needed - scene is bright
            local_contrast: 0.0,   // No enhancement needed
        },
        SceneBrightness::Bright => AdaptiveBurstParams {
            frame_count: 2,         // "1-2 images usually sufficient" per HDR+ paper
            robustness: 0.3,        // Very light denoising
            motion_threshold: 0.15, // Strictest motion rejection
            shadow_boost: 0.1,      // Minimal shadow lift
            local_contrast: 0.1,    // Minimal contrast enhancement
        },
        SceneBrightness::Medium => AdaptiveBurstParams {
            frame_count: 4,  // Standard burst
            robustness: 0.6, // Light denoising
            motion_threshold: 0.2,
            shadow_boost: 0.2,    // Default shadow boost
            local_contrast: 0.15, // Default contrast enhancement
        },
        SceneBrightness::Low => AdaptiveBurstParams {
            frame_count: 6,         // More frames for low light
            robustness: 1.0,        // Moderate denoising
            motion_threshold: 0.25, // More lenient (motion blur less visible in dark)
            shadow_boost: 0.4,      // Moderate shadow recovery
            local_contrast: 0.2,    // Moderate contrast enhancement
        },
        SceneBrightness::VeryDark => AdaptiveBurstParams {
            frame_count: 8,         // Maximum per HDR+ paper (2-8 range)
            robustness: 1.5,        // Aggressive denoising
            motion_threshold: 0.35, // Most lenient
            shadow_boost: 0.7,      // Strong shadow recovery for night mode
            local_contrast: 0.3,    // Strong local contrast for dark scenes
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

/// Multi-source brightness analysis combining GPU histogram, V4L2 exposure, and luminance
#[derive(Debug, Clone)]
pub struct MultiMetricBrightness {
    /// Simple luminance average (fast, always available)
    pub luminance_avg: f32,
    /// GPU histogram metrics (more accurate, may fail on some systems)
    pub histogram: Option<GpuBrightnessMetrics>,
    /// V4L2 exposure metadata (camera settings, if available)
    pub exposure: Option<ExposureMetadata>,
    /// Final fused brightness classification
    pub classification: SceneBrightness,
    /// Confidence in the classification (0.0-1.0)
    pub confidence: f32,
}

/// Estimate brightness from V4L2 exposure settings
///
/// Uses exposure time and ISO/gain to infer scene brightness.
/// Cameras use longer exposure or higher ISO in darker scenes.
fn classify_from_exposure(exposure: &ExposureMetadata) -> Option<(SceneBrightness, f32)> {
    // Exposure time in seconds
    let exp_time = exposure.exposure_time?;

    // Get ISO or gain (normalize gain to ISO-like scale)
    let iso = exposure.iso.map(|i| i as f32).or_else(|| {
        // Map gain to approximate ISO: gain of 0 ~ ISO 100, gain of 255 ~ ISO 3200
        exposure
            .gain
            .map(|g| 100.0 * (1.0 + g.max(0) as f32 / 32.0))
    })?;

    // Calculate exposure value (EV) approximation
    // EV = log2(100 * f^2 / (ISO * t)) for f/2.0 (typical webcam)
    // Simplified: higher EV = brighter scene
    let ev = (100.0 * 4.0 / (iso * exp_time as f32)).log2();

    debug!(
        exposure_time = exp_time,
        iso, ev, "V4L2 exposure-based brightness estimation"
    );

    // Map EV to brightness classification
    // EV ~14+ is very bright (sunny outdoors)
    // EV ~10-14 is bright (overcast, shade)
    // EV ~6-10 is medium (indoor lighting)
    // EV ~2-6 is low (dim indoor)
    // EV <2 is very dark (night)
    let (brightness, confidence) = if ev > 12.0 {
        (SceneBrightness::VeryBright, 0.8)
    } else if ev > 9.0 {
        (SceneBrightness::Bright, 0.7)
    } else if ev > 5.0 {
        (SceneBrightness::Medium, 0.6)
    } else if ev > 1.0 {
        (SceneBrightness::Low, 0.7)
    } else {
        (SceneBrightness::VeryDark, 0.8)
    };

    Some((brightness, confidence))
}

/// Classify brightness from GPU histogram metrics
///
/// Uses histogram-derived metrics for more nuanced classification.
fn classify_from_histogram(metrics: &GpuBrightnessMetrics) -> (SceneBrightness, f32) {
    let mean = metrics.mean_luminance;
    let median = metrics.median_luminance;
    let p5 = metrics.percentile_5;
    let p95 = metrics.percentile_95;
    let dynamic_range = metrics.dynamic_range_stops;

    // Use median as primary indicator (more robust to outliers than mean)
    // But also consider dynamic range and shadow/highlight distribution

    // High dynamic range scenes need more careful handling
    let is_high_contrast = dynamic_range > 6.0;

    // Determine base classification from median
    let base_brightness = if median > 0.5 {
        SceneBrightness::VeryBright
    } else if median > 0.3 {
        SceneBrightness::Bright
    } else if median > 0.1 {
        SceneBrightness::Medium
    } else if median > 0.03 {
        SceneBrightness::Low
    } else {
        SceneBrightness::VeryDark
    };

    // Calculate confidence based on histogram characteristics
    let mut confidence: f32 = 0.9;

    // Reduce confidence if mean and median diverge significantly (skewed histogram)
    let skew = (mean - median).abs();
    if skew > 0.1 {
        confidence -= 0.1;
    }

    // High contrast scenes are harder to classify accurately
    if is_high_contrast {
        confidence -= 0.1;
    }

    // Boost confidence if shadows/highlights match expectation
    match base_brightness {
        SceneBrightness::VeryBright => {
            if metrics.highlight_fraction > 0.2 {
                confidence += 0.05;
            }
        }
        SceneBrightness::VeryDark => {
            if metrics.shadow_fraction > 0.3 {
                confidence += 0.05;
            }
        }
        _ => {}
    }

    debug!(
        mean,
        median,
        p5,
        p95,
        dynamic_range,
        ?base_brightness,
        confidence,
        "Histogram-based brightness classification"
    );

    (base_brightness, confidence.clamp(0.0, 1.0))
}

/// Perform comprehensive multi-metric brightness analysis
///
/// Combines three sources of brightness information:
/// 1. GPU histogram (most accurate, uses percentiles and dynamic range)
/// 2. V4L2 exposure settings (what the camera thinks brightness is)
/// 3. Simple luminance average (fast fallback)
///
/// Returns fused classification with confidence score.
pub fn analyze_brightness_multi(
    frame: &CameraFrame,
    exposure: Option<ExposureMetadata>,
) -> MultiMetricBrightness {
    // 1. Always compute simple luminance (fast, always works)
    let (luminance_avg, simple_brightness) = estimate_scene_brightness(frame);
    let simple_confidence = 0.5; // Lowest confidence - just averaging

    // 2. Try GPU histogram analysis (most accurate)
    let histogram = analyze_brightness_gpu(&frame.data, frame.width, frame.height);
    let histogram_result = histogram.as_ref().map(classify_from_histogram);

    // 3. Try V4L2 exposure analysis
    let exposure_result = exposure.as_ref().and_then(classify_from_exposure);

    // Fuse results using confidence-weighted voting
    let mut votes: [(SceneBrightness, f32); 3] = [
        (simple_brightness, simple_confidence),
        histogram_result.unwrap_or((simple_brightness, 0.0)),
        exposure_result.unwrap_or((simple_brightness, 0.0)),
    ];

    // Sort by confidence (highest first)
    votes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Use highest confidence result, but boost confidence if sources agree
    let (classification, mut final_confidence) = votes[0];

    // Check for agreement between sources
    let sources_agree = votes
        .iter()
        .filter(|(b, c)| *c > 0.1 && *b == classification)
        .count();

    if sources_agree >= 2 {
        final_confidence = (final_confidence + 0.1).min(1.0);
    }

    info!(
        luminance_avg,
        ?simple_brightness,
        histogram_available = histogram.is_some(),
        exposure_available = exposure.is_some(),
        ?classification,
        confidence = final_confidence,
        sources_agreeing = sources_agree,
        "Multi-metric brightness analysis complete"
    );

    MultiMetricBrightness {
        luminance_avg,
        histogram,
        exposure,
        classification,
        confidence: final_confidence,
    }
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
        // Very bright scene (> 0.5)
        assert_eq!(
            SceneBrightness::from_luminance(0.6),
            SceneBrightness::VeryBright
        );

        // Bright scene (0.3 - 0.5)
        assert_eq!(
            SceneBrightness::from_luminance(0.4),
            SceneBrightness::Bright
        );

        // Medium scene (0.1 - 0.3)
        assert_eq!(
            SceneBrightness::from_luminance(0.2),
            SceneBrightness::Medium
        );

        // Low light scene (0.03 - 0.1)
        assert_eq!(SceneBrightness::from_luminance(0.05), SceneBrightness::Low);

        // Very dark scene (< 0.03)
        assert_eq!(
            SceneBrightness::from_luminance(0.01),
            SceneBrightness::VeryDark
        );
    }

    #[test]
    fn test_adaptive_params_scaling() {
        let very_bright = calculate_adaptive_params(SceneBrightness::VeryBright);
        let bright = calculate_adaptive_params(SceneBrightness::Bright);
        let dark = calculate_adaptive_params(SceneBrightness::VeryDark);

        // Very bright scenes should use 1 frame (no HDR+)
        assert_eq!(very_bright.frame_count, 1);

        // Bright scenes should use 2 frames per HDR+ paper
        assert_eq!(bright.frame_count, 2);

        // Darker scenes should capture more frames (max 8 per HDR+ paper)
        assert!(dark.frame_count > bright.frame_count);
        assert_eq!(dark.frame_count, 8);

        // Darker scenes should use higher robustness
        assert!(dark.robustness > bright.robustness);

        // Darker scenes should have higher motion threshold (more lenient)
        assert!(dark.motion_threshold > bright.motion_threshold);
    }
}
