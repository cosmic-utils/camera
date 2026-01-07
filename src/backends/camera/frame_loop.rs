// SPDX-License-Identifier: GPL-3.0-only
//! Thread lifecycle management for capture loops
//!
//! This module provides a standardized way to manage capture loop threads
//! across different depth camera backends, reducing code duplication and
//! ensuring consistent thread lifecycle handling.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use tracing::{debug, info, warn};

/// Action returned by the capture loop callback to control loop behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopAction {
    /// Continue running the loop
    Continue,
    /// Stop the loop gracefully
    Stop,
}

/// Controller for a capture loop running in a separate thread
///
/// This provides a standardized interface for starting, stopping, and
/// managing the lifecycle of capture loop threads.
///
/// # Example
///
/// ```ignore
/// let controller = CaptureLoopController::start("depth-capture", || {
///     // Capture and process a frame
///     match capture_frame() {
///         Ok(frame) => {
///             process_frame(frame);
///             LoopAction::Continue
///         }
///         Err(e) => {
///             warn!("Capture error: {}", e);
///             LoopAction::Continue // Keep trying
///         }
///     }
/// });
///
/// // Later, stop the loop
/// controller.stop();
/// ```
pub struct CaptureLoopController {
    /// Thread handle for joining
    thread_handle: Option<JoinHandle<()>>,
    /// Signal to stop the loop
    stop_signal: Arc<AtomicBool>,
    /// Name for logging
    name: String,
}

impl CaptureLoopController {
    /// Start a new capture loop in a separate thread
    ///
    /// The provided closure is called repeatedly until it returns `LoopAction::Stop`
    /// or the controller's `stop()` method is called.
    ///
    /// # Arguments
    ///
    /// * `name` - A descriptive name for the loop (used in logging)
    /// * `loop_fn` - A closure that performs one iteration of the capture loop
    ///
    /// # Returns
    ///
    /// A controller that can be used to stop the loop and wait for it to finish.
    pub fn start<F>(name: &str, mut loop_fn: F) -> Self
    where
        F: FnMut() -> LoopAction + Send + 'static,
    {
        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_signal_clone = Arc::clone(&stop_signal);
        let name_clone = name.to_string();

        info!(name = %name, "Starting capture loop");

        let thread_handle = thread::spawn(move || {
            debug!(name = %name_clone, "Capture loop thread started");

            loop {
                // Check stop signal first
                if stop_signal_clone.load(Ordering::SeqCst) {
                    debug!(name = %name_clone, "Stop signal received");
                    break;
                }

                // Execute one iteration
                match loop_fn() {
                    LoopAction::Continue => {}
                    LoopAction::Stop => {
                        debug!(name = %name_clone, "Loop requested stop");
                        break;
                    }
                }
            }

            info!(name = %name_clone, "Capture loop thread exiting");
        });

        Self {
            thread_handle: Some(thread_handle),
            stop_signal,
            name: name.to_string(),
        }
    }

    /// Start a capture loop with initialization
    ///
    /// The `init_fn` is called once at the start of the thread to set up
    /// resources. If initialization fails, the thread exits immediately.
    ///
    /// # Arguments
    ///
    /// * `name` - A descriptive name for the loop
    /// * `init_fn` - Initialization closure, returns Ok(state) or Err(message)
    /// * `loop_fn` - Loop closure that receives the state and returns LoopAction
    pub fn start_with_init<S, I, F>(name: &str, init_fn: I, mut loop_fn: F) -> Self
    where
        S: Send + 'static,
        I: FnOnce() -> Result<S, String> + Send + 'static,
        F: FnMut(&mut S) -> LoopAction + Send + 'static,
    {
        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_signal_clone = Arc::clone(&stop_signal);
        let name_clone = name.to_string();

        info!(name = %name, "Starting capture loop with initialization");

        let thread_handle = thread::spawn(move || {
            debug!(name = %name_clone, "Capture loop thread started, initializing...");

            // Run initialization
            let mut state = match init_fn() {
                Ok(s) => {
                    debug!(name = %name_clone, "Initialization successful");
                    s
                }
                Err(e) => {
                    warn!(name = %name_clone, error = %e, "Initialization failed");
                    return;
                }
            };

            loop {
                if stop_signal_clone.load(Ordering::SeqCst) {
                    debug!(name = %name_clone, "Stop signal received");
                    break;
                }

                match loop_fn(&mut state) {
                    LoopAction::Continue => {}
                    LoopAction::Stop => {
                        debug!(name = %name_clone, "Loop requested stop");
                        break;
                    }
                }
            }

            info!(name = %name_clone, "Capture loop thread exiting");
        });

        Self {
            thread_handle: Some(thread_handle),
            stop_signal,
            name: name.to_string(),
        }
    }

    /// Check if the loop is still running
    pub fn is_running(&self) -> bool {
        self.thread_handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    /// Get a clone of the stop signal for external use
    ///
    /// This can be passed to capture functions that need to check
    /// for stop requests within long-running operations.
    pub fn stop_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop_signal)
    }

    /// Signal the loop to stop (non-blocking)
    ///
    /// This sets the stop signal but doesn't wait for the thread to finish.
    /// Use `stop()` or `stop_and_wait()` if you need to wait.
    pub fn request_stop(&self) {
        debug!(name = %self.name, "Requesting capture loop stop");
        self.stop_signal.store(true, Ordering::SeqCst);
    }

    /// Stop the loop and wait for the thread to finish
    ///
    /// This is the preferred way to stop a capture loop as it ensures
    /// clean shutdown before returning.
    pub fn stop(&mut self) {
        self.request_stop();
        self.join();
    }

    /// Wait for the thread to finish without sending stop signal
    ///
    /// Useful if the loop stops itself via `LoopAction::Stop`.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            debug!(name = %self.name, "Waiting for capture loop thread to finish");
            if let Err(e) = handle.join() {
                warn!(name = %self.name, "Capture loop thread panicked: {:?}", e);
            } else {
                debug!(name = %self.name, "Capture loop thread finished");
            }
        }
    }
}

impl Drop for CaptureLoopController {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            debug!(name = %self.name, "CaptureLoopController dropped, stopping loop");
            self.stop();
        }
    }
}

/// Builder for creating capture loops with common configuration
pub struct CaptureLoopBuilder {
    name: String,
}

impl CaptureLoopBuilder {
    /// Create a new builder with the given loop name
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }

    /// Start the capture loop with the given closure
    pub fn start<F>(self, loop_fn: F) -> CaptureLoopController
    where
        F: FnMut() -> LoopAction + Send + 'static,
    {
        CaptureLoopController::start(&self.name, loop_fn)
    }

    /// Start the capture loop with initialization
    pub fn start_with_init<S, I, F>(self, init_fn: I, loop_fn: F) -> CaptureLoopController
    where
        S: Send + 'static,
        I: FnOnce() -> Result<S, String> + Send + 'static,
        F: FnMut(&mut S) -> LoopAction + Send + 'static,
    {
        CaptureLoopController::start_with_init(&self.name, init_fn, loop_fn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    #[test]
    fn test_basic_loop() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let mut controller = CaptureLoopController::start("test-loop", move || {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            if count >= 10 {
                LoopAction::Stop
            } else {
                LoopAction::Continue
            }
        });

        // Wait for loop to finish itself
        controller.join();

        assert_eq!(counter.load(Ordering::SeqCst), 11); // 0-10 inclusive
    }

    #[test]
    fn test_stop_signal() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let mut controller = CaptureLoopController::start("test-loop", move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(10));
            LoopAction::Continue
        });

        // Let it run a bit
        thread::sleep(Duration::from_millis(50));

        // Stop and verify it ran at least once
        controller.stop();
        assert!(counter.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn test_with_init() {
        let result = Arc::new(AtomicU32::new(0));
        let result_clone = Arc::clone(&result);

        let mut controller = CaptureLoopController::start_with_init(
            "test-init-loop",
            || Ok(42u32), // Init returns 42
            move |state| {
                result_clone.store(*state, Ordering::SeqCst);
                LoopAction::Stop
            },
        );

        controller.join();
        assert_eq!(result.load(Ordering::SeqCst), 42);
    }

    #[test]
    fn test_init_failure() {
        let ran = Arc::new(AtomicBool::new(false));
        let ran_clone = Arc::clone(&ran);

        let mut controller = CaptureLoopController::start_with_init(
            "test-fail-init",
            || Err::<(), _>("Init failed".to_string()),
            move |_: &mut ()| {
                ran_clone.store(true, Ordering::SeqCst);
                LoopAction::Stop
            },
        );

        controller.join();
        // Loop function should never run if init fails
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[test]
    fn test_is_running() {
        let controller = CaptureLoopController::start("test-running", || {
            thread::sleep(Duration::from_millis(100));
            LoopAction::Continue
        });

        assert!(controller.is_running());

        // Drop will stop it
        drop(controller);
    }
}
