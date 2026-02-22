// SPDX-License-Identifier: GPL-3.0-only

//! Hardware flash LED control via Linux sysfs
//!
//! Discovers and controls flash LEDs exposed at `/sys/class/leds/*:flash`.
//! Uses torch mode (brightness file) which is group-writable by `feedbackd`,
//! avoiding the root-only `flash_strobe`/`flash_brightness` interface.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Flash operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlashMode {
    /// Flash LED is off
    #[default]
    Off,
    /// Flash fires during photo capture (LED on briefly before shutter)
    On,
    /// Torch / flashlight mode (LED stays on continuously)
    Torch,
}

impl FlashMode {
    /// Cycle to the next mode: Off -> On -> Torch -> Off
    pub fn next(self) -> Self {
        match self {
            FlashMode::Off => FlashMode::On,
            FlashMode::On => FlashMode::Torch,
            FlashMode::Torch => FlashMode::Off,
        }
    }
}

/// A flash LED device discovered via sysfs
#[derive(Debug, Clone)]
pub struct FlashDevice {
    /// Sysfs path, e.g. `/sys/class/leds/white:flash`
    path: PathBuf,
    /// Maximum brightness value (from `max_brightness` file)
    max_brightness: u32,
    /// Human-readable name (directory basename)
    name: String,
}

impl FlashDevice {
    /// Scan `/sys/class/leds/` for entries matching `*:flash` and return
    /// all devices that we can write to.
    pub fn discover() -> Vec<FlashDevice> {
        let leds_dir = Path::new("/sys/class/leds");
        let Ok(entries) = std::fs::read_dir(leds_dir) else {
            warn!("Cannot read /sys/class/leds — flash discovery skipped");
            return Vec::new();
        };

        let mut devices = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };

            // Match entries like "white:flash", "yellow:flash"
            if !name_str.ends_with(":flash") {
                continue;
            }

            let led_path = entry.path();
            let brightness_path = led_path.join("brightness");
            let max_brightness_path = led_path.join("max_brightness");

            // Read max_brightness
            let max_brightness = match std::fs::read_to_string(&max_brightness_path) {
                Ok(s) => match s.trim().parse::<u32>() {
                    Ok(v) if v > 0 => v,
                    _ => {
                        warn!(
                            path = %max_brightness_path.display(),
                            "Invalid max_brightness value"
                        );
                        continue;
                    }
                },
                Err(e) => {
                    warn!(
                        path = %max_brightness_path.display(),
                        error = %e,
                        "Cannot read max_brightness"
                    );
                    continue;
                }
            };

            // Verify we can write to brightness
            match std::fs::OpenOptions::new()
                .write(true)
                .open(&brightness_path)
            {
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        path = %brightness_path.display(),
                        error = %e,
                        "Cannot write brightness — user may need to be in 'feedbackd' group"
                    );
                    continue;
                }
            }

            info!(name = name_str, max_brightness, "Discovered flash LED");

            devices.push(FlashDevice {
                path: led_path,
                max_brightness,
                name: name_str.to_string(),
            });
        }

        // Sort by name for deterministic ordering (white before yellow)
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        devices
    }

    /// Get the device name (e.g. "white:flash")
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set raw brightness value (0 = off, max_brightness = full)
    pub fn set_brightness(&self, value: u32) -> io::Result<()> {
        let clamped = value.min(self.max_brightness);
        std::fs::write(self.path.join("brightness"), clamped.to_string())
    }

    /// Turn off the LED
    pub fn off(&self) -> io::Result<()> {
        self.set_brightness(0)
    }

    /// Turn on at a fraction of max brightness (0.0 = off, 1.0 = full)
    pub fn torch(&self, intensity: f32) -> io::Result<()> {
        let clamped = intensity.clamp(0.0, 1.0);
        let value = (clamped * self.max_brightness as f32).round() as u32;
        self.set_brightness(value)
    }
}

/// Result of hardware flash detection.
///
/// Separates "hardware exists" from "we can control it" so the UI can show
/// a helpful permission error instead of silently hiding the flash button.
pub struct FlashHardware {
    /// Devices we can actually control (writable)
    pub devices: Vec<FlashDevice>,
    /// User-facing error if hardware was found but not writable
    pub permission_error: Option<String>,
}

impl FlashHardware {
    /// Scan `/sys/class/leds/` for `*:flash` entries.
    ///
    /// Always detects hardware presence. If LEDs exist but the brightness
    /// file is not writable, builds a user-friendly error message with
    /// the correct privilege escalation command and group name.
    pub fn detect() -> FlashHardware {
        let leds_dir = Path::new("/sys/class/leds");
        let Ok(entries) = std::fs::read_dir(leds_dir) else {
            warn!("Cannot read /sys/class/leds — flash discovery skipped");
            return FlashHardware {
                devices: Vec::new(),
                permission_error: None,
            };
        };

        let mut devices = Vec::new();
        let mut permission_failures: Vec<(String, PathBuf)> = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };

            if !name_str.ends_with(":flash") {
                continue;
            }

            let led_path = entry.path();
            let brightness_path = led_path.join("brightness");
            let max_brightness_path = led_path.join("max_brightness");

            // Read max_brightness
            let max_brightness = match std::fs::read_to_string(&max_brightness_path) {
                Ok(s) => match s.trim().parse::<u32>() {
                    Ok(v) if v > 0 => v,
                    _ => {
                        warn!(
                            path = %max_brightness_path.display(),
                            "Invalid max_brightness value"
                        );
                        continue;
                    }
                },
                Err(e) => {
                    warn!(
                        path = %max_brightness_path.display(),
                        error = %e,
                        "Cannot read max_brightness"
                    );
                    continue;
                }
            };

            // Attempt write access
            match std::fs::OpenOptions::new()
                .write(true)
                .open(&brightness_path)
            {
                Ok(_) => {
                    info!(name = name_str, max_brightness, "Discovered flash LED");
                    devices.push(FlashDevice {
                        path: led_path,
                        max_brightness,
                        name: name_str.to_string(),
                    });
                }
                Err(_) => {
                    warn!(
                        path = %brightness_path.display(),
                        "Flash LED found but not writable"
                    );
                    permission_failures.push((name_str.to_string(), brightness_path));
                }
            }
        }

        devices.sort_by(|a, b| a.name.cmp(&b.name));

        // Build permission error message if we found hardware but can't write
        let permission_error = if !permission_failures.is_empty() && devices.is_empty() {
            Some(Self::build_permission_error(&permission_failures))
        } else {
            None
        };

        FlashHardware {
            devices,
            permission_error,
        }
    }

    /// Whether any controllable flash devices were found
    pub fn has_devices(&self) -> bool {
        !self.devices.is_empty()
    }

    /// Whether there is a permission error to show
    pub fn has_error(&self) -> bool {
        self.permission_error.is_some()
    }

    /// Build a user-friendly permission error message.
    ///
    /// Dynamically detects the current username, the required group from
    /// file ownership, and whether `doas` or `sudo` is available.
    fn build_permission_error(failures: &[(String, PathBuf)]) -> String {
        let username = std::env::var("USER").unwrap_or_else(|_| "user".to_string());

        let escalation_tool = if Path::new("/usr/bin/doas").exists() {
            "doas"
        } else {
            "sudo"
        };

        // Try to detect the group from the first failed brightness file
        let group = failures
            .first()
            .and_then(|(_, path)| {
                let meta = std::fs::metadata(path).ok()?;
                let gid = meta.gid();
                // Resolve GID to group name by reading /etc/group
                let group_contents = std::fs::read_to_string("/etc/group").ok()?;
                for line in group_contents.lines() {
                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() >= 3 && parts[2].parse::<u32>().ok() == Some(gid) {
                        return Some(parts[0].to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| "feedbackd".to_string());

        format!(
            "Flash LEDs detected but cannot be controlled.\n\n\
             Run: {escalation_tool} adduser {username} {group}\n\n\
             Then log out and back in."
        )
    }
}

/// Turn on all discovered flash devices at full brightness
pub fn all_on(devices: &[FlashDevice]) {
    for dev in devices {
        if let Err(e) = dev.torch(1.0) {
            warn!(device = %dev.name, error = %e, "Failed to turn on flash LED");
        }
    }
}

/// Turn off all discovered flash devices
pub fn all_off(devices: &[FlashDevice]) {
    for dev in devices {
        if let Err(e) = dev.off() {
            warn!(device = %dev.name, error = %e, "Failed to turn off flash LED");
        }
    }
}
