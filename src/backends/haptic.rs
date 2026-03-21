// SPDX-License-Identifier: GPL-3.0-only

//! Haptic feedback via Linux force-feedback (evdev)
//!
//! Detects haptic devices (vibration motors) at startup and provides
//! a simple API to trigger short vibration pulses for UI feedback.

use std::fs;
use std::os::unix::io::AsRawFd;
use std::sync::OnceLock;
use tracing::{debug, info, warn};

/// Cached haptic device path (detected once at first use)
static HAPTIC_DEVICE: OnceLock<Option<String>> = OnceLock::new();

/// Linux input event types and ioctl constants
const EV_FF: u16 = 0x15;
const FF_RUMBLE: u16 = 0x50;

/// ioctl request codes
/// EVIOCSFF = _IOW('E', 0x80, struct ff_effect) = 0x40304580
const EVIOCSFF: u64 = 0x40304580;
/// EVIOCRMFF = _IOW('E', 0x81, int) = 0x40044581
const EVIOCRMFF: u64 = 0x40044581;

/// Build a 48-byte ff_effect struct for FF_RUMBLE.
///
/// Kernel layout (from linux/input.h):
///   offset 0:  type (u16)
///   offset 2:  id (i16)
///   offset 4:  direction (u16)
///   offset 6:  trigger.button (u16)
///   offset 8:  trigger.interval (u16)
///   offset 10: replay.length (u16)
///   offset 12: replay.delay (u16)
///   offset 16: union — ff_rumble_effect.strong_magnitude (u16)
///   offset 18: union — ff_rumble_effect.weak_magnitude (u16)
fn build_ff_rumble(duration_ms: u16, strong: u16) -> [u8; 48] {
    let mut buf = [0u8; 48];
    buf[0..2].copy_from_slice(&FF_RUMBLE.to_ne_bytes()); // type = FF_RUMBLE
    buf[2..4].copy_from_slice(&(-1i16).to_ne_bytes()); // id = -1 (new)
    buf[10..12].copy_from_slice(&duration_ms.to_ne_bytes()); // replay.length
    buf[16..18].copy_from_slice(&strong.to_ne_bytes()); // strong_magnitude
    buf
}

/// Build a 24-byte input_event struct (64-bit Linux).
fn build_input_event(type_: u16, code: u16, value: i32) -> [u8; 24] {
    let mut buf = [0u8; 24];
    // tv_sec (i64) = 0, tv_usec (i64) = 0 — already zeroed
    buf[16..18].copy_from_slice(&type_.to_ne_bytes());
    buf[18..20].copy_from_slice(&code.to_ne_bytes());
    buf[20..24].copy_from_slice(&value.to_ne_bytes());
    buf
}

/// Detect a haptic feedback device by scanning /dev/input/event* for
/// devices with "haptic" in their name.
fn detect_haptic_device() -> Option<String> {
    let entries = fs::read_dir("/dev/input").ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name()?.to_str()?;
        if !name.starts_with("event") {
            continue;
        }

        // Read device name from sysfs
        let sysfs_name = format!("/sys/class/input/{}/device/name", name);
        if let Ok(device_name) = fs::read_to_string(&sysfs_name) {
            let device_name = device_name.trim().to_lowercase();
            if device_name.contains("haptic") {
                let device_path = path.to_string_lossy().to_string();
                info!(device = %device_path, name = %device_name.trim(), "Haptic device detected");
                return Some(device_path);
            }
        }
    }

    debug!("No haptic device found");
    None
}

/// Get the cached haptic device path, detecting on first call.
fn get_haptic_device() -> Option<&'static str> {
    HAPTIC_DEVICE.get_or_init(detect_haptic_device).as_deref()
}

/// Check if haptic feedback is available on this device.
pub fn is_available() -> bool {
    get_haptic_device().is_some()
}

/// Trigger a short haptic pulse.
///
/// `duration_ms` controls the vibration length (typically 15-30ms for a tap).
/// This is non-blocking — the vibration runs asynchronously via the kernel.
pub fn vibrate(duration_ms: u16) {
    let Some(device_path) = get_haptic_device() else {
        return;
    };

    // Open device, upload effect, play, close — all in a background thread
    // to avoid blocking the UI thread on the file I/O.
    let device_path = device_path.to_string();
    std::thread::spawn(move || {
        if let Err(e) = vibrate_sync(&device_path, duration_ms) {
            warn!(error = %e, "Haptic feedback failed");
        }
    });
}

fn vibrate_sync(device_path: &str, duration_ms: u16) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(device_path)
        .map_err(|e| format!("open {device_path}: {e}"))?;

    let fd = file.as_raw_fd();

    // Upload FF_RUMBLE effect
    let mut effect = build_ff_rumble(duration_ms, 0x4000);

    #[allow(clippy::unnecessary_cast)]
    let ret = unsafe { libc::ioctl(fd, EVIOCSFF as _, effect.as_mut_ptr()) };
    if ret < 0 {
        return Err(format!("EVIOCSFF: {}", std::io::Error::last_os_error()));
    }

    // Read back the assigned effect ID
    let effect_id = i16::from_ne_bytes([effect[2], effect[3]]);

    // Play the effect
    let play_ev = build_input_event(EV_FF, effect_id as u16, 1);
    let ret = unsafe { libc::write(fd, play_ev.as_ptr() as *const libc::c_void, play_ev.len()) };
    if ret < 0 {
        return Err(format!("write play: {}", std::io::Error::last_os_error()));
    }

    // Wait for effect to complete, then clean up
    std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 10));

    // Remove effect (ignore errors — some drivers don't support this)
    let eid = effect_id as libc::c_int;
    #[allow(clippy::unnecessary_cast)]
    unsafe {
        libc::ioctl(fd, EVIOCRMFF as _, &eid as *const libc::c_int);
    }

    Ok(())
}
