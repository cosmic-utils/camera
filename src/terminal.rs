// SPDX-License-Identifier: GPL-3.0-only

//! Terminal-based camera viewer
//!
//! Renders camera feed to the terminal using Unicode half-block characters
//! for improved vertical resolution.

use crate::backends::camera::pipewire::{
    PipeWirePipeline, enumerate_pipewire_cameras, get_pipewire_formats,
};
use crate::backends::camera::types::{CameraDevice, CameraFormat, CameraFrame, PixelFormat};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::channel::mpsc;
use ratatui::{
    Terminal, backend::CrosstermBackend, buffer::Buffer, layout::Rect, style::Color,
    widgets::Widget,
};
use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info};

/// Run the terminal camera viewer
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize GStreamer
    gstreamer::init()?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the app
    let result = run_app(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

struct CameraPipeline {
    _pipeline: PipeWirePipeline,
    receiver: mpsc::Receiver<CameraFrame>,
}

impl CameraPipeline {
    fn new(
        device: &CameraDevice,
        format: &CameraFormat,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (sender, receiver) = mpsc::channel(10);
        let pipeline = PipeWirePipeline::new(device, format, sender)?;
        Ok(Self {
            _pipeline: pipeline,
            receiver,
        })
    }

    fn try_get_frame(&mut self) -> Option<CameraFrame> {
        // Non-blocking receive
        self.receiver.try_recv().ok()
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Enumerate cameras
    let cameras = enumerate_pipewire_cameras().unwrap_or_default();
    if cameras.is_empty() {
        return Err("No cameras found".into());
    }

    info!(count = cameras.len(), "Found cameras");

    let multi_camera = cameras.len() > 1;
    let mut current_camera_index = 0;
    let mut pipeline = initialize_camera(&cameras[current_camera_index])?;

    let mut frame_widget = FrameWidget::new();
    let mut show_help = false;
    let mut status_message = build_status_message(multi_camera);

    loop {
        // Poll for frames (non-blocking) - drain all available frames to get latest
        while let Some(frame) = pipeline.try_get_frame() {
            frame_widget.update_frame(frame);
        }

        // Draw
        terminal.draw(|f| {
            let area = f.area();

            // Reserve bottom line for status
            let camera_area = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: area.height.saturating_sub(1),
            };

            f.render_widget(&frame_widget, camera_area);

            // Render status bar
            let status_area = Rect {
                x: area.x,
                y: area.height.saturating_sub(1),
                width: area.width,
                height: 1,
            };

            let status = StatusBar {
                message: &status_message,
            };
            f.render_widget(status, status_area);
        })?;

        // Handle input with timeout for frame updates
        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            // Ctrl+C to quit
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }

            // 'p' to take a picture
            if key.code == KeyCode::Char('p') {
                show_help = false;
                if let Some(frame) = &frame_widget.frame {
                    match save_photo(frame) {
                        Ok(path) => {
                            status_message = format!("Saved: {}", path.display());
                        }
                        Err(e) => {
                            error!("Failed to save photo: {}", e);
                            status_message = format!("Error: {}", e);
                        }
                    }
                }
            }

            // 's' to switch camera
            if key.code == KeyCode::Char('s') && multi_camera {
                show_help = false;
                current_camera_index = (current_camera_index + 1) % cameras.len();

                // Drop old pipeline first
                drop(pipeline);

                match initialize_camera(&cameras[current_camera_index]) {
                    Ok(new_pipeline) => {
                        pipeline = new_pipeline;
                        status_message = build_status_message(multi_camera);
                        frame_widget = FrameWidget::new(); // Clear old frame
                    }
                    Err(e) => {
                        error!("Failed to switch camera: {}", e);
                        status_message = format!("Error: {}", e);
                        // Try to go back to previous camera
                        current_camera_index = if current_camera_index == 0 {
                            cameras.len() - 1
                        } else {
                            current_camera_index - 1
                        };
                        pipeline = initialize_camera(&cameras[current_camera_index])?;
                    }
                }
            }

            // 'h' to toggle help
            if key.code == KeyCode::Char('h') {
                show_help = !show_help;
                status_message = if show_help {
                    build_help_message(multi_camera)
                } else {
                    build_status_message(multi_camera)
                };
            }

            // 'q' also quits
            if key.code == KeyCode::Char('q') {
                break;
            }
        }
    }

    Ok(())
}

fn initialize_camera(device: &CameraDevice) -> Result<CameraPipeline, Box<dyn std::error::Error>> {
    info!(device = %device.name, "Initializing camera");

    let formats = get_pipewire_formats(&device.path, device.metadata_path.as_deref());
    if formats.is_empty() {
        return Err(format!("No formats available for camera: {}", device.name).into());
    }

    // Find a good format - prefer lower resolution for terminal (faster processing)
    let format = select_terminal_format(&formats);

    info!(format = %format, "Selected format");
    CameraPipeline::new(device, &format)
}

fn build_status_message(multi_camera: bool) -> String {
    let mut msg = "'p' picture".to_string();
    if multi_camera {
        msg.push_str(" | 's' switch camera");
    }
    msg.push_str(" | 'h' help | 'q' quit");
    msg
}

fn build_help_message(multi_camera: bool) -> String {
    let mut msg = String::from("p: Take picture | ");
    if multi_camera {
        msg.push_str("s: Switch camera | ");
    }
    msg.push_str("h: Toggle help | q/Ctrl+C: Quit");
    msg
}

/// Save the current frame as a JPEG photo
fn save_photo(frame: &CameraFrame) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let width = frame.width;
    let height = frame.height;

    // Convert frame to RGB bytes
    let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let (r, g, b) = sample_pixel_rgb(frame, x, y);
            rgb_data.push(r);
            rgb_data.push(g);
            rgb_data.push(b);
        }
    }

    let img: image::RgbImage =
        image::ImageBuffer::from_raw(width, height, rgb_data).ok_or("Failed to create image")?;

    let photo_dir = crate::app::get_photo_directory("Camera");
    std::fs::create_dir_all(&photo_dir)?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let filename = format!("IMG_{}.jpg", timestamp);
    let filepath = photo_dir.join(&filename);

    img.save(&filepath)?;
    info!(path = %filepath.display(), "Photo saved");

    Ok(filepath)
}

fn select_terminal_format(formats: &[CameraFormat]) -> CameraFormat {
    // For terminal mode, prefer 640x480 or similar - high resolution isn't useful
    // and lower resolution means faster frame capture
    let target_pixels = 640 * 480;

    formats
        .iter()
        .min_by_key(|f| {
            let pixels = f.width * f.height;
            let diff = (pixels as i64 - target_pixels as i64).abs();
            // Prefer formats with framerate
            let fps_penalty = if f.framerate.is_some() { 0 } else { 1_000_000 };
            diff + fps_penalty
        })
        .cloned()
        .unwrap_or_else(|| formats[0].clone())
}

/// Widget that renders a camera frame using half-block characters
struct FrameWidget {
    frame: Option<CameraFrame>,
}

impl FrameWidget {
    fn new() -> Self {
        Self { frame: None }
    }

    fn update_frame(&mut self, frame: CameraFrame) {
        self.frame = Some(frame);
    }
}

impl Widget for &FrameWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Some(frame) = &self.frame else {
            // No frame yet - show placeholder
            let msg = "Waiting for camera...";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            if y < area.y + area.height && x < area.x + area.width {
                buf.set_string(x, y, msg, ratatui::style::Style::default());
            }
            return;
        };

        // Calculate display dimensions maintaining aspect ratio
        // Each terminal cell displays 2 vertical pixels using half-block characters
        let frame_aspect = frame.width as f64 / frame.height as f64;
        let term_width = area.width as f64;
        let term_height = (area.height * 2) as f64; // *2 because half-blocks

        let (display_width, display_height) = if term_width / term_height > frame_aspect {
            // Terminal is wider - fit to height
            let h = term_height;
            let w = h * frame_aspect;
            (w as u16, (h / 2.0) as u16)
        } else {
            // Terminal is taller - fit to width
            let w = term_width;
            let h = w / frame_aspect;
            (w as u16, (h / 2.0) as u16)
        };

        // Center the image
        let x_offset = area.x + (area.width.saturating_sub(display_width)) / 2;
        let y_offset = area.y + (area.height.saturating_sub(display_height)) / 2;

        // Scale factors
        let x_scale = frame.width as f64 / display_width as f64;
        let y_scale = frame.height as f64 / (display_height * 2) as f64;

        // Render using half-block characters
        // Each terminal cell represents 2 vertical pixels:
        // - Upper half (▀) colored with fg
        // - Lower half colored with bg
        for ty in 0..display_height {
            for tx in 0..display_width {
                let term_x = x_offset + tx;
                let term_y = y_offset + ty;

                if term_x >= area.x + area.width || term_y >= area.y + area.height {
                    continue;
                }

                // Sample upper pixel
                let src_x = (tx as f64 * x_scale) as u32;
                let src_y_top = (ty as f64 * 2.0 * y_scale) as u32;
                let src_y_bottom = ((ty as f64 * 2.0 + 1.0) * y_scale) as u32;

                let top_color = sample_pixel(frame, src_x, src_y_top);
                let bottom_color = sample_pixel(frame, src_x, src_y_bottom);

                let cell = buf.cell_mut((term_x, term_y)).unwrap();
                cell.set_char('▀');
                cell.set_fg(top_color);
                cell.set_bg(bottom_color);
            }
        }
    }
}

fn sample_pixel(frame: &CameraFrame, x: u32, y: u32) -> Color {
    let (r, g, b) = sample_pixel_rgb(frame, x, y);
    Color::Rgb(r, g, b)
}

fn sample_pixel_rgb(frame: &CameraFrame, x: u32, y: u32) -> (u8, u8, u8) {
    let x = x.min(frame.width - 1);
    let y = y.min(frame.height - 1);
    let data = frame.data_slice();

    match frame.format {
        PixelFormat::RGBA => {
            let idx = (y * frame.stride + x * 4) as usize;
            if idx + 2 < data.len() {
                (data[idx], data[idx + 1], data[idx + 2])
            } else {
                (0, 0, 0)
            }
        }
        PixelFormat::RGB24 => {
            let idx = (y * frame.stride + x * 3) as usize;
            if idx + 2 < data.len() {
                (data[idx], data[idx + 1], data[idx + 2])
            } else {
                (0, 0, 0)
            }
        }
        PixelFormat::Gray8 => {
            let idx = (y * frame.stride + x) as usize;
            if idx < data.len() {
                let v = data[idx];
                (v, v, v)
            } else {
                (0, 0, 0)
            }
        }
        PixelFormat::NV12 | PixelFormat::NV21 => {
            let y_idx = (y * frame.stride + x) as usize;
            if y_idx >= data.len() {
                return (0, 0, 0);
            }
            let luma = data[y_idx];

            // UV plane is after the Y plane, at half resolution
            let (uv_offset, uv_stride) = if let Some(planes) = &frame.yuv_planes {
                (planes.uv_offset, planes.uv_stride)
            } else {
                // Fallback: UV plane starts right after Y plane
                ((frame.stride * frame.height) as usize, frame.stride)
            };
            let uv_x = (x & !1) as usize; // round down to even
            let uv_y = (y / 2) as usize;
            let uv_idx = uv_offset + uv_y * uv_stride as usize + uv_x;

            if uv_idx + 1 >= data.len() {
                return (luma, luma, luma);
            }

            let (u, v) = if frame.format == PixelFormat::NV12 {
                (data[uv_idx], data[uv_idx + 1])
            } else {
                (data[uv_idx + 1], data[uv_idx])
            };

            yuv_to_rgb(luma, u, v)
        }
        PixelFormat::I420 => {
            let y_idx = (y * frame.stride + x) as usize;
            if y_idx >= data.len() {
                return (0, 0, 0);
            }
            let luma = data[y_idx];

            let (u_offset, u_stride, v_offset, v_stride) = if let Some(planes) = &frame.yuv_planes {
                (
                    planes.uv_offset,
                    planes.uv_stride,
                    planes.v_offset,
                    planes.v_stride,
                )
            } else {
                // Fallback: standard I420 layout
                let y_size = (frame.stride * frame.height) as usize;
                let half_stride = frame.stride / 2;
                let u_size = (half_stride * frame.height / 2) as usize;
                (y_size, half_stride, y_size + u_size, half_stride)
            };

            let cx = (x / 2) as usize;
            let cy = (y / 2) as usize;
            let u_idx = u_offset + cy * u_stride as usize + cx;
            let v_idx = v_offset + cy * v_stride as usize + cx;

            if u_idx >= data.len() || v_idx >= data.len() {
                return (luma, luma, luma);
            }

            yuv_to_rgb(luma, data[u_idx], data[v_idx])
        }
        PixelFormat::YUYV | PixelFormat::YVYU => {
            // Packed 4:2:2: two pixels share chroma
            // YUYV: Y0 U  Y1 V  (4 bytes per 2 pixels)
            // YVYU: Y0 V  Y1 U
            let pair_x = (x & !1) as usize; // round to even
            let base = (y as usize) * (frame.stride as usize) + pair_x * 2;
            if base + 3 >= data.len() {
                return (0, 0, 0);
            }
            let luma = if x & 1 == 0 {
                data[base]
            } else {
                data[base + 2]
            };
            let (u, v) = if frame.format == PixelFormat::YUYV {
                (data[base + 1], data[base + 3])
            } else {
                (data[base + 3], data[base + 1])
            };
            yuv_to_rgb(luma, u, v)
        }
        PixelFormat::UYVY | PixelFormat::VYUY => {
            // Packed 4:2:2: two pixels share chroma
            // UYVY: U  Y0 V  Y1 (4 bytes per 2 pixels)
            // VYUY: V  Y0 U  Y1
            let pair_x = (x & !1) as usize;
            let base = (y as usize) * (frame.stride as usize) + pair_x * 2;
            if base + 3 >= data.len() {
                return (0, 0, 0);
            }
            let luma = if x & 1 == 0 {
                data[base + 1]
            } else {
                data[base + 3]
            };
            let (u, v) = if frame.format == PixelFormat::UYVY {
                (data[base], data[base + 2])
            } else {
                (data[base + 2], data[base])
            };
            yuv_to_rgb(luma, u, v)
        }
    }
}

/// Convert YUV (BT.601) to RGB
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = y as f32;
    let u = u as f32 - 128.0;
    let v = v as f32 - 128.0;

    let r = (y + 1.402 * v).clamp(0.0, 255.0) as u8;
    let g = (y - 0.344136 * u - 0.714136 * v).clamp(0.0, 255.0) as u8;
    let b = (y + 1.772 * u).clamp(0.0, 255.0) as u8;

    (r, g, b)
}

/// Status bar widget
struct StatusBar<'a> {
    message: &'a str,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Fill background
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.set_char(' ');
                cell.set_bg(Color::DarkGray);
            }
        }

        // Render text
        let text = if self.message.len() > area.width as usize {
            &self.message[..area.width as usize]
        } else {
            self.message
        };

        buf.set_string(
            area.x,
            area.y,
            text,
            ratatui::style::Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray),
        );
    }
}
