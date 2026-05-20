// SPDX-License-Identifier: GPL-3.0-only

//! Audio device enumeration and PulseAudio source-volume management.
//!
//! Uses the pure-Rust `pulseaudio` crate to speak the PulseAudio wire protocol
//! directly over `$XDG_RUNTIME_DIR/pulse/native`. Falls back to `pw-dump` only
//! if PulseAudio can't be reached at all (PipeWire-without-PA setups without
//! `pipewire-pulse`). No fork+exec of `pactl` — the previous version did, but
//! `pactl` isn't shipped inside the `org.freedesktop.Platform` flatpak runtime
//! and would silently no-op every audio operation there.

use std::ffi::CString;
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

use pulseaudio::protocol::{
    self, AuthParams, AuthReply, ChannelVolume, Command as PaCommand, CommandReply, GetSourceInfo,
    MAX_VERSION, Prop, Props, ServerInfo, SetClientNameReply, SetDeviceVolumeParams, SourceInfo,
    SourceInfoList, TagStructRead, Volume,
};

/// Minimal blocking PulseAudio protocol client. Owns a single socket; one
/// instance per logical operation is fine — connection setup is ~1ms over a
/// Unix socket and we don't do these in hot paths (recording start/stop,
/// enumeration on hotplug).
struct PulseClient {
    sock: std::io::BufReader<UnixStream>,
    protocol_version: u16,
    next_tag: u32,
}

impl PulseClient {
    /// Connect to PulseAudio and complete the auth + SetClientName handshake.
    /// Returns `None` if no PA socket is reachable (no PA, no PipeWire-Pulse).
    fn connect() -> Option<Self> {
        let path = pulseaudio::socket_path_from_env()?;
        let stream = UnixStream::connect(&path).ok()?;
        // Bound blocking reads so a misbehaving server can't wedge enumeration
        // or the volume-guard restore. Total worst-case latency per
        // `PulseClient` instance is ~`PA_READ_TIMEOUT * N reads` where N is up
        // to ~4 (Auth + SetClientName + one query + one ack), so the chosen
        // 500ms keeps `VideoRecorder::Drop` under ~2s even on a wedged server.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let mut sock = std::io::BufReader::new(stream);

        let cookie = pulseaudio::cookie_path_from_env()
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default();

        let auth = AuthParams {
            version: MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        };

        protocol::write_command_message(sock.get_mut(), 0, &PaCommand::Auth(auth), MAX_VERSION)
            .ok()?;
        let (_, auth_reply) =
            protocol::read_reply_message::<AuthReply>(&mut sock, MAX_VERSION).ok()?;
        let protocol_version = std::cmp::min(MAX_VERSION, auth_reply.version);

        let mut props = Props::new();
        if let Ok(name) = CString::new(env!("CARGO_PKG_NAME")) {
            props.set(Prop::ApplicationName, name);
        }
        protocol::write_command_message(
            sock.get_mut(),
            1,
            &PaCommand::SetClientName(props),
            protocol_version,
        )
        .ok()?;
        let _ =
            protocol::read_reply_message::<SetClientNameReply>(&mut sock, protocol_version).ok()?;

        Some(Self {
            sock,
            protocol_version,
            next_tag: 2,
        })
    }

    fn next_tag(&mut self) -> u32 {
        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1);
        tag
    }

    /// Bound on how many unsolicited / stale messages we discard while waiting
    /// for the reply to a specific request tag. Even an actively-subscribed
    /// client shouldn't see this many events between request and reply.
    const MAX_DRAIN_BEFORE_REPLY: usize = 16;

    /// Send a request and read its reply. Discards any unsolicited messages or
    /// stale replies whose `seq` doesn't match the tag we sent — PipeWire-Pulse
    /// can push messages even on un-subscribed clients, which would otherwise
    /// desync the request/reply pairing and silently drop the reply.
    fn send<R: CommandReply + TagStructRead>(&mut self, cmd: &PaCommand) -> Option<R> {
        let tag = self.next_tag();
        protocol::write_command_message(self.sock.get_mut(), tag, cmd, self.protocol_version)
            .ok()?;
        for _ in 0..Self::MAX_DRAIN_BEFORE_REPLY {
            match protocol::read_reply_message::<R>(&mut self.sock, self.protocol_version) {
                Ok((seq, reply)) if seq == tag => return Some(reply),
                Ok((seq, _)) => {
                    debug!(expected = tag, got = seq, "Discarding stale PA reply");
                }
                // `UnexpectedCommand` fires when an async event (e.g. a
                // SubscribeEvent pushed by PipeWire-Pulse) arrives in the
                // middle of our request/reply cycle. The bytes have already
                // been consumed; just try again.
                Err(_) => {
                    debug!("Discarding unsolicited PA message while waiting for reply");
                }
            }
        }
        warn!(tag, "Gave up draining PA messages waiting for reply");
        None
    }

    fn send_ack(&mut self, cmd: &PaCommand) -> bool {
        let tag = self.next_tag();
        if protocol::write_command_message(self.sock.get_mut(), tag, cmd, self.protocol_version)
            .is_err()
        {
            return false;
        }
        for _ in 0..Self::MAX_DRAIN_BEFORE_REPLY {
            match protocol::read_ack_message(&mut self.sock) {
                Ok(seq) if seq == tag => return true,
                Ok(seq) => {
                    debug!(expected = tag, got = seq, "Discarding stale PA ack");
                }
                Err(_) => {
                    debug!("Discarding unsolicited PA message while waiting for ack");
                }
            }
        }
        warn!(tag, "Gave up draining PA messages waiting for ack");
        false
    }

    fn server_info(&mut self) -> Option<ServerInfo> {
        self.send::<ServerInfo>(&PaCommand::GetServerInfo)
    }

    fn list_sources(&mut self) -> Option<Vec<SourceInfo>> {
        self.send::<SourceInfoList>(&PaCommand::GetSourceInfoList)
    }

    fn get_source_info(&mut self, name: &str) -> Option<SourceInfo> {
        let req = GetSourceInfo {
            index: None,
            name: Some(CString::new(name).ok()?),
        };
        self.send::<SourceInfo>(&PaCommand::GetSourceInfo(req))
    }

    fn set_source_volume(&mut self, name: &str, volume: ChannelVolume) -> bool {
        let Ok(name_c) = CString::new(name) else {
            return false;
        };
        let params = SetDeviceVolumeParams {
            device_index: None,
            device_name: Some(name_c),
            volume,
        };
        self.send_ack(&PaCommand::SetSourceVolume(params))
    }
}

/// RAII guard that boosts a PulseAudio source's software volume to 100% and
/// restores the previous value on `Drop`.
///
/// Some platforms (notably Alpine + alsa-ucm on the Pixel 3a) ship the
/// built-in mic's PA volume clamped to a low default (~32% / -30 dB). Even
/// with the compressor + makeup-gain in the recording pipeline, that
/// pre-attenuation gives us a poor SNR — software gain can't recover what
/// PA threw away. By raising the source volume to 100% for the duration of
/// the recording we keep the signal at full strength, and restore the user's
/// original setting on stop so we don't permanently mutate their PA state.
///
/// Best-effort: if the PA socket isn't reachable or the source doesn't
/// exist, the guard is a no-op (logged as debug) — recording still proceeds.
#[derive(Debug)]
pub struct PulseSourceVolumeGuard {
    device: String,
    /// Previous per-channel volumes, kept verbatim so `Drop` can restore
    /// exactly what was there (including any per-channel imbalance). `None`
    /// means the prior volume could not be queried; `Drop` is a no-op.
    original_volume: Option<ChannelVolume>,
}

impl PulseSourceVolumeGuard {
    /// Query the source's current volume, set it to 100% on every channel,
    /// and return a guard that restores the originals on drop. `None` if
    /// `device` is empty (caller picked the PA default source — we don't
    /// second-guess the default's volume) or if PA can't be reached.
    pub fn boost_to_full(device: &str) -> Option<Self> {
        if device.is_empty() {
            return None;
        }
        let mut client = PulseClient::connect()?;
        let info = client.get_source_info(device)?;
        let original = info.cvolume;
        let channels = original.channels().len() as u8;
        if channels == 0 {
            debug!(device, "PA source has no channels; skipping boost");
            return None;
        }

        info!(
            device,
            previous_db = original
                .channels()
                .first()
                .map(|v| v.to_db())
                .unwrap_or(f32::NEG_INFINITY),
            channels,
            "Boosting PA source volume to 100% for recording"
        );

        let boosted = ChannelVolume::norm(channels);
        if !client.set_source_volume(device, boosted) {
            warn!(
                device,
                "Failed to boost PA source volume — skipping restore"
            );
            return None;
        }

        Some(Self {
            device: device.to_string(),
            original_volume: Some(original),
        })
    }
}

impl Drop for PulseSourceVolumeGuard {
    fn drop(&mut self) {
        let Some(prev) = self.original_volume.take() else {
            return;
        };
        let Some(mut client) = PulseClient::connect() else {
            warn!(device = %self.device, "PA unreachable on drop — cannot restore source volume");
            return;
        };
        info!(
            device = %self.device,
            restore_db = prev
                .channels()
                .first()
                .map(|v| v.to_db())
                .unwrap_or(f32::NEG_INFINITY),
            "Restoring PA source volume"
        );
        let _ = client.set_source_volume(&self.device, prev);
    }
}

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

/// Enumerate available audio input devices.
///
/// Source-of-truth order:
/// 1. **PulseAudio native protocol** (`pulseaudio` crate over the
///    `$XDG_RUNTIME_DIR/pulse/native` socket). Works on PA-primary distros,
///    on PipeWire systems with `pipewire-pulse`, and inside flatpak.
/// 2. **`pw-dump`** as a fallback for PipeWire-without-PA setups where
///    `pipewire-tools` is installed but no PA-compat socket exists.
///
/// Both backends return PA-style node names (e.g. `alsa_input.usb-…`) so the
/// downstream `pulsesrc` GStreamer element can open them directly.
pub fn enumerate_audio_devices() -> Vec<AudioDevice> {
    if let Some(devices) = enumerate_via_pa_protocol()
        && !devices.is_empty()
    {
        return devices;
    }
    debug!("PulseAudio enumeration empty/unavailable, falling back to pw-dump");
    enumerate_via_pw_dump()
}

/// Enumerate input sources via PipeWire's `pw-dump`.
fn enumerate_via_pw_dump() -> Vec<AudioDevice> {
    let output = match Command::new("pw-dump").output() {
        Ok(output) => output,
        Err(e) => {
            debug!("pw-dump not available: {}", e);
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

/// Enumerate input sources by talking the PulseAudio protocol directly.
/// Returns `None` if PA can't be reached at all (so the caller falls back
/// to `pw-dump`). Returns an empty `Vec` if PA is up but has no real sources.
fn enumerate_via_pa_protocol() -> Option<Vec<AudioDevice>> {
    let mut client = PulseClient::connect()?;
    let server = client.server_info();
    let sources = client.list_sources()?;

    let default_source = server
        .and_then(|s| s.default_source_name)
        .and_then(|c| c.into_string().ok());

    let mut devices = Vec::new();
    for src in sources {
        // Skip sink monitors — they're not real inputs.
        if src.monitor_of_sink_name.is_some() {
            continue;
        }

        let node_name = src.name.to_string_lossy().into_owned();
        let name = src
            .description
            .as_ref()
            .map(|c| c.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| node_name.clone());
        let is_default = default_source.as_deref() == Some(node_name.as_str());

        let sample_format = format!("{:?}", src.sample_spec.format).to_lowercase();
        let sample_rate = src.sample_spec.sample_rate;

        let channels = channel_info_from_pa(&src);

        debug!(
            name = %name,
            node = %node_name,
            default = is_default,
            channels = channels.len(),
            format = %sample_format,
            rate = sample_rate,
            "Found PA audio source"
        );

        devices.push(AudioDevice {
            name,
            serial: src.index.to_string(),
            node_name,
            is_default,
            channels,
            sample_format,
            sample_rate,
        });
    }

    devices.sort_by(|a, b| match (a.is_default, b.is_default) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    Some(devices)
}

/// Map a PA `SourceInfo` to our `AudioChannelInfo` list using the channel
/// map's position names and the matching per-channel volumes.
fn channel_info_from_pa(src: &SourceInfo) -> Vec<AudioChannelInfo> {
    let positions: Vec<String> = src
        .channel_map
        .into_iter()
        .map(|p| format!("{:?}", p))
        .collect();

    let mut channels = Vec::with_capacity(positions.len());
    for (i, position) in positions.iter().enumerate() {
        let vol = src
            .cvolume
            .channels()
            .get(i)
            .copied()
            .unwrap_or(Volume::NORM);
        let linear = vol.to_linear() as f64;
        let db = vol.to_db() as f64;
        channels.push(AudioChannelInfo {
            position: position.clone(),
            volume: linear,
            volume_db: db,
        });
    }
    channels
}
