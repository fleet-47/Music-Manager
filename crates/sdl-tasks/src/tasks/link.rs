//! The Download link task (F5). The conversation is documented on [`crate::Runner`].
//!
//! The form has one field, so the answer starts the run. The run lists the link flat (the total is
//! known from then on), reads each item's own metadata at its turn, and takes it through intake:
//! `YouTube` Music items direct, every other item through matching. A playlist link also writes its
//! playlist file at the end.

use std::sync::{Mutex, PoisonError};

use futures::StreamExt;
use futures::stream;
use sdl_core::{Attention, EventKind, Library, WantedSong};
use sdl_library::playlists;
use sdl_youtube::{LinkItem, list_link, read_item};

use crate::conversation::{Answered, Answers, Offer, Opened, Recipient, Step};
use crate::progress::{Outcome, drive, summary_line};
use crate::tasks::intake::{Intake, Listed, Route, ends_the_run, failed};
use crate::tasks::send_libraries;
use crate::{Flow, TaskContext, TaskError};

/// The Download link task.
pub(crate) struct DownloadLink<'a> {
    context: &'a TaskContext,
    answered: Option<(String, Recipient)>,
}

impl<'a> DownloadLink<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            answered: None,
        }
    }
}

/// One item at its turn: read its own metadata, then take it. An item that cannot be read is a
/// failed item with the reason (an unavailable video says so), not a dropped one.
async fn read_and_take(
    at: usize,
    item: &LinkItem,
    from: &[Listed],
    library: &Library,
    intake: &Intake<'_>,
    read: &Mutex<Vec<(usize, WantedSong)>>,
) -> Result<Outcome, TaskError> {
    if item.url.is_empty() {
        let title = if item.title.is_empty() {
            format!("Item {} of the playlist", at.saturating_add(1))
        } else {
            item.title.clone()
        };
        return Ok(Outcome::Failed(Attention::failed(
            title,
            String::new(),
            "The site reports this entry as unavailable (private or removed). There is nothing to download."
                .to_owned(),
        )));
    }
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
    intake.take(library, &song, from, Route::Search).await
}

impl Step for DownloadLink<'_> {
    fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> + Send {
        let events = &self.context.events;
        let opened = send_libraries(self.context).map(|()| {
            events.send(EventKind::Headline("Paste a link".to_owned()));
            Opened::Ready
        });
        std::future::ready(opened)
    }

    fn offer(&mut self, offer: Offer) -> impl Future<Output = Result<bool, TaskError>> + Send {
        let sent = if offer == Offer::Libraries {
            send_libraries(self.context)
        } else {
            Ok(())
        };
        std::future::ready(sent.map(|()| false))
    }

    fn answer(
        &mut self,
        answers: Answers,
    ) -> impl Future<Output = Result<Answered, TaskError>> + Send {
        let Answers::DownloadLink { url, recipient } = answers else {
            return std::future::ready(Ok(Answered::Ignored));
        };
        self.answered = Some((url, recipient));
        std::future::ready(Ok(Answered::Run))
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let (url, recipient) = self
            .answered
            .take()
            .unwrap_or_else(|| (String::new(), Recipient::Me));
        context
            .events
            .send(EventKind::Headline("Reading the link".to_owned()));
        let listing = list_link(url.trim()).await?;
        let playlist = listing.playlist;
        let from: Vec<Listed> = playlist
            .iter()
            .map(|info| Listed::new(&info.name, &info.id))
            .collect();
        let library = recipient.make(&context.config)?;
        context.remember_friend(&library)?;
        let read: Mutex<Vec<(usize, WantedSong)>> = Mutex::default();
        let intake = Intake::new(context);
        let total = listing.items.len();
        // Plain loops, not closures: see the note in the Sync Spotify task.
        let mut pending = Vec::with_capacity(total);
        for (at, item) in listing.items.iter().enumerate() {
            pending.push(read_and_take(at, item, &from, &library, &intake, &read));
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
            playlists::write(&library, &info.name, &info.id, &songs)?;
        }
        Ok(Flow::done(summary_line(&tally), &tally.counts()))
    }
}
