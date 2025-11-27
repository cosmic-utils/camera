// SPDX-License-Identifier: MPL-2.0

//! Audio device enumeration for PipeWire

use std::process::Command;
use tracing::{debug, warn};

/// Represents an audio input device
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub serial: String,
    pub node_name: String,
    pub is_default: bool,
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
        if node.get("type").and_then(|v| v.as_str()) == Some("PipeWire:Interface:Metadata") {
            if let Some(props) = node.get("props") {
                if props.get("metadata.name").and_then(|v| v.as_str()) == Some("default") {
                    if let Some(metadata) = node.get("metadata").and_then(|v| v.as_array()) {
                        for entry in metadata {
                            if entry.get("key").and_then(|v| v.as_str())
                                == Some("default.audio.source")
                                || entry.get("key").and_then(|v| v.as_str())
                                    == Some("default.configured.audio.source")
                            {
                                if let Some(value) = entry.get("value") {
                                    if let Some(name) = value.get("name").and_then(|v| v.as_str()) {
                                        default_node_name = Some(name.to_string());
                                        debug!(default_source = %name, "Found default audio source from metadata");
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    // Second pass: collect all audio sources
    for node in &nodes {
        if let Some(info) = node.get("info") {
            if let Some(props) = info.get("props") {
                if let Some(media_class) = props.get("media.class").and_then(|v| v.as_str()) {
                    if media_class == "Audio/Source" {
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

                        devices.push(AudioDevice {
                            name,
                            serial,
                            node_name,
                            is_default,
                        });

                        debug!(
                            name = %devices.last().unwrap().name,
                            serial = %devices.last().unwrap().serial,
                            is_default = is_default,
                            "Found audio input device"
                        );
                    }
                }
            }
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
