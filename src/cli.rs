// SPDX-License-Identifier: GPL-3.0-only

//! CLI commands for camera operations
//!
//! This module provides command-line functionality for:
//! - Listing available cameras
//! - Taking photos
//! - Recording videos

use camera::backends::camera::pipewire::{
    PipeWirePipeline, enumerate_pipewire_cameras, get_pipewire_formats,
};
use camera::backends::camera::types::{CameraFormat, CameraFrame};
use camera::pipelines::photo::PhotoPipeline;
use camera::pipelines::video::{EncoderConfig, VideoRecorder};
use chrono::Local;
use futures::channel::mpsc;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// List all available cameras
pub fn list_cameras() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize GStreamer
    gstreamer::init()?;

    let cameras = enumerate_pipewire_cameras().unwrap_or_default();

    if cameras.is_empty() {
        println!("No cameras found.");
        return Ok(());
    }

    println!("Available cameras:");
    println!();
    for (index, camera) in cameras.iter().enumerate() {
        println!("  [{}] {}", index, camera.name);

        // Get formats for this camera
        let formats = get_pipewire_formats(&camera.path, camera.metadata_path.as_deref());
        if !formats.is_empty() {
            // Group formats by resolution and show best framerate
            let mut resolutions: Vec<(u32, u32, u32)> = Vec::new();
            for format in &formats {
                let fps = format.framerate.unwrap_or(30);
                if let Some(existing) = resolutions
                    .iter_mut()
                    .find(|(w, h, _)| *w == format.width && *h == format.height)
                {
                    if fps > existing.2 {
                        existing.2 = fps;
                    }
                } else {
                    resolutions.push((format.width, format.height, fps));
                }
            }

            // Sort by resolution (highest first)
            resolutions.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)));

            // Show top 3 resolutions
            let display_count = resolutions.len().min(3);
            let res_strs: Vec<String> = resolutions
                .iter()
                .take(display_count)
                .map(|(w, h, fps)| format!("{}x{}@{}fps", w, h, fps))
                .collect();

            println!("      Formats: {}", res_strs.join(", "));
        }
        println!();
    }

    Ok(())
}

/// Take a photo using the specified camera
pub fn take_photo(
    camera_index: usize,
    output: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize GStreamer
    gstreamer::init()?;

    // Enumerate cameras
    let cameras = enumerate_pipewire_cameras().unwrap_or_default();
    if cameras.is_empty() {
        return Err("No cameras found".into());
    }

    if camera_index >= cameras.len() {
        return Err(format!(
            "Camera index {} out of range (0-{})",
            camera_index,
            cameras.len() - 1
        )
        .into());
    }

    let camera = &cameras[camera_index];
    println!("Using camera: {}", camera.name);

    // Get formats and select best one for photos (highest resolution)
    let formats = get_pipewire_formats(&camera.path, camera.metadata_path.as_deref());
    if formats.is_empty() {
        return Err("No formats available for camera".into());
    }

    let format = select_photo_format(&formats);
    println!("Capture format: {}x{}", format.width, format.height);

    // Determine output path
    let output_dir = if let Some(path) = output.as_ref() {
        if path.is_dir() {
            path.clone()
        } else {
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| get_default_photo_dir())
        }
    } else {
        get_default_photo_dir()
    };

    // Ensure output directory exists
    std::fs::create_dir_all(&output_dir)?;

    // Start camera pipeline
    println!("Capturing...");
    let (sender, mut receiver) = mpsc::channel(10);
    let _pipeline = PipeWirePipeline::new(camera, &format, sender)?;

    // Wait for frames to stabilize (camera warm-up)
    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    let warmup = Duration::from_millis(500);
    let mut frame: Option<CameraFrame> = None;

    while start.elapsed() < timeout {
        match receiver.try_next() {
            Ok(Some(f)) => {
                frame = Some(f);
                // After warmup period, use the next good frame
                if start.elapsed() > warmup {
                    break;
                }
            }
            Ok(None) => break, // Channel closed
            Err(_) => {
                // No frame available yet, wait a bit
                std::thread::sleep(Duration::from_millis(16));
            }
        }
    }

    let frame = frame.ok_or("Failed to capture frame from camera")?;

    // Use photo pipeline to save the image
    let photo_pipeline = PhotoPipeline::new();

    // Create async runtime for the pipeline
    let rt = tokio::runtime::Runtime::new()?;
    let output_path = rt.block_on(async {
        photo_pipeline
            .capture_and_save(Arc::new(frame), output_dir)
            .await
    })?;

    // If user specified a specific filename, rename the file
    if let Some(user_path) = output {
        if !user_path.is_dir() {
            std::fs::rename(&output_path, &user_path)?;
            println!("Photo saved: {}", user_path.display());
            return Ok(());
        }
    }

    println!("Photo saved: {}", output_path.display());
    Ok(())
}

/// Record a video using the specified camera
pub fn record_video(
    camera_index: usize,
    duration: u64,
    output: Option<PathBuf>,
    enable_audio: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize GStreamer
    gstreamer::init()?;

    // Enumerate cameras
    let cameras = enumerate_pipewire_cameras().unwrap_or_default();
    if cameras.is_empty() {
        return Err("No cameras found".into());
    }

    if camera_index >= cameras.len() {
        return Err(format!(
            "Camera index {} out of range (0-{})",
            camera_index,
            cameras.len() - 1
        )
        .into());
    }

    let camera = &cameras[camera_index];
    println!("Using camera: {}", camera.name);

    // Get formats and select best one for video
    let formats = get_pipewire_formats(&camera.path, camera.metadata_path.as_deref());
    if formats.is_empty() {
        return Err("No formats available for camera".into());
    }

    let format = select_video_format(&formats);
    let framerate = format.framerate.unwrap_or(30);
    println!(
        "Recording format: {}x{} @ {}fps",
        format.width, format.height, framerate
    );

    // Determine output path
    let output_path = if let Some(path) = output {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        path
    } else {
        let dir = get_default_video_dir();
        std::fs::create_dir_all(&dir)?;
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        dir.join(format!("video_{}.mp4", timestamp))
    };

    println!("Output: {}", output_path.display());
    println!("Duration: {} seconds", duration);
    if enable_audio {
        println!("Audio: enabled");
    }

    // Create encoder config
    let encoder_config = EncoderConfig::default();

    // Create video recorder
    let recorder = VideoRecorder::new(
        &camera.path,
        camera.metadata_path.as_deref(),
        format.width,
        format.height,
        framerate,
        &format.pixel_format,
        output_path.clone(),
        encoder_config,
        enable_audio,
        None, // Use default audio device
        None, // No preview sender needed for CLI
        None, // Auto-select encoder
    )?;

    // Start recording
    println!();
    println!("Recording... (press Ctrl+C to stop early)");
    recorder.start()?;

    // Set up Ctrl+C handler
    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();
    ctrlc::set_handler(move || {
        stop_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    })?;

    // Wait for duration or Ctrl+C
    let start = Instant::now();
    let target_duration = Duration::from_secs(duration);

    while start.elapsed() < target_duration {
        if stop_flag.load(std::sync::atomic::Ordering::SeqCst) {
            println!();
            println!("Stopping early...");
            break;
        }

        // Print progress
        let elapsed = start.elapsed().as_secs();
        print!("\rRecording: {:02}:{:02}", elapsed / 60, elapsed % 60);
        std::io::Write::flush(&mut std::io::stdout())?;

        std::thread::sleep(Duration::from_millis(100));
    }
    println!();

    // Stop recording
    let final_path = recorder.stop()?;
    println!("Video saved: {}", final_path.display());

    Ok(())
}

/// Select the best format for photo capture (highest resolution)
fn select_photo_format(formats: &[CameraFormat]) -> CameraFormat {
    formats
        .iter()
        .max_by_key(|f| f.width * f.height)
        .cloned()
        .unwrap_or_else(|| formats[0].clone())
}

/// Select the best format for video recording (balanced resolution and framerate)
fn select_video_format(formats: &[CameraFormat]) -> CameraFormat {
    // Prefer 1080p at 30fps, otherwise highest resolution with reasonable framerate
    let target_height = 1080;
    let target_fps = 30;

    // First try to find exact match
    if let Some(format) = formats.iter().find(|f| {
        f.height == target_height && f.framerate.map(|fps| fps >= target_fps).unwrap_or(false)
    }) {
        return format.clone();
    }

    // Otherwise find closest to 1080p with at least 24fps
    formats
        .iter()
        .filter(|f| f.framerate.map(|fps| fps >= 24).unwrap_or(false))
        .min_by_key(|f| {
            let height_diff = (f.height as i32 - target_height as i32).abs();
            let fps_diff = (f.framerate.unwrap_or(30) as i32 - target_fps as i32).abs();
            height_diff * 10 + fps_diff // Prioritize resolution over framerate
        })
        .cloned()
        .unwrap_or_else(|| formats[0].clone())
}

/// Get default photo directory
fn get_default_photo_dir() -> PathBuf {
    dirs::picture_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("camera")
}

/// Get default video directory
fn get_default_video_dir() -> PathBuf {
    dirs::video_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("camera")
}
