// SPDX-License-Identifier: GPL-3.0-only

//! Audio device enumeration for PipeWire

use std::process::Command;
use tracing::{debug, warn};

/// Per-channel information from PipeWire
#[derive(Debug, Clone)]
pub struct AudioChannelInfo {
    /// Channel position (e.g. "AUX0", "FL", "FR")
    pub position: String,
    /// PipeWire software volume (linear, 1.0 = unity)
    pub volume: f64,
    /// Volume in dB
    pub volume_db: f64,
}

/// Represents an audio input device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub serial: String,
    pub node_name: String,
    pub is_default: bool,
    /// Per-channel information from PipeWire
    pub channels: Vec<AudioChannelInfo>,
    /// Sample format (e.g. "S16LE", "S32LE", "F32LE")
    pub sample_format: String,
    /// Sample rate in Hz (e.g. 48000)
    pub sample_rate: u32,
}

/// Convert linear volume to dB
fn linear_to_db(vol: f64) -> f64 {
    if vol <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * vol.log10()
    }
}

/// Enumerate available audio input devices using PipeWire
pub fn enumerate_audio_devices() -> Vec<AudioDevice> {
    let output = match Command::new("pw-dump").output() {
        Ok(output) => output,
        Err(e) => {
            warn!("Failed to run pw-dump: {}", e);
            return Vec::new();
        }
    };

    if !output.status.success() {
        warn!("pw-dump command failed");
        return Vec::new();
    }

    let stdout = match std::str::from_utf8(&output.stdout) {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to parse pw-dump output: {}", e);
            return Vec::new();
        }
    };

    // Parse JSON output
    let nodes: Vec<serde_json::Value> = match serde_json::from_str(stdout) {
        Ok(nodes) => nodes,
        Err(e) => {
            warn!("Failed to parse JSON from pw-dump: {}", e);
            return Vec::new();
        }
    };

    let mut devices = Vec::new();
    let mut default_node_name: Option<String> = None;

    // First pass: find the default audio source from metadata
    for node in &nodes {
        if node.get("type").and_then(|v| v.as_str()) == Some("PipeWire:Interface:Metadata")
            && let Some(props) = node.get("props")
            && props.get("metadata.name").and_then(|v| v.as_str()) == Some("default")
        {
            if let Some(metadata) = node.get("metadata").and_then(|v| v.as_array()) {
                for entry in metadata {
                    if (entry.get("key").and_then(|v| v.as_str()) == Some("default.audio.source")
                        || entry.get("key").and_then(|v| v.as_str())
                            == Some("default.configured.audio.source"))
                        && let Some(value) = entry.get("value")
                        && let Some(name) = value.get("name").and_then(|v| v.as_str())
                    {
                        default_node_name = Some(name.to_string());
                        debug!(default_source = %name, "Found default audio source from metadata");
                        break;
                    }
                }
            }
            break;
        }
    }

    // Second pass: collect all audio sources with channel details
    for node in &nodes {
        if let Some(info) = node.get("info")
            && let Some(props) = info.get("props")
            && let Some(media_class) = props.get("media.class").and_then(|v| v.as_str())
            && media_class == "Audio/Source"
        {
            let name = props
                .get("node.nick")
                .or_else(|| props.get("node.description"))
                .or_else(|| props.get("node.name"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown Audio Device")
                .to_string();

            let serial = props
                .get("object.serial")
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .to_string();

            let node_name = props
                .get("node.name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let is_default = default_node_name
                .as_ref()
                .map(|default| default == &node_name)
                .unwrap_or(false);

            // Extract channel info from node params
            let (channels, sample_format, sample_rate) = extract_channel_info(info.get("params"));

            devices.push(AudioDevice {
                name,
                serial,
                node_name,
                is_default,
                channels,
                sample_format,
                sample_rate,
            });

            debug!(
                name = %devices.last().unwrap().name,
                serial = %devices.last().unwrap().serial,
                channels = devices.last().unwrap().channels.len(),
                format = %devices.last().unwrap().sample_format,
                rate = devices.last().unwrap().sample_rate,
                is_default = is_default,
                "Found audio input device"
            );
        }
    }

    // Sort: default first, then alphabetically
    devices.sort_by(|a, b| match (a.is_default, b.is_default) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    devices
}

/// Extract channel information from PipeWire node params
fn extract_channel_info(
    params: Option<&serde_json::Value>,
) -> (Vec<AudioChannelInfo>, String, u32) {
    let mut channels = Vec::new();
    let mut sample_format = String::new();
    let mut sample_rate = 0u32;

    let Some(params) = params else {
        return (channels, sample_format, sample_rate);
    };

    // Get format info from EnumFormat
    if let Some(enum_formats) = params.get("EnumFormat").and_then(|v| v.as_array()) {
        for fmt in enum_formats {
            if fmt.get("mediaType").and_then(|v| v.as_str()) == Some("audio") {
                if let Some(f) = fmt.get("format").and_then(|v| v.as_str()) {
                    sample_format = f.to_string();
                }
                if let Some(r) = fmt.get("rate").and_then(|v| v.as_u64()) {
                    sample_rate = r as u32;
                }
                // Get channel positions from EnumFormat
                if let Some(positions) = fmt.get("position").and_then(|v| v.as_array()) {
                    for pos in positions {
                        if let Some(pos_str) = pos.as_str() {
                            channels.push(AudioChannelInfo {
                                position: pos_str.to_string(),
                                volume: 1.0,
                                volume_db: 0.0,
                            });
                        }
                    }
                }
                break;
            }
        }
    }

    // Overlay per-channel volumes from Props
    if let Some(props_list) = params.get("Props").and_then(|v| v.as_array()) {
        for props in props_list {
            if let Some(volumes) = props.get("channelVolumes").and_then(|v| v.as_array()) {
                if let Some(channel_map) = props.get("channelMap").and_then(|v| v.as_array()) {
                    // Rebuild channels from channelMap + volumes (more authoritative)
                    if !channel_map.is_empty() {
                        channels.clear();
                        for (i, ch) in channel_map.iter().enumerate() {
                            let vol = volumes.get(i).and_then(|v| v.as_f64()).unwrap_or(1.0);
                            let position = ch.as_str().unwrap_or("?").to_string();
                            channels.push(AudioChannelInfo {
                                position,
                                volume: vol,
                                volume_db: linear_to_db(vol),
                            });
                        }
                    }
                } else {
                    // No channel map, just update volumes on existing channels
                    for (i, vol_val) in volumes.iter().enumerate() {
                        if let Some(ch) = channels.get_mut(i)
                            && let Some(vol) = vol_val.as_f64()
                        {
                            ch.volume = vol;
                            ch.volume_db = linear_to_db(vol);
                        }
                    }
                }
            }
        }
    }

    (channels, sample_format, sample_rate)
}
