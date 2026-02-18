// SPDX-License-Identifier: GPL-3.0-only

//! Terminal-based camera viewer
//!
//! Renders camera feed to the terminal using Unicode half-block characters
//! for improved vertical resolution.

use crate::backends::camera::pipewire::{
    PipeWirePipeline, enumerate_pipewire_cameras, get_pipewire_formats,
};
use crate::backends::camera::types::{CameraDevice, CameraFormat, CameraFrame};

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

    let mut current_camera_index = 0;
    let mut pipeline = initialize_camera(&cameras[current_camera_index])?;

    let mut frame_widget = FrameWidget::new();
    let mut status_message = format!(
        "Camera: {} | Press 's' to switch, 'q' or Ctrl+C to quit",
        cameras[current_camera_index].name
    );

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

            // 's' to switch camera
            if key.code == KeyCode::Char('s') && cameras.len() > 1 {
                current_camera_index = (current_camera_index + 1) % cameras.len();

                // Drop old pipeline first
                drop(pipeline);

                match initialize_camera(&cameras[current_camera_index]) {
                    Ok(new_pipeline) => {
                        pipeline = new_pipeline;
                        status_message = format!(
                            "Camera: {} | Press 's' to switch, 'q' or Ctrl+C to quit",
                            cameras[current_camera_index].name
                        );
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
    let x = x.min(frame.width - 1);
    let y = y.min(frame.height - 1);

    let idx = (y * frame.stride + x * 4) as usize;

    if idx + 2 < frame.data.len() {
        let r = frame.data[idx];
        let g = frame.data[idx + 1];
        let b = frame.data[idx + 2];
        Color::Rgb(r, g, b)
    } else {
        Color::Black
    }
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
