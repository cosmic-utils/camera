// SPDX-License-Identifier: GPL-3.0-only

//! Lightweight audio-level probe used by the Settings drawer to show a
//! microphone meter before recording starts.
//!
//! Spins up a minimal GStreamer pipeline that mirrors the recorder's audio
//! branch (minus the encoder and muxer): pulsesrc → audioconvert →
//! audioresample → level → mono capsfilter → level → fakesink. The shared
//! [`install_level_sync_handler`] writes peak/RMS values into a
//! [`SharedAudioLevels`] mutex that the UI then snapshots on a 100 ms tick.
//!
//! The probe is owned by `AppModel` and torn down whenever the settings
//! drawer closes, `record_audio` is toggled off, the selected device
//! changes, or a real recording starts.

use gstreamer as gst;
use gstreamer::prelude::*;
use tracing::{info, warn};

use crate::pipelines::audio_level::{
    PULSESRC_SLAVE_METHOD, SharedAudioLevels, install_level_sync_handler,
};

/// Running probe pipeline. Drop or call [`AudioLevelProbe::stop`] to tear down.
pub struct AudioLevelProbe {
    pipeline: gst::Pipeline,
    levels: SharedAudioLevels,
    device: Option<String>,
}

impl AudioLevelProbe {
    /// Build and start the probe. `device` is the PipeWire/PulseAudio node
    /// name (e.g. `"alsa_input.usb-…"`). Pass `None` to capture from the
    /// system default.
    pub fn start(device: Option<&str>) -> Result<Self, String> {
        let device_str = device
            .map(|d| format!("device=\"{}\" ", d.replace('"', "\\\"")))
            .unwrap_or_default();

        let desc = format!(
            "pulsesrc name=probe-src {device_str}slave-method={slave} do-timestamp=true provide-clock=false \
             ! audioconvert \
             ! audioresample \
             ! level name=audio-level-input post-messages=true interval=100000000 \
             ! capsfilter caps=audio/x-raw,channels=1 \
             ! level name=audio-level-output post-messages=true interval=100000000 \
             ! fakesink sync=false",
            slave = PULSESRC_SLAVE_METHOD,
        );

        info!(desc = %desc, "Starting audio-level probe pipeline");

        let pipeline = gst::parse::launch(&desc)
            .map_err(|e| format!("Failed to parse probe pipeline: {e}"))?
            .dynamic_cast::<gst::Pipeline>()
            .map_err(|_| "Failed to cast probe element to Pipeline".to_string())?;

        let levels: SharedAudioLevels = Default::default();
        install_level_sync_handler(&pipeline, &levels);

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| format!("Failed to start probe pipeline: {e}"))?;

        Ok(Self {
            pipeline,
            levels,
            device: device.map(|d| d.to_string()),
        })
    }

    /// Cloned `Arc` handle to the live level data.
    pub fn levels(&self) -> SharedAudioLevels {
        self.levels.clone()
    }

    /// The PipeWire node name the probe was started with, or `None` for
    /// the system default.
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// Stop the pipeline and release GStreamer resources.
    pub fn stop(self) {
        if let Some(bus) = self.pipeline.bus() {
            bus.unset_sync_handler();
        }
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            warn!(error = %e, "Failed to stop probe pipeline cleanly");
        }
    }
}

impl Drop for AudioLevelProbe {
    fn drop(&mut self) {
        if let Some(bus) = self.pipeline.bus() {
            bus.unset_sync_handler();
        }
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
