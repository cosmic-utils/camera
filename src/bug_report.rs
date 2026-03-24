// SPDX-License-Identifier: GPL-3.0-only

//! Bug report generation
//!
//! Collects comprehensive system information for debugging purposes:
//! - Video/audio devices
//! - Available video encoders
//! - GPU information from WGPU
//! - System information (kernel, flatpak, etc.)
//! - Application settings (Config)
//! - Live diagnostics (InsightsState + audio/camera runtime state)

use crate::app::insights::types::{FallbackState, InsightsState};
use crate::config::Config;
use crate::constants::app_info;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

/// Snapshot of live application state needed for the bug report.
///
/// Fields that come from `AppModel` but are not in `InsightsState` or `Config`.
#[derive(Debug, Clone)]
pub struct AppStateSnapshot {
    /// Index of the currently selected camera
    pub current_camera_index: usize,
    /// Index of the currently selected audio device
    pub current_audio_device_index: usize,
    /// Index of the currently selected video encoder
    pub current_video_encoder_index: usize,
}

/// Bug report generator
pub struct BugReportGenerator;

impl BugReportGenerator {
    /// Generate a comprehensive bug report and save it to a file
    ///
    /// Returns the path to the generated report file
    pub async fn generate(
        video_devices: &[crate::backends::camera::types::CameraDevice],
        audio_devices: &[crate::backends::audio::AudioDevice],
        video_encoders: &[crate::media::encoders::video::EncoderInfo],
        save_folder_name: &str,
        insights: &InsightsState,
        config: &Config,
        snapshot: &AppStateSnapshot,
    ) -> Result<PathBuf, String> {
        info!("Generating bug report...");

        let mut report = String::new();

        // Header
        report.push_str("# Camera Bug Report\n\n");
        report.push_str(&format!(
            "Generated: {}\n\n",
            chrono::Local::now().to_rfc3339()
        ));

        // Application version
        report.push_str("## Application Information\n\n");
        report.push_str(&format!("**Version:** {}\n", app_info::version()));
        report.push_str(&format!(
            "**Runtime:** {}\n",
            app_info::runtime_environment()
        ));
        report.push('\n');

        // System information
        report.push_str(&Self::get_system_info().await);

        // GPU information
        report.push_str(&Self::get_gpu_info().await);

        // Video devices (full details)
        report.push_str(&Self::format_video_devices(
            video_devices,
            snapshot.current_camera_index,
        ));

        // PipeWire audio devices (full details)
        report.push_str(&Self::format_audio_devices(
            audio_devices,
            snapshot.current_audio_device_index,
        ));

        // Video encoders
        report.push_str(&Self::format_video_encoders(
            video_encoders,
            snapshot.current_video_encoder_index,
        ));

        // Settings (full Config dump)
        report.push_str(&Self::format_settings(
            config,
            video_devices.get(snapshot.current_camera_index),
            video_encoders.get(snapshot.current_video_encoder_index),
            audio_devices.get(snapshot.current_audio_device_index),
        ));

        // Insights / diagnostics (full live state)
        report.push_str(&Self::format_insights(
            insights,
            config,
            audio_devices.get(snapshot.current_audio_device_index),
        ));

        // PipeWire dump (optional diagnostics)
        report.push_str(&Self::get_pipewire_dump().await);

        // Save to file
        let output_path = Self::get_report_path(save_folder_name);
        tokio::fs::write(&output_path, report)
            .await
            .map_err(|e| format!("Failed to write bug report: {}", e))?;

        info!(path = ?output_path, "Bug report generated successfully");
        Ok(output_path)
    }

    /// Get the path where the bug report will be saved
    /// Reports are saved in the same directory as photos/videos: ~/Pictures/Camera/
    fn get_report_path(save_folder_name: &str) -> PathBuf {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("camera-bug-report-{}.md", timestamp);

        // Use the same directory as photos/videos
        let report_dir = crate::app::get_photo_directory(save_folder_name);

        // Ensure directory exists
        if let Err(e) = std::fs::create_dir_all(&report_dir) {
            warn!(error = %e, "Failed to create bug report directory, using fallback");
            // Fallback to home directory
            if let Some(home) = dirs::home_dir() {
                return home.join(&filename);
            }
        }

        report_dir.join(&filename)
    }

    /// Collect system information
    async fn get_system_info() -> String {
        let mut info = String::from("## System Information\n\n");

        // Linux kernel version
        if let Ok(output) = Command::new("uname").arg("-r").output()
            && let Ok(kernel) = String::from_utf8(output.stdout)
        {
            info.push_str(&format!("**Kernel:** {}\n", kernel.trim()));
        }

        // Distribution info
        if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
            for line in os_release.lines() {
                if line.starts_with("PRETTY_NAME=") {
                    let distro = line
                        .strip_prefix("PRETTY_NAME=")
                        .unwrap_or("")
                        .trim_matches('"');
                    info.push_str(&format!("**Distribution:** {}\n", distro));
                    break;
                }
            }
        }

        // Check if running in Flatpak
        if app_info::is_flatpak() {
            info.push_str("**Runtime:** Flatpak\n");

            // Get flatpak runtime details
            if let Ok(flatpak_info) = tokio::fs::read_to_string("/.flatpak-info").await {
                info.push_str("\n### Flatpak Details\n\n");
                info.push_str("```ini\n");
                info.push_str(&flatpak_info);
                info.push_str("```\n");
            }
        } else {
            info.push_str("**Runtime:** Native\n");
        }

        // PipeWire version
        if let Ok(output) = Command::new("pw-cli").arg("--version").output()
            && let Ok(pw_version) = String::from_utf8(output.stdout)
        {
            info.push_str(&format!("**PipeWire Version:** {}\n", pw_version.trim()));
        }

        info.push('\n');
        info
    }

    /// Get GPU information using system commands
    async fn get_gpu_info() -> String {
        let mut info = String::from("## GPU Information\n\n");

        // Try lspci for GPU info
        if let Ok(output) = Command::new("lspci").output()
            && let Ok(lspci_output) = String::from_utf8(output.stdout)
        {
            let gpu_lines: Vec<&str> = lspci_output
                .lines()
                .filter(|line| {
                    line.contains("VGA") || line.contains("3D") || line.contains("Display")
                })
                .collect();

            if !gpu_lines.is_empty() {
                for line in gpu_lines {
                    info.push_str(&format!("- {}\n", line));
                }
                info.push('\n');
            }
        }

        // Try glxinfo for more details (if available)
        if let Ok(output) = Command::new("glxinfo").arg("-B").output()
            && output.status.success()
            && let Ok(glx_output) = String::from_utf8(output.stdout)
        {
            info.push_str("### GLX Information\n\n");
            info.push_str("```\n");
            info.push_str(&glx_output);
            info.push_str("```\n\n");
        }

        // Try vulkaninfo (if available)
        if let Ok(output) = Command::new("vulkaninfo").arg("--summary").output()
            && output.status.success()
            && let Ok(vk_output) = String::from_utf8(output.stdout)
        {
            info.push_str("### Vulkan Information\n\n");
            info.push_str("```\n");
            info.push_str(&vk_output);
            info.push_str("```\n\n");
        }

        if info == "## GPU Information\n\n" {
            info.push_str("**Status:** Could not detect GPU information\n\n");
        }

        info
    }

    /// Format video devices with full details
    fn format_video_devices(
        devices: &[crate::backends::camera::types::CameraDevice],
        current_index: usize,
    ) -> String {
        let mut info = String::from("## Video Devices\n\n");

        if devices.is_empty() {
            info.push_str("**No video devices detected**\n\n");
            return info;
        }

        for (idx, device) in devices.iter().enumerate() {
            let selected = if idx == current_index {
                " **SELECTED**"
            } else {
                ""
            };
            info.push_str(&format!(
                "### Device {} - {}{}\n\n",
                idx + 1,
                device.name,
                selected,
            ));
            info.push_str(&format!("- **Camera ID:** `{}`\n", device.path));
            if let Some(ref di) = device.device_info {
                info.push_str(&format!("- **Card:** {}\n", di.card));
                info.push_str(&format!("- **Driver:** {}\n", di.driver));
                info.push_str(&format!("- **V4L2 Path:** {}\n", di.path));
                if di.real_path != di.path {
                    info.push_str(&format!("- **Real Path:** {}\n", di.real_path));
                }
            }
            info.push_str(&format!("- **Rotation:** {}\n", device.rotation));
            if let Some(ref handler) = device.pipeline_handler {
                info.push_str(&format!("- **Pipeline Handler:** {}\n", handler));
            }
            if let Some(ref sensor) = device.sensor_model {
                info.push_str(&format!("- **Sensor Model:** {}\n", sensor));
            }
            if let Some(ref location) = device.camera_location {
                info.push_str(&format!("- **Location:** {}\n", location));
            }
            if let Some(ref version) = device.libcamera_version {
                info.push_str(&format!("- **libcamera Version:** {}\n", version));
            }
            info.push_str(&format!(
                "- **Multi-stream Support:** {}\n",
                device.supports_multistream
            ));
            if let Some(ref lens) = device.lens_actuator_path {
                info.push_str(&format!("- **Lens Actuator:** {}\n", lens));
            }
            info.push('\n');
        }

        info
    }

    /// Format PipeWire audio devices with full details
    fn format_audio_devices(
        devices: &[crate::backends::audio::AudioDevice],
        current_index: usize,
    ) -> String {
        let mut info = String::from("## PipeWire Audio Devices\n\n");

        if devices.is_empty() {
            info.push_str("**No audio devices detected**\n\n");
            return info;
        }

        for (idx, device) in devices.iter().enumerate() {
            let selected = if idx == current_index {
                " **SELECTED**"
            } else {
                ""
            };
            info.push_str(&format!(
                "### Device {} - {}{}\n\n",
                idx + 1,
                device.name,
                selected,
            ));
            info.push_str(&format!("- **Serial:** {}\n", device.serial));
            info.push_str(&format!("- **Node Name:** `{}`\n", device.node_name));
            info.push_str(&format!("- **Default:** {}\n", device.is_default));
            if !device.sample_format.is_empty() {
                info.push_str(&format!(
                    "- **Format:** {} / {} Hz / {}ch\n",
                    device.sample_format,
                    device.sample_rate,
                    device.channels.len()
                ));
            }
            if !device.channels.is_empty() {
                info.push_str("- **Channels:**\n");
                for ch in &device.channels {
                    info.push_str(&format!("  - {} ({:.1} dB)\n", ch.position, ch.volume_db));
                }
            }
            info.push('\n');
        }

        info
    }

    /// Format video encoder information
    fn format_video_encoders(
        encoders: &[crate::media::encoders::video::EncoderInfo],
        selected_index: usize,
    ) -> String {
        let mut info = String::from("## Video Encoders\n\n");

        if encoders.is_empty() {
            info.push_str("**No video encoders detected**\n\n");
            return info;
        }

        for (idx, encoder) in encoders.iter().enumerate() {
            let selected = if idx == selected_index {
                " **SELECTED**"
            } else {
                ""
            };
            info.push_str(&format!(
                "### {} - {}{}\n\n",
                idx + 1,
                encoder.display_name,
                selected
            ));
            info.push_str(&format!("- **Codec:** {:?}\n", encoder.codec));
            info.push_str(&format!(
                "- **GStreamer Element:** {}\n",
                encoder.element_name
            ));
            info.push_str(&format!(
                "- **Hardware Accelerated:** {}\n",
                encoder.is_hardware
            ));
            info.push_str(&format!("- **Priority:** {}\n", encoder.priority));
            info.push('\n');
        }

        info
    }

    /// Format all application settings
    fn format_settings(
        config: &Config,
        current_camera: Option<&crate::backends::camera::types::CameraDevice>,
        current_encoder: Option<&crate::media::encoders::video::EncoderInfo>,
        current_audio_device: Option<&crate::backends::audio::AudioDevice>,
    ) -> String {
        let mut info = String::from("## Application Settings\n\n");

        info.push_str(&format!("- **Theme:** {:?}\n", config.app_theme));
        info.push_str(&format!("- **Save Folder:** {}\n", config.save_folder_name));
        info.push_str(&format!(
            "- **Mirror Preview:** {}\n",
            config.mirror_preview
        ));
        info.push_str(&format!(
            "- **Haptic Feedback:** {}\n",
            config.haptic_feedback
        ));
        info.push_str(&format!(
            "- **Composition Guide:** {:?}\n",
            config.composition_guide
        ));
        info.push_str(&format!(
            "- **Virtual Camera:** {}\n",
            config.virtual_camera_enabled
        ));

        // Current camera
        info.push_str("\n### Camera\n\n");
        if let Some(cam) = current_camera {
            info.push_str(&format!("- **Device:** {}\n", cam.name));
            if let Some(ref di) = cam.device_info {
                info.push_str(&format!("- **Card:** {}\n", di.card));
                info.push_str(&format!("- **Driver:** {}\n", di.driver));
                info.push_str(&format!("- **Path:** {}\n", di.path));
            }
        } else {
            info.push_str("- **Device:** (none)\n");
        }

        // Photo
        info.push_str("\n### Photo\n\n");
        info.push_str(&format!(
            "- **Output Format:** {}\n",
            config.photo_output_format.display_name()
        ));
        info.push_str(&format!(
            "- **Burst Mode:** {:?}\n",
            config.burst_mode_setting
        ));
        info.push_str(&format!(
            "- **Save Burst Raw:** {}\n",
            config.save_burst_raw
        ));
        if !config.photo_settings.is_empty() {
            info.push_str("- **Per-Camera Photo Settings:**\n");
            for (camera, settings) in &config.photo_settings {
                info.push_str(&format!(
                    "  - `{}`: {}x{} {} @ {} fps\n",
                    camera,
                    settings.width,
                    settings.height,
                    settings.pixel_format,
                    settings
                        .framerate
                        .map(|f| f.to_string())
                        .unwrap_or_else(|| "(auto)".to_string()),
                ));
            }
        }

        // Timelapse
        info.push_str("\n### Timelapse\n\n");
        info.push_str(&format!(
            "- **Interval:** {}\n",
            config.timelapse_interval.display_name()
        ));

        // Video
        info.push_str("\n### Video\n\n");
        if let Some(enc) = current_encoder {
            info.push_str(&format!("- **Encoder:** {}\n", enc.display_name));
        }
        info.push_str(&format!(
            "- **Quality:** {}\n",
            config.bitrate_preset.display_name()
        ));
        info.push_str(&format!("- **Record Audio:** {}\n", config.record_audio));
        info.push_str(&format!(
            "- **Audio Encoder:** {}\n",
            config.audio_encoder.display_name()
        ));
        if let Some(dev) = current_audio_device {
            let mic_name = if dev.is_default {
                format!("{} (Default)", dev.name)
            } else {
                dev.name.clone()
            };
            info.push_str(&format!("- **Microphone:** {}\n", mic_name));
        }
        if !config.video_settings.is_empty() {
            info.push_str("- **Per-Camera Video Settings:**\n");
            for (camera, settings) in &config.video_settings {
                info.push_str(&format!(
                    "  - `{}`: {}x{} {} @ {} fps\n",
                    camera,
                    settings.width,
                    settings.height,
                    settings.pixel_format,
                    settings
                        .framerate
                        .map(|f| f.to_string())
                        .unwrap_or_else(|| "(auto)".to_string()),
                ));
            }
        }

        info.push('\n');
        info
    }

    /// Format all insights/diagnostics state
    fn format_insights(
        insights: &InsightsState,
        config: &Config,
        current_audio_device: Option<&crate::backends::audio::AudioDevice>,
    ) -> String {
        let mut info = String::from("## Insights / Diagnostics\n\n");
        let na = "N/A";

        // Pipeline string
        info.push_str("### Pipeline\n\n");
        if let Some(ref pipeline) = insights.full_pipeline_string {
            info.push_str("```\n");
            info.push_str(pipeline);
            info.push_str("\n```\n");
        } else {
            info.push_str("No pipeline active\n");
        }

        // Decoder chain
        if !insights.decoder_chain.is_empty() {
            info.push_str("\n### Decoder Fallback Chain\n\n");
            for d in &insights.decoder_chain {
                let state = match d.state {
                    FallbackState::Selected => "SELECTED",
                    FallbackState::Available => "Available",
                    FallbackState::Unavailable => "Unavailable",
                };
                info.push_str(&format!(
                    "- **{}** ({}) - {}\n",
                    d.name, d.description, state
                ));
            }
        }

        // Backend info
        info.push_str("\n### Backend\n\n");
        if !insights.backend_type.is_empty() {
            info.push_str(&format!("- **Type:** {}\n", insights.backend_type));
            if let Some(ref model) = insights.sensor_model {
                info.push_str(&format!("- **Sensor:** {}\n", model));
            }
            if let Some(ref version) = insights.libcamera_version {
                info.push_str(&format!("- **libcamera Version:** {}\n", version));
            }
            if let Some(ref decoder) = insights.mjpeg_decoder {
                info.push_str(&format!("- **MJPEG Decoder:** {}\n", decoder));
            }
            if let Some(ref handler) = insights.pipeline_handler {
                info.push_str(&format!("- **Pipeline Handler:** {}\n", handler));
            }
            let mode = if insights.is_multistream {
                "Dual-stream"
            } else {
                "Single-stream"
            };
            let source = if insights.is_multistream {
                "Separate Preview & Capture"
            } else {
                "Preview & Capture"
            };
            info.push_str(&format!("- **{}:** {}\n", mode, source));
        } else {
            info.push_str("- **Type:** (not active)\n");
        }

        // Stream info
        if let Some(ref stream) = insights.preview_stream {
            let title = if insights.is_multistream {
                "Preview Stream"
            } else {
                "Preview + Capture Stream"
            };
            info.push_str(&format!("\n### {}\n\n", title));
            Self::format_stream_info(&mut info, stream);
        }
        if insights.is_multistream
            && let Some(ref stream) = insights.capture_stream
        {
            info.push_str("\n### Capture Stream\n\n");
            Self::format_stream_info(&mut info, stream);
        }

        // Format chain (source, native format, processing)
        let chain = &insights.format_chain;
        if !chain.source.is_empty() {
            info.push_str("\n### Format Chain\n\n");
            info.push_str(&format!("- **Source:** {}\n", chain.source));
            if !chain.resolution.is_empty() {
                info.push_str(&format!("- **Resolution:** {}\n", chain.resolution));
            }
            if !chain.framerate.is_empty() {
                info.push_str(&format!("- **Framerate:** {}\n", chain.framerate));
            }
            info.push_str(&format!("- **Native Format:** {}\n", chain.native_format));
            if let Some(ref cpu_proc) = insights.cpu_processing {
                info.push_str(&format!("- **CPU Processing:** {}\n", cpu_proc));
            }
            info.push_str(&format!(
                "- **GPU Processing:** {}\n",
                chain.wgpu_processing
            ));
        }

        // Performance metrics
        info.push_str("\n### Performance Metrics\n\n");
        let latency_ms = insights.frame_latency_us as f64 / 1000.0;
        info.push_str(&format!("- **Frame Latency:** {:.2} ms\n", latency_ms));
        info.push_str(&format!(
            "- **Dropped Frames:** {}\n",
            insights.dropped_frames
        ));
        let frame_mb = insights.frame_size_decoded as f64 / (1024.0 * 1024.0);
        info.push_str(&format!("- **Frame Size:** {:.2} MB\n", frame_mb));
        if insights.cpu_decode_time_us > 0 {
            let cpu_ms = insights.cpu_decode_time_us as f64 / 1000.0;
            info.push_str(&format!("- **CPU Decode Time:** {:.2} ms\n", cpu_ms));
        }
        let copy_ms = insights.copy_time_us as f64 / 1000.0;
        if copy_ms < 0.01 {
            info.push_str("- **Frame Wrap Time:** < 0.01 ms (zero-copy)\n");
        } else {
            info.push_str(&format!("- **Frame Wrap Time:** {:.2} ms\n", copy_ms));
        }
        let gpu_ms = insights.gpu_conversion_time_us as f64 / 1000.0;
        info.push_str(&format!("- **GPU Upload Time:** {:.2} ms\n", gpu_ms));
        if insights.copy_bandwidth_mbps > 0.0 {
            info.push_str(&format!(
                "- **GPU Upload Bandwidth:** {:.1} MB/s\n",
                insights.copy_bandwidth_mbps
            ));
        }

        // Audio section (mirrors the insights UI audio section)
        info.push_str("\n### Audio\n\n");
        let status = if config.record_audio {
            "Enabled"
        } else {
            "Disabled"
        };
        info.push_str(&format!("- **Recording:** {}\n", status));
        if let Some(dev) = current_audio_device {
            let dev_name = if dev.is_default {
                format!("{} (Default)", dev.name)
            } else {
                dev.name.clone()
            };
            info.push_str(&format!("- **Device:** {}\n", dev_name));
            info.push_str(&format!("- **Node Name:** `{}`\n", dev.node_name));
            if !dev.sample_format.is_empty() {
                info.push_str(&format!(
                    "- **Format:** {} / {} Hz / {}ch\n",
                    dev.sample_format,
                    dev.sample_rate,
                    dev.channels.len()
                ));
            }
        }
        // Codec
        let codec = if gstreamer::ElementFactory::find("opusenc").is_some() {
            "Opus"
        } else if gstreamer::ElementFactory::find("avenc_aac").is_some()
            || gstreamer::ElementFactory::find("faac").is_some()
            || gstreamer::ElementFactory::find("voaacenc").is_some()
        {
            "AAC"
        } else {
            "None"
        };
        info.push_str(&format!("- **Codec:** {}\n", codec));
        info.push_str("- **Channels:** Mono\n");
        // Audio pipeline
        if config.record_audio {
            let enc_element = if codec == "Opus" {
                "opusenc"
            } else if codec == "AAC" {
                "avenc_aac"
            } else {
                "none"
            };
            info.push_str(&format!(
                "- **Pipeline:** `pulsesrc \u{2192} queue \u{2192} audioconvert \u{2192} audioresample \u{2192} level \u{2192} capsfilter(mono) \u{2192} level \u{2192} {}`\n",
                enc_element
            ));
        }
        // Input channel volumes
        if let Some(dev) = current_audio_device
            && !dev.channels.is_empty()
        {
            info.push_str("- **Input Channels:**\n");
            for (i, ch) in dev.channels.iter().enumerate() {
                let live_rms = insights
                    .audio_levels
                    .as_ref()
                    .and_then(|l| l.input_rms_db.get(i).copied());
                let level_text = live_rms
                    .map(|db| format!(" (live: {:.1} dB)", db))
                    .unwrap_or_default();
                info.push_str(&format!(
                    "  - {} {:.1} dB{}\n",
                    ch.position, ch.volume_db, level_text
                ));
            }
        }
        // Output levels
        if let Some(ref levels) = insights.audio_levels {
            info.push_str(&format!(
                "- **Output Peak:** {:.1} dB\n",
                levels.output_peak_db
            ));
            info.push_str(&format!(
                "- **Output RMS:** {:.1} dB\n",
                levels.output_rms_db
            ));
        }

        // Frame metadata — full libcamera FrameMetadata dump
        info.push_str("\n### Frame Metadata (libcamera)\n\n");
        if let Some(ref meta) = insights.frame_metadata {
            Self::format_frame_metadata(&mut info, meta);
        } else {
            // Fall back to the InsightsState copies (e.g. non-libcamera backends)
            info.push_str(&format!(
                "- **Exposure:** {}\n",
                insights
                    .meta_exposure_us
                    .map_or_else(|| na.to_string(), Self::format_exposure)
            ));
            info.push_str(&format!(
                "- **Analogue Gain:** {}\n",
                insights
                    .meta_analogue_gain
                    .map_or_else(|| na.to_string(), |g| format!("{:.2}x", g))
            ));
            info.push_str(&format!(
                "- **Digital Gain:** {}\n",
                insights
                    .meta_digital_gain
                    .map_or_else(|| na.to_string(), |g| format!("{:.2}x", g))
            ));
            info.push_str(&format!(
                "- **Colour Temp:** {}\n",
                insights
                    .meta_colour_temperature
                    .map_or_else(|| na.to_string(), |t| format!("{} K", t))
            ));
            info.push_str(&format!(
                "- **WB Gains (R, B):** {}\n",
                insights
                    .meta_colour_gains
                    .map_or_else(|| na.to_string(), |g| format!("{:.2}, {:.2}", g[0], g[1]))
            ));
            info.push_str(&format!(
                "- **Black Level:** {}\n",
                insights
                    .meta_black_level
                    .map_or_else(|| na.to_string(), |bl| format!("{:.4}", bl))
            ));
            info.push_str(&format!(
                "- **Illuminance:** {}\n",
                insights
                    .meta_lux
                    .map_or_else(|| na.to_string(), |l| format!("{:.0} lux", l))
            ));
            info.push_str(&format!(
                "- **Lens Position:** {}\n",
                insights
                    .meta_lens_position
                    .map_or_else(|| na.to_string(), |p| format!("{:.2} dioptres", p))
            ));
            info.push_str(&format!(
                "- **Focus FoM:** {}\n",
                insights
                    .meta_focus_fom
                    .map_or_else(|| na.to_string(), |f| format!("{}", f))
            ));
            info.push_str(&format!(
                "- **Sequence:** {}\n",
                insights
                    .meta_sequence
                    .map_or_else(|| na.to_string(), |s| format!("{}", s))
            ));
        }

        // Recording diagnostics
        if let Some(ref diag) = insights.recording_diag {
            info.push_str("\n### Recording Pipeline\n\n");
            info.push_str(&format!("- **Mode:** {}\n", diag.mode));
            info.push_str(&format!("- **Encoder:** {}\n", diag.encoder));
            info.push_str(&format!(
                "- **Resolution:** {} @ {} fps\n",
                diag.resolution, diag.framerate
            ));
            info.push_str("\n```\n");
            info.push_str(&diag.pipeline_string);
            info.push_str("\n```\n");
        }

        if let Some(ref stats) = insights.recording_stats {
            info.push_str("\n### Recording Live Stats\n\n");
            info.push_str(&format!(
                "- **Effective FPS:** {:.1}\n",
                stats.effective_fps
            ));
            info.push_str(&format!(
                "- **Capture:** {} sent, {} dropped\n",
                stats.capture_sent, stats.capture_dropped
            ));
            info.push_str(&format!(
                "- **Channel Backlog:** {} queued\n",
                stats.channel_backlog
            ));
            info.push_str(&format!(
                "- **Pusher:** {} pushed, {} skipped\n",
                stats.pusher_pushed, stats.pusher_skipped
            ));
            if stats.last_processing_delay_us > 0 {
                let delay_ms = stats.last_processing_delay_us as f64 / 1000.0;
                info.push_str(&format!("- **Processing Delay:** {:.1} ms\n", delay_ms));
            }
            if stats.last_convert_time_us > 0 {
                let convert_ms = stats.last_convert_time_us as f64 / 1000.0;
                info.push_str(&format!("- **NV12 Convert Time:** {:.2} ms\n", convert_ms));
            }
            info.push_str(&format!(
                "- **Current PTS:** {:.1} s\n",
                stats.last_pts_ms as f64 / 1000.0
            ));
        }

        info.push('\n');
        info
    }

    /// Format an exposure time value with appropriate units
    fn format_exposure(us: u64) -> String {
        if us >= 1_000_000 {
            format!("{:.2} s", us as f64 / 1_000_000.0)
        } else if us >= 1_000 {
            format!("{:.2} ms", us as f64 / 1_000.0)
        } else {
            format!("{} \u{00b5}s", us)
        }
    }

    /// Format the full libcamera FrameMetadata with all fields
    fn format_frame_metadata(
        info: &mut String,
        meta: &crate::backends::camera::types::FrameMetadata,
    ) {
        let na = "N/A";

        info.push_str(&format!(
            "- **Exposure:** {}\n",
            meta.exposure_time
                .map_or_else(|| na.to_string(), Self::format_exposure)
        ));
        info.push_str(&format!(
            "- **Analogue Gain:** {}\n",
            meta.analogue_gain
                .map_or_else(|| na.to_string(), |g| format!("{:.2}x", g))
        ));
        info.push_str(&format!(
            "- **Digital Gain:** {}\n",
            meta.digital_gain
                .map_or_else(|| na.to_string(), |g| format!("{:.2}x", g))
        ));
        info.push_str(&format!(
            "- **Colour Temp:** {}\n",
            meta.colour_temperature
                .map_or_else(|| na.to_string(), |t| format!("{} K", t))
        ));
        info.push_str(&format!(
            "- **WB Gains (R, B):** {}\n",
            meta.colour_gains
                .map_or_else(|| na.to_string(), |g| format!("{:.2}, {:.2}", g[0], g[1]))
        ));
        info.push_str(&format!(
            "- **Black Level:** {}\n",
            meta.black_level
                .map_or_else(|| na.to_string(), |bl| format!("{:.4}", bl))
        ));
        info.push_str(&format!(
            "- **Illuminance:** {}\n",
            meta.lux
                .map_or_else(|| na.to_string(), |l| format!("{:.0} lux", l))
        ));
        info.push_str(&format!(
            "- **Lens Position:** {}\n",
            meta.lens_position
                .map_or_else(|| na.to_string(), |p| format!("{:.2} dioptres", p))
        ));
        info.push_str(&format!(
            "- **Focus FoM:** {}\n",
            meta.focus_fom
                .map_or_else(|| na.to_string(), |f| format!("{}", f))
        ));
        info.push_str(&format!(
            "- **Sequence:** {}\n",
            meta.sequence
                .map_or_else(|| na.to_string(), |s| format!("{}", s))
        ));
        info.push_str(&format!(
            "- **Sensor Timestamp:** {}\n",
            meta.sensor_timestamp.map_or_else(
                || na.to_string(),
                |ns| format!("{} ns ({:.3} s)", ns, ns as f64 / 1_000_000_000.0)
            )
        ));
        info.push_str(&format!(
            "- **AF State:** {}\n",
            meta.af_state
                .map_or_else(|| na.to_string(), |s| format!("{:?}", s))
        ));
        info.push_str(&format!(
            "- **AE State:** {}\n",
            meta.ae_state
                .map_or_else(|| na.to_string(), |s| format!("{:?}", s))
        ));
        info.push_str(&format!(
            "- **AWB State:** {}\n",
            meta.awb_state
                .map_or_else(|| na.to_string(), |s| format!("{:?}", s))
        ));
        // Colour correction matrix
        if let Some(ccm) = &meta.colour_correction_matrix {
            info.push_str("- **Colour Correction Matrix:**\n");
            info.push_str("  ```\n");
            for row in ccm {
                info.push_str(&format!(
                    "  [{:8.4}, {:8.4}, {:8.4}]\n",
                    row[0], row[1], row[2]
                ));
            }
            info.push_str("  ```\n");
        } else {
            info.push_str(&format!("- **Colour Correction Matrix:** {}\n", na));
        }
    }

    /// Format a single stream info block
    fn format_stream_info(info: &mut String, stream: &crate::app::insights::types::StreamInfo) {
        info.push_str(&format!("- **Role:** {}\n", stream.role));
        info.push_str(&format!("- **Resolution:** {}\n", stream.resolution));
        info.push_str(&format!("- **Pixel Format:** {}\n", stream.pixel_format));
        info.push_str(&format!("- **Frame Count:** {}\n", stream.frame_count));
        if !stream.source.is_empty() {
            info.push_str(&format!("- **Source:** {}\n", stream.source));
        }
        if !stream.gpu_processing.is_empty() {
            info.push_str(&format!(
                "- **GPU Processing:** {}\n",
                stream.gpu_processing
            ));
        }
        if stream.frame_size_bytes > 0 {
            let mb = stream.frame_size_bytes as f64 / (1024.0 * 1024.0);
            info.push_str(&format!("- **Frame Size:** {:.2} MB\n", mb));
        }
    }

    /// Get detailed PipeWire dump
    async fn get_pipewire_dump() -> String {
        let mut info = String::from("## PipeWire Detailed Information\n\n");

        // Get pw-dump output if available
        if let Ok(output) = Command::new("pw-dump").output()
            && output.status.success()
            && let Ok(dump) = String::from_utf8(output.stdout)
        {
            info.push_str("```json\n");
            info.push_str(&dump);
            info.push_str("\n```\n\n");
            return info;
        }

        info.push_str("**pw-dump not available or failed**\n\n");
        info
    }
}
