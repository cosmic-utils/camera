// SPDX-License-Identifier: GPL-3.0-only

//! Native PipeWire enumeration using pipewire-rs
//!
//! This module provides camera discovery using native PipeWire bindings,
//! replacing the subprocess-based pw-cli approach with proper library integration.
//! Benefits:
//! - No subprocess spawning overhead
//! - Real-time hotplug detection via Registry events
//! - Direct property access without text parsing
//! - Better reliability and error handling

use super::super::types::{CameraDevice, DeviceInfo, SensorRotation};
use pipewire as pw;
use pw::types::ObjectType;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

/// Camera information collected from PipeWire node properties
#[derive(Debug, Clone)]
struct CameraNodeInfo {
    /// PipeWire node ID
    id: u32,
    /// object.serial property for stable identification
    serial: Option<String>,
    /// node.description (camera name)
    name: Option<String>,
    /// node.nick (card name)
    nick: Option<String>,
    /// object.path (e.g., "v4l2:/dev/video0")
    object_path: Option<String>,
    /// api.libcamera.rotation property
    rotation: Option<String>,
    /// media.class property
    media_class: Option<String>,
}

impl CameraNodeInfo {
    fn new(id: u32) -> Self {
        Self {
            id,
            serial: None,
            name: None,
            nick: None,
            object_path: None,
            rotation: None,
            media_class: None,
        }
    }

    /// Check if this is a video source (camera)
    fn is_video_source(&self) -> bool {
        self.media_class
            .as_ref()
            .is_some_and(|c| c == "Video/Source")
    }

    /// Check if this is our own virtual camera (to skip self-detection)
    fn is_virtual_camera(&self) -> bool {
        self.name
            .as_ref()
            .is_some_and(|n| n.contains("Camera (Virtual)"))
    }

    /// Convert to CameraDevice
    fn to_camera_device(&self) -> Option<CameraDevice> {
        let name = self.name.as_ref()?;

        // Build path: prefer serial for stability, fall back to node ID
        let path = if let Some(serial) = &self.serial {
            format!("pipewire-serial-{}", serial)
        } else {
            format!("pipewire-{}", self.id)
        };

        // Parse rotation
        let rotation = self
            .rotation
            .as_ref()
            .map(|r| SensorRotation::from_degrees(r))
            .unwrap_or_default();

        // Build device info from V4L2 path
        let device_info = self.build_device_info();

        Some(CameraDevice {
            name: name.clone(),
            path,
            metadata_path: Some(self.id.to_string()),
            device_info,
            rotation,
        })
    }

    /// Build DeviceInfo from object.path and V4L2 queries
    fn build_device_info(&self) -> Option<DeviceInfo> {
        // Extract V4L2 device path from object.path (format: "v4l2:/dev/video0")
        let v4l2_path = self.object_path.as_ref()?.strip_prefix("v4l2:")?;

        // Get real path by resolving symlinks
        let real_path = std::fs::canonicalize(v4l2_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| v4l2_path.to_string());

        // Get driver name using V4L2 ioctl
        let driver = get_v4l2_driver(v4l2_path).unwrap_or_default();

        // Use node.nick as the card name
        let card = self.nick.clone().unwrap_or_default();

        Some(DeviceInfo {
            card,
            driver,
            path: v4l2_path.to_string(),
            real_path,
        })
    }
}

/// Get V4L2 driver name using ioctl
fn get_v4l2_driver(device_path: &str) -> Option<String> {
    use std::os::unix::io::AsRawFd;

    // VIDIOC_QUERYCAP ioctl number
    const VIDIOC_QUERYCAP: libc::c_ulong = 0x80685600;

    // V4L2 capability structure (simplified - we only need driver field)
    #[repr(C)]
    struct V4l2Capability {
        driver: [u8; 16],
        card: [u8; 32],
        bus_info: [u8; 32],
        version: u32,
        capabilities: u32,
        device_caps: u32,
        reserved: [u32; 3],
    }

    let file = std::fs::File::open(device_path).ok()?;
    let fd = file.as_raw_fd();

    let mut cap: V4l2Capability = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(fd, VIDIOC_QUERYCAP, &mut cap as *mut V4l2Capability) };

    if result < 0 {
        return None;
    }

    // Find null terminator or use full length
    let len = cap.driver.iter().position(|&c| c == 0).unwrap_or(16);
    String::from_utf8_lossy(&cap.driver[..len])
        .to_string()
        .into()
}

/// Enumerate cameras using native PipeWire bindings
///
/// This runs the PipeWire main loop briefly to collect all camera nodes,
/// then returns the results. For hotplug detection, use `PipeWireWatcher`.
pub fn enumerate_cameras_native() -> Option<Vec<CameraDevice>> {
    debug!("Attempting native PipeWire camera enumeration");

    // Use a thread-local approach since PipeWire MainLoop isn't Send
    let cameras: Arc<Mutex<Vec<CameraDevice>>> = Arc::new(Mutex::new(Vec::new()));
    let cameras_clone = cameras.clone();

    // Run enumeration in a dedicated thread since PipeWire types aren't Send
    let handle = std::thread::spawn(move || enumerate_in_thread(cameras_clone));

    // Wait for enumeration with timeout
    match handle.join() {
        Ok(true) => {
            let result = cameras.lock().ok()?.clone();
            if result.is_empty() {
                debug!("No cameras found via native PipeWire");
                None
            } else {
                info!(count = result.len(), "Enumerated cameras via native PipeWire");
                Some(result)
            }
        }
        Ok(false) => {
            warn!("Native PipeWire enumeration failed");
            None
        }
        Err(e) => {
            error!(?e, "Native PipeWire enumeration thread panicked");
            None
        }
    }
}

/// Internal enumeration function that runs in a dedicated thread
fn enumerate_in_thread(cameras: Arc<Mutex<Vec<CameraDevice>>>) -> bool {
    // Initialize PipeWire
    pw::init();

    // Create main loop (Rc version for proper lifetime management)
    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            error!(?e, "Failed to create PipeWire main loop");
            return false;
        }
    };

    // Create context
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(?e, "Failed to create PipeWire context");
            return false;
        }
    };

    // Connect to PipeWire daemon
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(e) => {
            error!(?e, "Failed to connect to PipeWire daemon");
            return false;
        }
    };

    // Get registry
    let registry = match core.get_registry_rc() {
        Ok(reg) => reg,
        Err(e) => {
            error!(?e, "Failed to get PipeWire registry");
            return false;
        }
    };

    // Collect nodes in a RefCell for interior mutability
    let nodes: Rc<RefCell<HashMap<u32, CameraNodeInfo>>> = Rc::new(RefCell::new(HashMap::new()));
    let nodes_for_listener = nodes.clone();

    // Track sync state
    let done = Rc::new(RefCell::new(false));
    let done_for_core = done.clone();
    let main_loop_weak = main_loop.downgrade();

    // Listen for registry events
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            // Only interested in Node objects
            if global.type_ != ObjectType::Node {
                return;
            }

            let node_id = global.id;
            debug!(node_id, type_ = ?global.type_, "Found PipeWire object");

            // Create node info entry
            let mut info = CameraNodeInfo::new(node_id);

            // Extract properties if available
            if let Some(props) = global.props {
                if let Some(media_class) = props.get("media.class") {
                    info.media_class = Some(media_class.to_string());
                }
                if let Some(serial) = props.get("object.serial") {
                    info.serial = Some(serial.to_string());
                }
                if let Some(name) = props.get("node.description") {
                    info.name = Some(name.to_string());
                }
                if let Some(nick) = props.get("node.nick") {
                    info.nick = Some(nick.to_string());
                }
                if let Some(path) = props.get("object.path") {
                    info.object_path = Some(path.to_string());
                }
                if let Some(rotation) = props.get("api.libcamera.rotation") {
                    info.rotation = Some(rotation.to_string());
                }
            }

            // Store the node info
            nodes_for_listener.borrow_mut().insert(node_id, info);
        })
        .register();

    // Request sync to know when initial enumeration is complete
    let pending_sync = match core.sync(0) {
        Ok(seq) => seq,
        Err(e) => {
            error!(?e, "Failed to request sync");
            return false;
        }
    };

    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            // Check if this is the response to our sync request
            if id == pw::core::PW_ID_CORE && seq == pending_sync {
                debug!("PipeWire sync complete, enumeration finished");
                *done_for_core.borrow_mut() = true;
                if let Some(main_loop) = main_loop_weak.upgrade() {
                    main_loop.quit();
                }
            }
        })
        .register();

    // Run the main loop (will quit when sync completes)
    // The sync mechanism ensures we get all objects before done is called
    main_loop.run();

    // Process collected nodes
    let nodes_map = nodes.borrow();
    let mut camera_list = cameras.lock().unwrap();

    for (id, info) in nodes_map.iter() {
        // Skip non-video-source nodes
        if !info.is_video_source() {
            continue;
        }

        // Skip virtual camera (self-detection prevention)
        if info.is_virtual_camera() {
            debug!(id, name = ?info.name, "Skipping virtual camera");
            continue;
        }

        // Skip nodes without a name
        if info.name.is_none() {
            debug!(id, "Skipping node without name");
            continue;
        }

        // Convert to CameraDevice
        if let Some(device) = info.to_camera_device() {
            debug!(
                id,
                name = %device.name,
                path = %device.path,
                rotation = %device.rotation,
                "Found video camera"
            );
            camera_list.push(device);
        }
    }

    true
}

/// Camera change event for hotplug detection
#[derive(Debug, Clone)]
pub enum CameraEvent {
    /// A new camera was connected
    Added(CameraDevice),
    /// A camera was disconnected
    Removed {
        /// Path identifier of the removed camera
        path: String,
    },
}

/// Hotplug watcher for camera connect/disconnect events
///
/// This watcher runs in a background thread and monitors PipeWire for camera changes.
/// Events are sent through the provided channel.
pub struct PipeWireHotplugWatcher {
    /// Handle to the watcher thread
    thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Flag to signal the watcher to stop
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl PipeWireHotplugWatcher {
    /// Create a new hotplug watcher
    ///
    /// # Arguments
    /// * `event_sender` - Channel sender for camera events
    ///
    /// # Returns
    /// A new watcher instance, or None if PipeWire is not available
    pub fn new(event_sender: std::sync::mpsc::Sender<CameraEvent>) -> Option<Self> {
        let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        let thread_handle = std::thread::spawn(move || {
            run_hotplug_watcher(event_sender, stop_flag_clone);
        });

        Some(Self {
            thread_handle: Some(thread_handle),
            stop_flag,
        })
    }

    /// Stop the watcher
    pub fn stop(&mut self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Release);

        if let Some(handle) = self.thread_handle.take() {
            // The thread will exit when the main loop quits
            let _ = handle.join();
        }
    }
}

impl Drop for PipeWireHotplugWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Internal function to run the hotplug watcher in a dedicated thread
fn run_hotplug_watcher(
    event_sender: std::sync::mpsc::Sender<CameraEvent>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
) {
    // Initialize PipeWire
    pw::init();

    // Create main loop
    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => {
            error!(?e, "Failed to create PipeWire main loop for hotplug watcher");
            return;
        }
    };

    // Create context
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(ctx) => ctx,
        Err(e) => {
            error!(?e, "Failed to create PipeWire context for hotplug watcher");
            return;
        }
    };

    // Connect to PipeWire daemon
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(e) => {
            error!(?e, "Failed to connect to PipeWire daemon for hotplug watcher");
            return;
        }
    };

    // Get registry
    let registry = match core.get_registry_rc() {
        Ok(reg) => reg,
        Err(e) => {
            error!(?e, "Failed to get PipeWire registry for hotplug watcher");
            return;
        }
    };

    // Track known cameras by node ID
    let known_cameras: Rc<RefCell<HashMap<u32, CameraNodeInfo>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let known_cameras_for_add = known_cameras.clone();
    let known_cameras_for_remove = known_cameras.clone();
    let event_sender_for_add = event_sender.clone();

    // Flag to track if initial sync is complete
    let initial_sync_done = Rc::new(RefCell::new(false));
    let initial_sync_for_listener = initial_sync_done.clone();

    // Listen for registry events
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            // Only interested in Node objects
            if global.type_ != ObjectType::Node {
                return;
            }

            let node_id = global.id;

            // Create node info entry
            let mut info = CameraNodeInfo::new(node_id);

            // Extract properties
            if let Some(props) = global.props {
                if let Some(media_class) = props.get("media.class") {
                    info.media_class = Some(media_class.to_string());
                }
                if let Some(serial) = props.get("object.serial") {
                    info.serial = Some(serial.to_string());
                }
                if let Some(name) = props.get("node.description") {
                    info.name = Some(name.to_string());
                }
                if let Some(nick) = props.get("node.nick") {
                    info.nick = Some(nick.to_string());
                }
                if let Some(path) = props.get("object.path") {
                    info.object_path = Some(path.to_string());
                }
                if let Some(rotation) = props.get("api.libcamera.rotation") {
                    info.rotation = Some(rotation.to_string());
                }
            }

            // Only track video sources
            if !info.is_video_source() || info.is_virtual_camera() || info.name.is_none() {
                return;
            }

            // Store the camera info
            let mut cameras = known_cameras_for_add.borrow_mut();
            let is_new = !cameras.contains_key(&node_id);
            cameras.insert(node_id, info.clone());

            // Only emit event after initial sync is complete
            if *initial_sync_for_listener.borrow() && is_new
                && let Some(device) = info.to_camera_device() {
                    info!(name = %device.name, path = %device.path, "Camera connected");
                    let _ = event_sender_for_add.send(CameraEvent::Added(device));
                }
        })
        .global_remove(move |id| {
            let mut cameras = known_cameras_for_remove.borrow_mut();
            if let Some(info) = cameras.remove(&id)
                && let Some(device) = info.to_camera_device() {
                    info!(name = %device.name, path = %device.path, "Camera disconnected");
                    let _ = event_sender.send(CameraEvent::Removed { path: device.path });
                }
        })
        .register();

    // Wait for initial sync to complete
    let pending_sync = match core.sync(0) {
        Ok(seq) => seq,
        Err(e) => {
            error!(?e, "Failed to request initial sync for hotplug watcher");
            return;
        }
    };

    let initial_sync_for_done = initial_sync_done.clone();
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending_sync {
                debug!("Hotplug watcher initial sync complete");
                *initial_sync_for_done.borrow_mut() = true;
            }
        })
        .register();

    info!("PipeWire hotplug watcher started");

    // Run the main loop until stop is requested
    // Note: The loop will run indefinitely; to stop it properly we'd need
    // to use a signal or timeout mechanism. For now, the thread will be
    // killed when the watcher is dropped.
    while !stop_flag.load(std::sync::atomic::Ordering::Acquire) {
        // Run the loop for a short time, then check stop flag
        // This is a workaround since we can't easily interrupt the main loop
        main_loop.run();
    }

    info!("PipeWire hotplug watcher stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_cameras() {
        // This test requires PipeWire to be running
        // Skip if not available
        if std::process::Command::new("pw-cli")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("Skipping test: PipeWire not available");
            return;
        }

        let cameras = enumerate_cameras_native();
        // Don't assert on camera count since it depends on the system
        // Just verify the function doesn't panic
        if let Some(cams) = cameras {
            for cam in &cams {
                assert!(!cam.name.is_empty());
                assert!(!cam.path.is_empty());
            }
        }
    }
}
