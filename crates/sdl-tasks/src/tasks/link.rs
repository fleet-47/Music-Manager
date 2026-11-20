//! The Download link task (F5). The conversation is documented on [`crate::Runner`].
//!
//! The form has one field, so the answer starts the run. The run lists the link flat (the total is
//! known from then on), reads each item's own metadata at its turn, and takes it through intake:
//! `YouTube` Music items direct, every other item through matching. A playlist link also writes its
//! playlist file at the end.

use std::sync::{Mutex, PoisonError};

use futures::StreamExt;
use futures::stream;
use sdl_core::{EventKind, WantedSong};
use sdl_library::playlists;
use sdl_youtube::{LinkItem, list_link, read_item};

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::progress::{Outcome, drive, summary_line};
use crate::tasks::intake::{Intake, Route, ends_the_run, failed};
use crate::{Flow, TaskContext, TaskError};

/// The Download link task.
pub(crate) struct DownloadLink<'a> {
    context: &'a TaskContext,
    url: Option<String>,
}

impl<'a> DownloadLink<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self { context, url: None }
    }
}

/// One item at its turn: read its own metadata, then take it. An item that cannot be read is a
/// failed item with the reason (an unavailable video says so), not a dropped one.
async fn read_and_take(
    at: usize,
    item: &LinkItem,
    from: &[String],
    intake: &Intake<'_>,
    read: &Mutex<Vec<(usize, WantedSong)>>,
) -> Result<Outcome, TaskError> {
    let song = match read_item(item).await {
        Ok(song) => song,
        Err(error) => {
            let error = TaskError::from(error);
            if ends_the_run(&error) {
                return Err(error);
            }
            let title = if item.title.is_empty() {
                &item.url
            } else {
                &item.title
            };
            return Ok(failed(title, "", &error));
        }
    };
    read.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((at, song.clone()));
    intake.take(&song, from, Route::Search).await
}

impl Step for DownloadLink<'_> {
    fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> + Send {
        self.context
            .events
            .send(EventKind::Headline("Paste a link".to_owned()));
        std::future::ready(Ok(Opened::Ready))
    }

    fn answer(
        &mut self,
        answers: Answers,
    ) -> impl Future<Output = Result<Answered, TaskError>> + Send {
        let Answers::DownloadLink { url } = answers else {
            return std::future::ready(Ok(Answered::Ignored));
        };
        self.url = Some(url);
        std::future::ready(Ok(Answered::Run))
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let url = self.url.take().unwrap_or_default();
        context
            .events
            .send(EventKind::Headline("Reading the link".to_owned()));
        let listing = list_link(url.trim()).await?;
        let playlist = listing.playlist;
        let from: Vec<String> = playlist.iter().map(|info| info.name.clone()).collect();
        let read: Mutex<Vec<(usize, WantedSong)>> = Mutex::default();
        let intake = Intake::new(context);
        let total = listing.items.len();
        // Plain loops, not closures: see the note in the Sync Spotify task.
        let mut pending = Vec::with_capacity(total);
        for (at, item) in listing.items.iter().enumerate() {
            pending.push(read_and_take(at, item, &from, &intake, &read));
        }
        let outcomes = stream::iter(pending).buffer_unordered(context.config.download_jobs.get());
        let tally = drive(outcomes, total, &context.events).await?;
        if let Some(info) = playlist {
            context
                .events
                .send(EventKind::Headline("Writing the playlist file".to_owned()));
            let mut songs = read.into_inner().unwrap_or_else(PoisonError::into_inner);
            songs.sort_by_key(|(at, _)| *at);
            let songs: Vec<WantedSong> = songs.into_iter().map(|(_, song)| song).collect();
            playlists::write(&context.config.music_root, &info.name, &info.id, &songs)?;
        }
        Ok(Flow::done(summary_line(&tally), &tally.counts()))
    }
}
