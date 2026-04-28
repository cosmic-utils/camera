// SPDX-License-Identifier: GPL-3.0-only

//! Insights drawer view for displaying diagnostic information

use crate::app::state::{AppModel, ContextPage, Message};
use crate::fl;
use cosmic::Element;
use cosmic::app::context_drawer;
use cosmic::iced::{Alignment, Length};
use cosmic::widget;

use super::types::FallbackState;

/// Theme-aware destructive/error text style.
fn error_text_style(theme: &cosmic::Theme) -> cosmic::iced::widget::text::Style {
    cosmic::iced::widget::text::Style {
        color: Some(cosmic::iced::Color::from(
            theme.cosmic().destructive_color(),
        )),
    }
}

/// Create a status text widget, styled based on availability and active state.
fn v4l2_status_text(text: String, available: bool, is_active: bool) -> Element<'static, Message> {
    if is_active {
        widget::text::body(text)
            .class(cosmic::theme::style::Text::Accent)
            .into()
    } else if available {
        widget::text::body(text).into()
    } else {
        widget::text::body(text)
            .class(cosmic::theme::style::iced::Text::Custom(error_text_style))
            .into()
    }
}

/// Check if a libcamera pixel format name matches a V4L2 FourCC string.
///
/// libcamera may use different names (e.g., "MJPEG" vs V4L2's "MJPG").
fn format_matches_fourcc(libcamera_fmt: &str, v4l2_fourcc: &str) -> bool {
    if libcamera_fmt.eq_ignore_ascii_case(v4l2_fourcc) {
        return true;
    }
    // Common aliases between libcamera and V4L2
    matches!(
        (libcamera_fmt, v4l2_fourcc),
        ("MJPEG", "MJPG") | ("MJPG", "MJPEG")
    )
}

impl AppModel {
    /// Create the insights view for the context drawer
    ///
    /// Shows pipeline information, performance metrics, and format capabilities.
    pub fn insights_view(&self) -> context_drawer::ContextDrawer<'_, Message> {
        // Capture buttons row at the top
        let capture_buttons: Element<'_, Message> = widget::Row::new()
            .push(
                widget::button::standard(fl!("insights-capture"))
                    .on_press(Message::InsightsCaptureFrames),
            )
            .push(widget::space::horizontal().width(Length::Fixed(8.0)))
            .push(
                widget::button::standard(fl!("insights-capture-burst"))
                    .on_press(Message::InsightsCaptureBurst),
            )
            .padding(8)
            .into();

        let mut sections = vec![capture_buttons, self.build_pipeline_section().into()];

        // Show backend/multistream sections when libcamera backend is active
        if !self.insights.backend_type.is_empty() {
            sections.push(self.build_backend_section().into());
        }

        if self.insights.is_multistream {
            // Dual-stream: separate Preview and Capture sections
            sections.push(self.build_preview_stream_section().into());
            if self.insights.capture_stream.is_some() {
                sections.push(self.build_capture_stream_section().into());
            }
        } else {
            // Single-stream: combined section
            sections.push(self.build_combined_stream_section().into());
        }

        // Recording section (shown when recording is active)
        if self.insights.recording_diag.is_some() {
            sections.push(self.build_recording_section().into());
        }

        // Audio section
        sections.push(self.build_audio_section().into());

        // Per-frame metadata section (libcamera only)
        if self.insights.has_libcamera_metadata {
            sections.push(self.build_metadata_section().into());
        }

        // V4L2 device formats sections (one per pixel format)
        for fmt in &self.insights.v4l2_formats {
            sections.push(self.build_v4l2_format_section(fmt).into());
        }

        let content: Element<'_, Message> = widget::settings::view_column(sections).into();

        context_drawer::context_drawer(content, Message::ToggleContextPage(ContextPage::Insights))
            .title(fl!("insights-title"))
    }

    /// Build the Pipeline section
    fn build_pipeline_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-pipeline"));

        // Full GStreamer pipeline string with copy button
        let pipeline_text = self
            .insights
            .full_pipeline_string
            .as_deref()
            .unwrap_or("No pipeline active");

        let pipeline_content = widget::container(
            widget::text::body(pipeline_text)
                .font(cosmic::font::mono())
                .size(10),
        )
        .padding(8)
        .class(cosmic::style::Container::Card)
        .width(Length::Fill);

        // Copy button
        let copy_button =
            widget::button::icon(widget::icon::from_name("edit-copy-symbolic").symbolic(true))
                .extra_small()
                .on_press(Message::CopyPipelineString);

        let pipeline_label = fl!("insights-pipeline-full-libcamera");
        section = section.add(widget::settings::item::builder(pipeline_label).control(copy_button));

        section = section.add(widget::settings::item_row(vec![pipeline_content.into()]));

        // Decoder fallback chain
        if !self.insights.decoder_chain.is_empty() {
            section = section.add(
                widget::settings::item::builder(fl!("insights-decoder-chain"))
                    .control(widget::Space::new().width(0).height(0)),
            );

            for decoder in &self.insights.decoder_chain {
                let (icon_name, status_text) = match decoder.state {
                    FallbackState::Selected => ("emblem-ok-symbolic", fl!("insights-selected")),
                    FallbackState::Available => {
                        ("media-record-symbolic", fl!("insights-available"))
                    }
                    FallbackState::Unavailable => {
                        ("window-close-symbolic", fl!("insights-unavailable"))
                    }
                };

                let row = widget::Row::new()
                    .push(widget::icon::from_name(icon_name).symbolic(true).size(16))
                    .push(widget::space::horizontal().width(Length::Fixed(8.0)))
                    .push(
                        widget::Column::new()
                            .push(widget::text::body(decoder.name).font(cosmic::font::mono()))
                            .push(
                                widget::text::caption(format!(
                                    "{} - {}",
                                    decoder.description, status_text
                                ))
                                .size(11),
                            ),
                    )
                    .align_y(Alignment::Center)
                    .padding(4);

                section = section.add(widget::settings::item_row(vec![row.into()]));
            }
        }

        section
    }

    /// Add performance metrics to a section
    fn add_performance_items<'a>(
        &self,
        mut section: widget::settings::Section<'a, Message>,
    ) -> widget::settings::Section<'a, Message> {
        // Frame latency
        let latency_ms = self.insights.frame_latency_us as f64 / 1000.0;
        section = section.add(
            widget::settings::item::builder(fl!("insights-frame-latency"))
                .control(widget::text::body(format!("{:.2} ms", latency_ms))),
        );

        // Dropped frames
        section = section.add(
            widget::settings::item::builder(fl!("insights-dropped-frames")).control(
                widget::text::body(format!("{}", self.insights.dropped_frames)),
            ),
        );

        // Frame size
        let decoded_mb = self.insights.frame_size_decoded as f64 / (1024.0 * 1024.0);
        section = section.add(
            widget::settings::item::builder(fl!("insights-frame-size-decoded"))
                .control(widget::text::body(format!("{:.2} MB", decoded_mb))),
        );

        // CPU decode time (turbojpeg MJPEG→I420)
        if self.insights.cpu_decode_time_us > 0 {
            let cpu_decode_ms = self.insights.cpu_decode_time_us as f64 / 1000.0;
            section = section.add(
                widget::settings::item::builder(fl!("insights-cpu-decode-time"))
                    .control(widget::text::body(format!("{:.2} ms", cpu_decode_ms))),
            );
        }

        // Frame wrap time
        let copy_ms = self.insights.copy_time_us as f64 / 1000.0;
        let copy_text = if copy_ms < 0.01 {
            "< 0.01 ms (zero-copy)".to_string()
        } else {
            format!("{:.2} ms", copy_ms)
        };
        section = section.add(
            widget::settings::item::builder(fl!("insights-copy-time"))
                .control(widget::text::body(copy_text)),
        );

        // GPU upload time
        let gpu_upload_ms = self.insights.gpu_conversion_time_us as f64 / 1000.0;
        section = section.add(
            widget::settings::item::builder(fl!("insights-gpu-upload-time"))
                .control(widget::text::body(format!("{:.2} ms", gpu_upload_ms))),
        );

        // GPU upload bandwidth
        let bandwidth_text = if self.insights.copy_bandwidth_mbps > 0.0 {
            format!("{:.1} MB/s", self.insights.copy_bandwidth_mbps)
        } else {
            "N/A".to_string()
        };
        section = section.add(
            widget::settings::item::builder(fl!("insights-gpu-upload-bandwidth"))
                .control(widget::text::body(bandwidth_text)),
        );

        section
    }

    /// Add format chain items to a section
    ///
    /// When `skip_resolution` is true, the Resolution and Framerate rows are
    /// omitted because they are already shown by the stream info items above.
    fn add_format_items<'a>(
        &'a self,
        mut section: widget::settings::Section<'a, Message>,
        skip_resolution: bool,
    ) -> widget::settings::Section<'a, Message> {
        let chain = &self.insights.format_chain;

        section = section.add(
            widget::settings::item::builder(fl!("insights-format-source"))
                .control(widget::text::body(&chain.source)),
        );
        if !skip_resolution {
            section = section.add(
                widget::settings::item::builder(fl!("insights-format-resolution"))
                    .control(widget::text::body(&chain.resolution)),
            );
            section = section.add(
                widget::settings::item::builder(fl!("insights-format-framerate"))
                    .control(widget::text::body(&chain.framerate)),
            );
        }
        section = section.add(
            widget::settings::item::builder(fl!("insights-format-native"))
                .control(widget::text::body(&chain.native_format)),
        );
        if let Some(cpu_proc) = &self.insights.cpu_processing {
            section = section.add(
                widget::settings::item::builder(fl!("insights-cpu-processing"))
                    .control(widget::text::body(cpu_proc)),
            );
        }
        section = section.add(
            widget::settings::item::builder(fl!("insights-format-wgpu"))
                .control(widget::text::body(&chain.wgpu_processing)),
        );

        section
    }

    /// Add stream info items to a section
    fn add_stream_items<'a>(
        &'a self,
        mut section: widget::settings::Section<'a, Message>,
        stream: &'a super::types::StreamInfo,
    ) -> widget::settings::Section<'a, Message> {
        section = section.add(
            widget::settings::item::builder(fl!("insights-stream-role"))
                .control(widget::text::body(&stream.role)),
        );
        section = section.add(
            widget::settings::item::builder(fl!("insights-stream-resolution"))
                .control(widget::text::body(&stream.resolution)),
        );
        // Show configured framerate, or measured FPS from frame count
        let framerate_text = if self.insights.format_chain.framerate != "N/A"
            && !self.insights.format_chain.framerate.is_empty()
        {
            self.insights.format_chain.framerate.clone()
        } else if self.insights.measured_fps > 0.0 {
            format!("{:.1} fps (measured)", self.insights.measured_fps)
        } else {
            String::new()
        };
        if !framerate_text.is_empty() {
            section = section.add(
                widget::settings::item::builder(fl!("insights-format-framerate"))
                    .control(widget::text::body(framerate_text)),
            );
        }
        section = section.add(
            widget::settings::item::builder(fl!("insights-stream-pixel-format"))
                .control(widget::text::body(&stream.pixel_format)),
        );
        section = section.add(
            widget::settings::item::builder(fl!("insights-stream-frame-count"))
                .control(widget::text::body(format!("{}", stream.frame_count))),
        );
        section
    }

    /// Build the combined single-stream section (Preview + Capture)
    fn build_combined_stream_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-stream-combined"));

        // Stream info if available
        let has_stream = self.insights.preview_stream.is_some();
        if let Some(stream) = &self.insights.preview_stream {
            section = self.add_stream_items(section, stream);
        }

        // Format chain (skip resolution/framerate if stream info already shows them)
        section = self.add_format_items(section, has_stream);

        // Performance metrics
        section = self.add_performance_items(section);

        section
    }

    /// Build the Preview Stream section (dual-stream mode)
    fn build_preview_stream_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-stream-preview"));

        // Stream info
        let has_stream = self.insights.preview_stream.is_some();
        if let Some(stream) = &self.insights.preview_stream {
            section = self.add_stream_items(section, stream);
        }

        // Format chain (skip resolution/framerate if stream info already shows them)
        section = self.add_format_items(section, has_stream);

        // Performance metrics (apply to preview rendering)
        section = self.add_performance_items(section);

        section
    }

    /// Build the Capture Stream section (dual-stream mode)
    fn build_capture_stream_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-stream-capture"));

        if let Some(stream) = &self.insights.capture_stream {
            section = self.add_stream_items(section, stream);

            // Source
            if !stream.source.is_empty() {
                section = section.add(
                    widget::settings::item::builder(fl!("insights-format-source"))
                        .control(widget::text::body(&stream.source)),
                );
            }

            // GPU processing
            if !stream.gpu_processing.is_empty() {
                section = section.add(
                    widget::settings::item::builder(fl!("insights-format-wgpu"))
                        .control(widget::text::body(&stream.gpu_processing)),
                );
            }

            // Frame size
            if stream.frame_size_bytes > 0 {
                let mb = stream.frame_size_bytes as f64 / (1024.0 * 1024.0);
                section = section.add(
                    widget::settings::item::builder(fl!("insights-frame-size-decoded"))
                        .control(widget::text::body(format!("{:.2} MB", mb))),
                );
            }
        }

        section
    }

    /// Build the Recording section (active recording pipeline info + live stats)
    fn build_recording_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-recording"));

        let diag = self.insights.recording_diag.as_ref().unwrap();

        // Recording mode
        section = section.add(
            widget::settings::item::builder(fl!("insights-recording-mode"))
                .control(widget::text::body(&diag.mode)),
        );

        // Encoder
        section = section.add(
            widget::settings::item::builder(fl!("insights-recording-encoder"))
                .control(widget::text::body(&diag.encoder).font(cosmic::font::mono())),
        );

        // Resolution + Framerate on one line
        section = section.add(
            widget::settings::item::builder(fl!("insights-recording-resolution")).control(
                widget::text::body(format!("{} @ {} fps", diag.resolution, diag.framerate)),
            ),
        );

        // Live stats (if available)
        if let Some(stats) = &self.insights.recording_stats {
            // Capture → Channel
            section = section.add(
                widget::settings::item::builder(fl!("insights-recording-capture")).control(
                    widget::text::body(format!(
                        "{} sent, {} dropped",
                        stats.capture_sent, stats.capture_dropped
                    )),
                ),
            );

            // Channel backlog
            section = section.add(
                widget::settings::item::builder(fl!("insights-recording-channel")).control(
                    widget::text::body(format!("{} queued", stats.channel_backlog)),
                ),
            );

            // Pusher → Appsrc
            section = section.add(
                widget::settings::item::builder(fl!("insights-recording-pusher")).control(
                    widget::text::body(format!(
                        "{} pushed, {} skipped",
                        stats.pusher_pushed, stats.pusher_skipped
                    )),
                ),
            );

            // Effective FPS
            section = section.add(
                widget::settings::item::builder(fl!("insights-recording-fps")).control(
                    widget::text::body(format!("{:.1} fps", stats.effective_fps)),
                ),
            );

            // Processing delay
            if stats.last_processing_delay_us > 0 {
                let delay_ms = stats.last_processing_delay_us as f64 / 1000.0;
                section = section.add(
                    widget::settings::item::builder(fl!("insights-recording-delay"))
                        .control(widget::text::body(format!("{:.1} ms", delay_ms))),
                );
            }

            // NV12 conversion time (only shown for pusher NV12 path)
            if stats.last_convert_time_us > 0 {
                let convert_ms = stats.last_convert_time_us as f64 / 1000.0;
                section = section.add(
                    widget::settings::item::builder(fl!("insights-recording-convert"))
                        .control(widget::text::body(format!("{:.2} ms", convert_ms))),
                );
            }

            // Current PTS
            section = section.add(
                widget::settings::item::builder(fl!("insights-recording-pts")).control(
                    widget::text::body(format!("{:.1} s", stats.last_pts_ms as f64 / 1000.0)),
                ),
            );
        }

        // Full pipeline string
        let pipeline_content = widget::container(
            widget::text::body(&diag.pipeline_string)
                .font(cosmic::font::mono())
                .size(10),
        )
        .padding(8)
        .class(cosmic::style::Container::Card)
        .width(Length::Fill);

        section = section.add(
            widget::settings::item::builder(fl!("insights-recording-pipeline"))
                .control(widget::Space::new().width(0).height(0)),
        );
        section = section.add(widget::settings::item_row(vec![pipeline_content.into()]));

        section
    }

    /// Build the per-frame metadata section (libcamera only)
    ///
    /// Shows all metadata fields with "N/A" when a value is not reported by the ISP.
    fn build_metadata_section(&self) -> widget::settings::Section<'_, Message> {
        let na = fl!("insights-meta-na");
        let mut section = widget::settings::section().title(fl!("insights-metadata"));

        // Exposure
        let text = match self.insights.meta_exposure_us {
            Some(us) if us >= 1_000_000 => format!("{:.2} s", us as f64 / 1_000_000.0),
            Some(us) if us >= 1_000 => format!("{:.2} ms", us as f64 / 1_000.0),
            Some(us) => format!("{} \u{00b5}s", us),
            None => na.clone(),
        };
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-exposure"))
                .control(widget::text::body(text)),
        );

        // Analogue Gain
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-analogue-gain")).control(
                widget::text::body(
                    self.insights
                        .meta_analogue_gain
                        .map_or_else(|| na.clone(), |g| format!("{:.2}x", g)),
                ),
            ),
        );

        // Digital Gain
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-digital-gain")).control(
                widget::text::body(
                    self.insights
                        .meta_digital_gain
                        .map_or_else(|| na.clone(), |g| format!("{:.2}x", g)),
                ),
            ),
        );

        // Colour Temperature
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-colour-temp")).control(
                widget::text::body(
                    self.insights
                        .meta_colour_temperature
                        .map_or_else(|| na.clone(), |t| format!("{} K", t)),
                ),
            ),
        );

        // WB Gains
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-colour-gains")).control(
                widget::text::body(
                    self.insights
                        .meta_colour_gains
                        .map_or_else(|| na.clone(), |g| format!("{:.2}, {:.2}", g[0], g[1])),
                ),
            ),
        );

        // Black Level
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-black-level")).control(
                widget::text::body(
                    self.insights
                        .meta_black_level
                        .map_or_else(|| na.clone(), |bl| format!("{:.4}", bl)),
                ),
            ),
        );

        // Illuminance (Lux)
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-lux")).control(widget::text::body(
                self.insights
                    .meta_lux
                    .map_or_else(|| na.clone(), |l| format!("{:.0} lux", l)),
            )),
        );

        // Lens Position
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-lens-position")).control(
                widget::text::body(
                    self.insights
                        .meta_lens_position
                        .map_or_else(|| na.clone(), |p| format!("{:.2} dioptres", p)),
                ),
            ),
        );

        // Focus FoM
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-focus-fom")).control(
                widget::text::body(
                    self.insights
                        .meta_focus_fom
                        .map_or_else(|| na.clone(), |f| format!("{}", f)),
                ),
            ),
        );

        // Sequence
        section = section.add(
            widget::settings::item::builder(fl!("insights-meta-sequence")).control(
                widget::text::body(
                    self.insights
                        .meta_sequence
                        .map_or_else(|| na.clone(), |s| format!("{}", s)),
                ),
            ),
        );

        section
    }

    /// Build the Audio section showing audio device, pipeline, per-channel details, and live levels
    fn build_audio_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-audio"));

        // Recording enabled/disabled
        let status = if self.config.record_audio {
            fl!("insights-audio-enabled")
        } else {
            fl!("insights-audio-disabled")
        };
        section = section.add(
            widget::settings::item::builder(fl!("insights-audio-recording"))
                .control(widget::text::body(status)),
        );

        // Selected audio device details
        let dev = self
            .available_audio_devices
            .get(self.current_audio_device_index);

        if let Some(dev) = dev {
            let name = if dev.is_default {
                format!("{} {}", dev.name, fl!("insights-audio-default"))
            } else {
                dev.name.clone()
            };
            section = section.add(
                widget::settings::item::builder(fl!("insights-audio-device"))
                    .control(widget::text::body(name)),
            );

            // Audio device node name (monospace)
            let node_content = widget::container(
                widget::text::body(&dev.node_name)
                    .font(cosmic::font::mono())
                    .size(11),
            )
            .padding(4);
            section = section.add(
                widget::settings::item::builder(fl!("insights-audio-node")).control(node_content),
            );

            // Native format info
            if !dev.sample_format.is_empty() {
                let format_text = format!(
                    "{} / {} Hz / {}ch",
                    dev.sample_format,
                    dev.sample_rate,
                    dev.channels.len()
                );
                section = section.add(
                    widget::settings::item::builder(fl!("insights-audio-format"))
                        .control(widget::text::body(format_text)),
                );
            }
        }

        // Audio codec
        let codec = if gstreamer::ElementFactory::find("opusenc").is_some() {
            "Opus"
        } else if gstreamer::ElementFactory::find("avenc_aac").is_some()
            || gstreamer::ElementFactory::find("faac").is_some()
            || gstreamer::ElementFactory::find("voaacenc").is_some()
        {
            "AAC"
        } else {
            "None"
        };
        section = section.add(
            widget::settings::item::builder(fl!("insights-audio-codec"))
                .control(widget::text::body(codec)),
        );

        // Output channels (always mono)
        section = section.add(
            widget::settings::item::builder(fl!("insights-audio-channels"))
                .control(widget::text::body(fl!("insights-audio-mono"))),
        );

        // Pipeline chain description
        let pipeline_desc = if self.config.record_audio {
            format!(
                "pulsesrc \u{2192} queue \u{2192} audioconvert \u{2192} audioresample \u{2192} level \u{2192} capsfilter(mono) \u{2192} level \u{2192} {}",
                if codec == "Opus" {
                    "opusenc"
                } else if codec == "AAC" {
                    "avenc_aac"
                } else {
                    "none"
                }
            )
        } else {
            fl!("insights-audio-disabled")
        };
        let pipeline_content = widget::container(
            widget::text::body(pipeline_desc)
                .font(cosmic::font::mono())
                .size(10),
        )
        .padding(8)
        .class(cosmic::style::Container::Card)
        .width(Length::Fill);
        section = section.add(
            widget::settings::item::builder(fl!("insights-audio-pipeline"))
                .control(widget::Space::new().width(0).height(0)),
        );
        section = section.add(widget::settings::item_row(vec![pipeline_content.into()]));

        // Per-channel input details from audio device info
        if let Some(dev) = dev
            && !dev.channels.is_empty()
        {
            section = section.add(
                widget::settings::item::builder(fl!("insights-audio-inputs"))
                    .control(widget::Space::new().width(0).height(0)),
            );

            let levels = &self.insights.audio_levels;

            for (i, ch) in dev.channels.iter().enumerate() {
                // Get live level for this channel (if recording)
                let live_rms = levels.as_ref().and_then(|l| l.input_rms_db.get(i).copied());

                let vol_text = format!("{:.1} dB", ch.volume_db,);

                let mut row = widget::Row::new()
                    .push(
                        widget::text::body(&ch.position)
                            .font(cosmic::font::mono())
                            .size(12)
                            .width(Length::Fixed(48.0)),
                    )
                    .push(widget::space::horizontal().width(Length::Fixed(8.0)))
                    .push(
                        widget::text::caption(vol_text)
                            .size(11)
                            .width(Length::Fixed(64.0)),
                    );

                // Live dB level bar (when recording)
                if let Some(rms_db) = live_rms {
                    row = row
                        .push(widget::space::horizontal().width(Length::Fixed(8.0)))
                        .push(Self::build_level_bar(rms_db))
                        .push(widget::space::horizontal().width(Length::Fixed(4.0)))
                        .push(
                            widget::text::caption(format!("{:.0} dB", rms_db))
                                .size(10)
                                .font(cosmic::font::mono()),
                        );
                }

                row = row.align_y(Alignment::Center).padding(2);
                section = section.add(widget::settings::item_row(vec![row.into()]));
            }
        }

        // Mono output level (after mix)
        if let Some(levels) = &self.insights.audio_levels {
            let rms_text = format!("{:.1} dB", levels.output_rms_db);

            let output_row = widget::Row::new()
                .push(
                    widget::text::body(fl!("insights-audio-mono"))
                        .font(cosmic::font::mono())
                        .size(12)
                        .width(Length::Fixed(48.0)),
                )
                .push(widget::space::horizontal().width(Length::Fixed(8.0)))
                .push(Self::build_level_bar(levels.output_rms_db))
                .push(widget::space::horizontal().width(Length::Fixed(4.0)))
                .push(
                    widget::text::caption(rms_text)
                        .size(10)
                        .font(cosmic::font::mono()),
                )
                .align_y(Alignment::Center)
                .padding(2);

            section = section.add(
                widget::settings::item::builder(fl!("insights-audio-output-level"))
                    .control(widget::Space::new().width(0).height(0)),
            );
            section = section.add(widget::settings::item_row(vec![output_row.into()]));
        } else if self.recording.is_recording() && self.config.record_audio {
            // Recording but no levels yet
            section = section.add(
                widget::settings::item::builder(fl!("insights-audio-output-level"))
                    .control(widget::text::body("...")),
            );
        }

        section
    }

    /// Build a horizontal level bar for a dB value.
    ///
    /// Maps -60 dB..0 dB to 0..100% width.  Uses green/yellow/red colouring.
    fn build_level_bar(db: f64) -> Element<'static, Message> {
        // Clamp and normalize: -60 dB → 0%, 0 dB → 100%
        let fraction = ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
        let bar_width = (fraction * 80.0).max(1.0);

        // Color: green < -12 dB, yellow -12...-3, red > -3
        let color = if db < -12.0 {
            cosmic::iced::Color::from_rgb(0.2, 0.8, 0.3) // green
        } else if db < -3.0 {
            cosmic::iced::Color::from_rgb(0.9, 0.8, 0.1) // yellow
        } else {
            cosmic::iced::Color::from_rgb(0.9, 0.2, 0.2) // red
        };

        widget::container(widget::Space::new().width(bar_width).height(8))
            .class(cosmic::style::Container::custom(move |_theme| {
                cosmic::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(color)),
                    border: cosmic::iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            }))
            .width(Length::Fixed(bar_width))
            .into()
    }

    /// Build a V4L2 format section for a single pixel format (e.g., MJPG, YUYV, H264).
    /// Each resolution+framerate combination gets its own row.
    fn build_v4l2_format_section(
        &self,
        fmt: &crate::backends::camera::v4l2_utils::V4l2FormatInfo,
    ) -> widget::settings::Section<'_, Message> {
        let title = format!("{} ({})", fmt.fourcc.trim(), fmt.description);
        let mut section = widget::settings::section().title(title);
        let fourcc_trimmed = fmt.fourcc.trim();

        // Get the actual running stream info for highlighting.
        // Use preview_stream (the actual negotiated format) rather than active_format
        // (user-selected format) since libcamera may negotiate differently
        // (e.g., user selects YUYV but libcamera uses MJPEG for better framerate).
        let stream = self.insights.preview_stream.as_ref();

        let mut sorted_sizes = fmt.sizes.clone();
        sorted_sizes.sort_by_key(|b| std::cmp::Reverse(b.width * b.height));

        for size in &sorted_sizes {
            let in_libcamera = self.insights.libcamera_formats.iter().any(|lf| {
                lf.width == size.width
                    && lf.height == size.height
                    && format_matches_fourcc(&lf.pixel_format, fourcc_trimmed)
            });

            // Check if this resolution+format matches the actual running stream.
            // The stream pixel_format may be e.g. "I422 (MJPEG)" or "NV12",
            // so check if it contains the V4L2 fourcc or its alias.
            let is_active_resolution = stream.is_some_and(|s| {
                let res_parts: Vec<&str> = s.resolution.split('x').collect();
                let matches_res = res_parts.len() == 2
                    && res_parts[0].parse::<u32>().ok() == Some(size.width)
                    && res_parts[1].parse::<u32>().ok() == Some(size.height);
                let stream_fmt = &s.pixel_format;
                let matches_fmt = stream_fmt.eq_ignore_ascii_case(fourcc_trimmed)
                    || stream_fmt.contains(fourcc_trimmed)
                    || (fourcc_trimmed == "MJPG"
                        && (stream_fmt.contains("MJPEG") || stream_fmt.contains("MJPG")))
                    || (fourcc_trimmed == "YUYV" && stream_fmt.contains("YUYV"));
                matches_res && matches_fmt
            });

            let status_text = if is_active_resolution {
                format!("\u{2713} {}", fl!("insights-v4l2-active-in-libcamera"))
            } else if in_libcamera {
                format!("\u{2713} {}", fl!("insights-v4l2-in-libcamera"))
            } else {
                format!("\u{2717} {}", fl!("insights-v4l2-not-in-libcamera"))
            };

            let resolution_label = format!("{}x{}", size.width, size.height);

            if size.framerates.is_empty() {
                let status_widget =
                    v4l2_status_text(status_text, in_libcamera, is_active_resolution);
                section = section
                    .add(widget::settings::item::builder(resolution_label).control(status_widget));
            } else {
                for &(num, denom) in &size.framerates {
                    let fps = if num > 0 {
                        let fps = denom as f64 / num as f64;
                        if fps == fps.round() {
                            format!("{} fps", fps as u32)
                        } else {
                            format!("{:.2} fps", fps)
                        }
                    } else {
                        "? fps".to_string()
                    };

                    let row_status =
                        v4l2_status_text(status_text.clone(), in_libcamera, is_active_resolution);
                    section = section.add(
                        widget::settings::item::builder(format!("{} @ {}", resolution_label, fps))
                            .control(row_status),
                    );
                }
            }
        }

        section
    }

    /// Build the Backend section (libcamera-specific info)
    fn build_backend_section(&self) -> widget::settings::Section<'_, Message> {
        let mut section = widget::settings::section().title(fl!("insights-backend"));

        // Backend type
        section = section.add(
            widget::settings::item::builder(fl!("insights-backend-type"))
                .control(widget::text::body(self.insights.backend_type)),
        );

        // Pipeline handler
        if let Some(handler) = &self.insights.pipeline_handler {
            section = section.add(
                widget::settings::item::builder(fl!("insights-pipeline-handler"))
                    .control(widget::text::body(handler)),
            );
        }

        // Sensor model
        if let Some(sensor) = &self.insights.sensor_model {
            section = section.add(
                widget::settings::item::builder(fl!("insights-sensor-model"))
                    .control(widget::text::body(sensor)),
            );
        }

        // libcamera version
        if let Some(version) = &self.insights.libcamera_version {
            section = section.add(
                widget::settings::item::builder(fl!("insights-libcamera-version"))
                    .control(widget::text::body(version)),
            );
        }

        // MJPEG decoder (shown when native libcamera decodes MJPEG)
        if let Some(decoder) = &self.insights.mjpeg_decoder {
            section = section.add(
                widget::settings::item::builder(fl!("insights-mjpeg-decoder"))
                    .control(widget::text::body(decoder)),
            );
        }

        // Stream mode: label is Single/Dual-stream, control shows source assignment
        let (mode_label, source_text) = if self.insights.is_multistream {
            (
                fl!("insights-multistream-dual"),
                fl!("insights-multistream-source-separate"),
            )
        } else {
            (
                fl!("insights-multistream-single"),
                fl!("insights-multistream-source-shared"),
            )
        };
        section = section.add(
            widget::settings::item::builder(mode_label).control(widget::text::body(source_text)),
        );

        section
    }
}
