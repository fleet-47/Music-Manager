//! Download link (F3): list the link with yt-dlp, then download each item as in F6.

use std::path::Path;

use futures::StreamExt;
use sdl_core::paths::{NameId, link_item_path, remove_tmp_dir_if_empty};
use sdl_core::{Attention, EventKind};
use sdl_youtube::{LinkItem, LinkListing, YoutubeError, download_audio, list_link};

use super::TaskError;
use super::progress::{Outcome, download_dir, drive, place, summary_line};
use super::runner::{Flow, TaskContext};

/// Run the whole link flow.
pub(super) async fn run(context: &TaskContext, url: &str) -> Result<Flow, TaskError> {
    let events = &context.events;
    events.send(EventKind::Headline("Reading the link".to_owned()));
    let listing = list_link(url).await?;
    events.send(EventKind::Headline("Downloading".to_owned()));
    let root = &context.config.music_root;
    let total = listing.items.len();
    let outcomes = futures::stream::iter(listing.items.iter().cloned())
        .map(|item| download_item(root, &listing, item))
        .buffer_unordered(context.config.download_jobs.get());
    let result = drive(outcomes, total, events).await;
    let cleanup = remove_tmp_dir_if_empty(root);
    let counts = result?;
    cleanup?;
    Ok(Flow::Done {
        line: summary_line(&counts),
    })
}

/// One item: exists? skip; download to a temp file; rename. Only a missing `yt-dlp` ends the run.
async fn download_item(
    root: &Path,
    listing: &LinkListing,
    item: LinkItem,
) -> Result<Outcome, TaskError> {
    let attention = |reason: String| Attention {
        title: item.title.clone(),
        artist: String::new(),
        reason,
    };
    let playlist = listing.playlist.as_ref().map(|list| NameId {
        name: &list.name,
        id: &list.id,
    });
    let dest = link_item_path(root, &item.site, playlist, &item.title, &item.id);
    match tokio::fs::try_exists(&dest).await {
        Ok(true) => return Ok(Outcome::Skipped),
        Ok(false) => {}
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    }
    // `_dir` is removed with everything in it when this function returns or is dropped.
    let (_dir, file) = match download_dir(root) {
        Ok(made) => made,
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    };
    match download_audio(&item.url, &file).await {
        Ok(()) => {}
        Err(YoutubeError::YtDlpNotFound) => {
            return Err(TaskError::Youtube(YoutubeError::YtDlpNotFound));
        }
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    }
    match place(&file, &dest).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(Outcome::Failed(attention(error.to_string()))),
    }
}
