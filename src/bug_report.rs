// SPDX-License-Identifier: MPL-2.0

//! Bug report generation
//!
//! Collects comprehensive system information for debugging purposes:
//! - PipeWire video/audio devices
//! - Available video encoders
//! - GPU information from WGPU
//! - System information (kernel, flatpak, etc.)

use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

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
        selected_encoder_index: usize,
        _wgpu_adapter_info: Option<String>,
    ) -> Result<PathBuf, String> {
        info!("Generating bug report...");

        let mut report = String::new();

        // Header
        report.push_str("# COSMIC Camera Bug Report\n\n");
        report.push_str(&format!(
            "Generated: {}\n\n",
            chrono::Local::now().to_rfc3339()
        ));

        // Application version
        report.push_str("## Application Information\n\n");
        report.push_str(&format!("**Version:** {}\n", env!("GIT_VERSION")));
        report.push_str("\n");

        // System information
        report.push_str(&Self::get_system_info().await);

        // GPU information
        report.push_str(&Self::get_gpu_info().await);

        // PipeWire video devices
        report.push_str(&Self::format_video_devices(video_devices));

        // PipeWire audio devices
        report.push_str(&Self::format_audio_devices(audio_devices));

        // Video encoders
        report.push_str(&Self::format_video_encoders(
            video_encoders,
            selected_encoder_index,
        ));

        // PipeWire detailed dump
        report.push_str(&Self::get_pipewire_dump().await);

        // Save to file
        let output_path = Self::get_report_path();
        tokio::fs::write(&output_path, report)
            .await
            .map_err(|e| format!("Failed to write bug report: {}", e))?;

        info!(path = ?output_path, "Bug report generated successfully");
        Ok(output_path)
    }

    /// Get the path where the bug report will be saved
    /// Reports are saved in the same directory as photos/videos: ~/Pictures/cosmic-camera/
    fn get_report_path() -> PathBuf {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("cosmic-camera-bug-report-{}.md", timestamp);

        // Use the same directory as photos/videos
        let report_dir = crate::app::get_photo_directory();

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
        if let Ok(output) = Command::new("uname").arg("-r").output() {
            if let Ok(kernel) = String::from_utf8(output.stdout) {
                info.push_str(&format!("**Kernel:** {}\n", kernel.trim()));
            }
        }

        // Distribution info
        if let Ok(output) = Command::new("cat").arg("/etc/os-release").output() {
            if let Ok(os_release) = String::from_utf8(output.stdout) {
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
        }

        // Check if running in Flatpak
        if std::path::Path::new("/.flatpak-info").exists() {
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
        if let Ok(output) = Command::new("pw-cli").arg("--version").output() {
            if let Ok(pw_version) = String::from_utf8(output.stdout) {
                info.push_str(&format!("**PipeWire Version:** {}\n", pw_version.trim()));
            }
        }

        info.push_str("\n");
        info
    }

    /// Get GPU information using system commands
    async fn get_gpu_info() -> String {
        let mut info = String::from("## GPU Information\n\n");

        // Try lspci for GPU info
        if let Ok(output) = Command::new("lspci").output() {
            if let Ok(lspci_output) = String::from_utf8(output.stdout) {
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
                    info.push_str("\n");
                }
            }
        }

        // Try glxinfo for more details (if available)
        if let Ok(output) = Command::new("glxinfo").arg("-B").output() {
            if output.status.success() {
                if let Ok(glx_output) = String::from_utf8(output.stdout) {
                    info.push_str("### GLX Information\n\n");
                    info.push_str("```\n");
                    info.push_str(&glx_output);
                    info.push_str("```\n\n");
                }
            }
        }

        // Try vulkaninfo (if available)
        if let Ok(output) = Command::new("vulkaninfo").arg("--summary").output() {
            if output.status.success() {
                if let Ok(vk_output) = String::from_utf8(output.stdout) {
                    info.push_str("### Vulkan Information\n\n");
                    info.push_str("```\n");
                    info.push_str(&vk_output);
                    info.push_str("```\n\n");
                }
            }
        }

        if info == "## GPU Information\n\n" {
            info.push_str("**Status:** Could not detect GPU information\n\n");
        }

        info
    }

    /// Format PipeWire video devices
    fn format_video_devices(devices: &[crate::backends::camera::types::CameraDevice]) -> String {
        let mut info = String::from("## PipeWire Video Devices\n\n");

        if devices.is_empty() {
            info.push_str("**No video devices detected**\n\n");
            return info;
        }

        for (idx, device) in devices.iter().enumerate() {
            info.push_str(&format!("### Device {} - {}\n\n", idx + 1, device.name));
            info.push_str(&format!("- **Path:** {}\n", device.path));
            if let Some(metadata_path) = &device.metadata_path {
                info.push_str(&format!("- **Metadata Path:** {}\n", metadata_path));
            }
            info.push_str("\n");
        }

        info
    }

    /// Format PipeWire audio devices
    fn format_audio_devices(devices: &[crate::backends::audio::AudioDevice]) -> String {
        let mut info = String::from("## PipeWire Audio Devices\n\n");

        if devices.is_empty() {
            info.push_str("**No audio devices detected**\n\n");
            return info;
        }

        for (idx, device) in devices.iter().enumerate() {
            info.push_str(&format!("### Device {} - {}\n\n", idx + 1, device.name));
            info.push_str(&format!("- **Serial:** {}\n", device.serial));
            info.push_str(&format!("- **Node Name:** {}\n", device.node_name));
            info.push_str(&format!("- **Default:** {}\n", device.is_default));
            info.push_str("\n");
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
                " ✓ **SELECTED**"
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
            info.push_str("\n");
        }

        info
    }

    /// Get detailed PipeWire dump
    async fn get_pipewire_dump() -> String {
        let mut info = String::from("## PipeWire Detailed Information\n\n");

        // Get pw-dump output if available
        if let Ok(output) = Command::new("pw-dump").output() {
            if output.status.success() {
                if let Ok(dump) = String::from_utf8(output.stdout) {
                    info.push_str("```json\n");
                    info.push_str(&dump);
                    info.push_str("\n```\n\n");
                    return info;
                }
            }
        }

        info.push_str("**pw-dump not available or failed**\n\n");
        info
    }
}
