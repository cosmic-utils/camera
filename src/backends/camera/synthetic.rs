// SPDX-License-Identifier: GPL-3.0-only

//! Synthetic camera devices for the screenshot preview harness.
//!
//! The harness feeds frames from a still image via `--preview-source`, which
//! needs no camera hardware and, crucially, no dma-buf provider (`/dev/udmabuf`
//! or a dma_heap). On its own though the app falls back to its "no camera" UI:
//! almost every control is gated on the enumerated device list being non-empty
//! (the preview itself, the camera switcher, the resolution/framerate/mode
//! pickers), so a file source with no device shows an empty placeholder.
//!
//! `--preview-fake-camera` injects the devices below in place of real
//! enumeration so that chrome renders, while the actual frames still come from
//! the file source. Nothing here ever opens a camera: with a file source active
//! the camera subscription is disabled (see `AppModel::subscription`), so these
//! devices are pure metadata.

use crate::backends::camera::types::{CameraDevice, CameraFormat, Framerate};

/// Two synthetic cameras — a back and a front, so the camera switcher renders
/// (it only appears with more than one device) — plus a plausible set of
/// formats, for the `--preview-fake-camera` preview harness.
///
/// The frames themselves come from `--preview-source`; these values only drive
/// the chrome (camera list, resolution/framerate/mode pickers). The first entry
/// is the one the app starts on, so its `path` must be non-empty for
/// `pick_startup_camera_index` to select it.
pub fn synthetic_preview_cameras() -> (Vec<CameraDevice>, Vec<CameraFormat>) {
    let device = |name: &str, id: &str, location: &str| CameraDevice {
        name: name.to_string(),
        path: id.to_string(),
        camera_location: Some(location.to_string()),
        sensor_model: Some("Synthetic Preview Sensor".to_string()),
        // `device_info: None` on purpose: there is no V4L2 node behind these, so
        // the exposure/color tools (which query one) stay hidden, exactly as
        // they are for a camera that exposes no such controls.
        ..Default::default()
    };

    let cameras = vec![
        device("Back Camera", "preview-back", "back"),
        device("Front Camera", "preview-front", "front"),
    ];

    (cameras, synthetic_formats())
}

/// A small spread of common resolutions and framerates. Photo mode selects the
/// maximum resolution and video mode the highest resolution at >= 25 fps, so
/// both modes resolve to a sensible format.
fn synthetic_formats() -> Vec<CameraFormat> {
    let fmt = |width, height, fps, pixel_format: &str, hw| CameraFormat {
        width,
        height,
        framerate: Some(Framerate::from_int(fps)),
        hardware_accelerated: hw,
        pixel_format: pixel_format.to_string(),
    };

    vec![
        fmt(1920, 1080, 60, "MJPG", true),
        fmt(1920, 1080, 30, "MJPG", true),
        fmt(1280, 720, 60, "MJPG", true),
        fmt(1280, 720, 30, "MJPG", true),
        fmt(640, 480, 30, "YUYV", false),
    ]
}
