//! The tools check (F13): which outside tools the functions need are missing, and installing them
//! with mise.

use std::ffi::OsString;

use sdl_core::process::{self, ToolError};

/// The tools the functions run, as the command name and the name mise installs it under.
const TOOLS: [(&str, &str); 3] = [
    ("yt-dlp", "yt-dlp"),
    ("ffmpeg", "conda:ffmpeg"),
    ("deno", "deno"),
];

/// The tools that are not on `PATH`, by command name.
#[must_use]
pub fn missing_tools() -> Vec<String> {
    TOOLS
        .iter()
        .filter(|(command, _)| !process::on_path(command))
        .map(|(command, _)| (*command).to_owned())
        .collect()
}

/// The names mise installs the tools called `names` (by command name) under.
fn mise_names(names: &[String]) -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|(command, _)| names.iter().any(|name| name == command))
        .map(|(_, mise_name)| *mise_name)
        .collect()
}

/// Run `mise install` for the tools named (by command name).
///
/// # Errors
/// What went wrong, in plain words: mise is missing, could not be run, or said why it failed.
pub async fn install_tools(names: &[String]) -> Result<(), String> {
    let mut args = vec![OsString::from("install")];
    args.extend(mise_names(names).into_iter().map(OsString::from));
    process::run("mise", &args)
        .await
        .map(drop)
        .map_err(|error| match error {
            ToolError::Failed { reason, .. } => reason,
            other => other.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_map_to_the_names_mise_installs_them_under() {
        let wanted = ["ffmpeg".to_owned(), "yt-dlp".to_owned(), "other".to_owned()];
        assert_eq!(mise_names(&wanted), ["yt-dlp", "conda:ffmpeg"]);
    }
}
