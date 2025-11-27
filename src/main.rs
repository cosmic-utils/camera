// SPDX-License-Identifier: MPL-2.0

use cosmic_camera::app::AppModel;
use cosmic_camera::i18n;

fn main() -> cosmic::iced::Result {
    // Initialize logging
    // Set RUST_LOG environment variable to control log level
    // Examples: RUST_LOG=debug, RUST_LOG=cosmic_camera=debug, RUST_LOG=info
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(true)
        .with_level(true)
        .init();

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
    cosmic::app::run::<AppModel>(settings, ())
}
