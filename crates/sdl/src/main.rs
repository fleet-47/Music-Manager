//! `sdl` entry point.

use color_eyre::Section as _;
use color_eyre::eyre::Result;
use sdl_core::{Config, ConfigError};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::config::HookBuilder::default()
        .display_location_section(false)
        .display_env_section(false)
        .install()?;
    // A missing or malformed config file is reported before anything else, with what to do.
    let config = Config::load().map_err(|error| {
        let advice = config_advice(&error);
        color_eyre::Report::from(error).suggestion(advice)
    })?;
    sdl::screen::run(config).await
}

fn config_advice(error: &ConfigError) -> &'static str {
    match error {
        ConfigError::NoHome => "Set HOME, then start sdl again.",
        ConfigError::Read { .. } => {
            "Copy config.example.toml from the sdl folder to that path and fill in client_id."
        }
        ConfigError::Parse { .. } => {
            "Fix the line shown above; config.example.toml lists every setting."
        }
    }
}
