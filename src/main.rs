// SPDX-License-Identifier: GPL-3.0-only

use camera::app::AppModel;
use camera::i18n;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod cli;

#[derive(Parser)]
#[command(name = "camera")]
#[command(about = "Camera application for the COSMIC desktop")]
#[command(version)]
#[command(subcommand_required = false)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Use an image or video file as the camera preview source instead of a real camera.
    /// Useful for testing, demos, or taking screenshots with consistent content.
    /// Supported formats: PNG, JPG, JPEG, WEBP (images) or MP4, WEBM, MKV (videos)
    #[arg(long, value_name = "FILE")]
    preview_source: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run in terminal mode (renders camera to terminal)
    Terminal,

    /// List available cameras
    List,

    /// Take a photo
    Photo {
        /// Camera index to use (from 'camera list')
        #[arg(short, long, default_value = "0")]
        camera: usize,

        /// Output file path (default: ~/Pictures/camera/photo_TIMESTAMP.jpg)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Record a video
    Video {
        /// Camera index to use (from 'camera list')
        #[arg(short, long, default_value = "0")]
        camera: usize,

        /// Recording duration in seconds
        #[arg(short, long, default_value = "10")]
        duration: u64,

        /// Output file path (default: ~/Videos/camera/video_TIMESTAMP.mp4)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Enable audio recording
        #[arg(short, long)]
        audio: bool,
    },

    /// Process images through computational photography pipelines
    Process {
        #[command(subcommand)]
        mode: ProcessMode,
    },
}

#[derive(Subcommand)]
enum ProcessMode {
    /// Burst mode: multi-frame denoising and HDR+ pipeline
    BurstMode {
        /// Input images or directory containing images (PNG, DNG supported)
        #[arg(required = true)]
        input: Vec<PathBuf>,

        /// Output directory for processed images (default: same as input or ~/Pictures/camera)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    // Set RUST_LOG environment variable to control log level
    // Examples: RUST_LOG=debug, RUST_LOG=camera=debug, RUST_LOG=info
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(true)
        .with_level(true)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Terminal) => camera::terminal::run(),
        Some(Commands::List) => cli::list_cameras(),
        Some(Commands::Photo { camera, output }) => cli::take_photo(camera, output),
        Some(Commands::Video {
            camera,
            duration,
            output,
            audio,
        }) => cli::record_video(camera, duration, output, audio),
        Some(Commands::Process { mode }) => match mode {
            ProcessMode::BurstMode { input, output } => cli::process_burst_mode(input, output),
        },
        None => run_gui(cli.preview_source),
    }
}

fn run_gui(preview_source: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Settings for configuring the application window and iced runtime.
    let mut settings = cosmic::app::Settings::default().size_limits(
        cosmic::iced::Limits::NONE
            .min_width(360.0)
            .min_height(180.0),
    );

    // When preview source is provided, set optimal window size for Flathub screenshots
    // Flathub recommends 1000x700 or smaller for standard displays
    if preview_source.is_some() {
        settings = settings.size(cosmic::iced::Size::new(900.0, 700.0));
    }

    // Create app flags with optional preview source
    let flags = camera::app::AppFlags { preview_source };

    // Starts the application's event loop with flags
    cosmic::app::run::<AppModel>(settings, flags)?;

    Ok(())
}
