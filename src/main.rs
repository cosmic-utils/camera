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
        None => run_gui(),
    }
}

fn run_gui() -> Result<(), Box<dyn std::error::Error>> {
    // Get the system's preferred languages.
    let requested_languages = i18n_embed::DesktopLanguageRequester::requested_languages();

    // Enable localizations to be applied.
    i18n::init(&requested_languages);

    // Settings for configuring the application window and iced runtime.
    let settings = cosmic::app::Settings::default().size_limits(
        cosmic::iced::Limits::NONE
            .min_width(360.0)
            .min_height(180.0),
    );

    // Starts the application's event loop with `()` as the application's flags.
    cosmic::app::run::<AppModel>(settings, ())?;

    Ok(())
}
