//! Download link (F3): take the link and format, list the link with yt-dlp, then download each
//! new item as in F6.

use std::path::Path;

use futures::StreamExt;
use sdl_core::paths::{NameId, Target, link_item_target};
use sdl_core::{Attention, Counts, EventKind, Format};
use sdl_youtube::{LinkItem, LinkListing, TagsFrom, YoutubeError, download_audio, list_link};

use super::TaskError;
use super::form::{Answered, Answers, Step};
use super::progress::{Outcome, download_dir, drive, place, summary_line, with_already};
use super::runner::{Flow, TaskContext};

/// Download link as a step form: the answers carry everything, so the run starts at once.
pub(super) struct Link<'a> {
    context: &'a TaskContext,
    url: String,
    format: Format,
}

impl<'a> Link<'a> {
    pub(super) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            url: String::new(),
            format: Format::default(),
        }
    }
}

impl Link<'_> {
    fn take(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::Link { url, format } = answers else {
            return Ok(Answered::Ignored);
        };
        self.context.remember_format(format)?;
        url.trim().clone_into(&mut self.url);
        self.format = format.format;
        Ok(Answered::Run)
    }
}

impl Step for Link<'_> {
    fn open(&mut self) -> impl Future<Output = Result<(), TaskError>> {
        self.context
            .events
            .send(EventKind::Headline("Waiting for the link".to_owned()));
        std::future::ready(Ok(()))
    }

    fn answer(&mut self, answers: Answers) -> impl Future<Output = Result<Answered, TaskError>> {
        std::future::ready(self.take(answers))
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let events = &context.events;
        events.send(EventKind::Headline("Reading the link".to_owned()));
        let listing = list_link(&self.url).await?;
        let root = &context.config.music_root;
        let (new, already) = new_items(root, &listing)?;
        if new.is_empty() {
            events.send(EventKind::Counts(Counts::default()));
            let counts = Counts::default();
            return Ok(Flow::done(
                summary_line(&with_already(counts, already)),
                &counts,
            ));
        }
        events.send(EventKind::Headline("Downloading".to_owned()));
        let total = new.len();
        let format = self.format;
        let outcomes = futures::stream::iter(new)
            .map(|item| download_item(root, &listing, item, format))
            .buffer_unordered(context.config.download_jobs.get());
        let counts = drive(outcomes, total, events).await?;
        Ok(Flow::done(
            summary_line(&with_already(counts, already)),
            &counts,
        ))
    }
}

/// Where `item` goes.
fn target_of(root: &Path, listing: &LinkListing, item: &LinkItem) -> Target {
    let playlist = listing.playlist.as_ref().map(|list| NameId {
        name: &list.name,
        id: &list.id,
    });
    link_item_target(root, &item.site, playlist, &item.title, &item.id)
}

/// The items of `listing` that are not in the library yet (in any format), and how many are.
/// Settled before any download starts, so a link already fetched costs no network call.
fn new_items(root: &Path, listing: &LinkListing) -> Result<(Vec<LinkItem>, usize), TaskError> {
    let mut new = Vec::new();
    for item in &listing.items {
        if !target_of(root, listing, item).is_present()? {
            new.push(item.clone());
        }
    }
    let already = listing.items.len().saturating_sub(new.len());
    Ok((new, already))
}

/// One item: download to a temp file; rename last. Only a missing `yt-dlp` ends the run.
async fn download_item(
    root: &Path,
    listing: &LinkListing,
    item: LinkItem,
    format: Format,
) -> Result<Outcome, TaskError> {
    let attention = |reason: String| Attention::failed(item.title.clone(), String::new(), reason);
    let target = target_of(root, listing, &item);
    // `_dir` is removed with everything in it when this function returns or is dropped.
    let (_dir, file) = match download_dir(root, format) {
        Ok(made) => made,
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    };
    match download_audio(&item.url, &file, format, TagsFrom::YtDlp).await {
        Ok(()) => {}
        Err(YoutubeError::YtDlpNotFound) => {
            return Err(TaskError::Youtube(YoutubeError::YtDlpNotFound));
        }
        Err(error) => return Ok(Outcome::Failed(attention(error.to_string()))),
    }
    match place(&file, &target.path(format)).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(Outcome::Failed(attention(error.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn item(title: &str, id: &str) -> LinkItem {
        LinkItem {
            id: id.to_owned(),
            title: title.to_owned(),
            url: format!("https://x/{id}"),
            site: "Youtube".to_owned(),
        }
    }

    #[test]
    fn items_present_in_any_format_are_left_out_before_any_download() -> TestResult {
        let root = tempfile::tempdir()?;
        let listing = LinkListing {
            playlist: None,
            items: vec![item("Old", "1"), item("Fresh", "2"), item("Older", "3")],
        };
        let target = |item: &LinkItem| target_of(root.path(), &listing, item);
        for (title, format) in [("Old", Format::Mp3), ("Older", Format::M4a)] {
            let found = listing.items.iter().find(|i| i.title == title);
            let path = found.map(|i| target(i).path(format)).ok_or("item")?;
            std::fs::create_dir_all(path.parent().ok_or("parent")?)?;
            std::fs::write(path, b"x")?;
        }
        let (new, already) = new_items(root.path(), &listing)?;
        assert_eq!(already, 2);
        assert_eq!(
            new.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["Fresh"]
        );
        Ok(())
    }
}
