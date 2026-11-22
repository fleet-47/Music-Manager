//! The Download link task (F5). The conversation is documented on [`crate::Runner`].
//!
//! The form has one field, so the answer starts the run. The run lists each link flat, in turn
//! (the total is known once all are listed), reads each item's own metadata at its turn, and takes
//! it through intake: `YouTube` Music items direct, every other item through matching. Every link's
//! items are one run with one total. Each playlist link also writes its playlist file at the end.

use std::sync::{Mutex, PoisonError};

use futures::StreamExt;
use futures::stream;
use sdl_core::{Attention, EventKind, Library, WantedSong};
use sdl_library::playlists;
use sdl_youtube::{LinkItem, LinkListing, list_link, read_item};

use crate::conversation::{Answered, Answers, Offer, Opened, Recipient, Step};
use crate::progress::{Outcome, drive, summary_line};
use crate::tasks::intake::{Intake, Listed, Route, ends_the_run, failed};
use crate::tasks::send_libraries;
use crate::{Flow, TaskContext, TaskError};

/// The Download link task.
pub(crate) struct DownloadLink<'a> {
    context: &'a TaskContext,
    answered: Option<(Vec<String>, Recipient)>,
}

/// One link of the run, listed: its items, the playlist that lists them (for a playlist link), and
/// where its first item stands among all the run's items.
struct Listing {
    link: LinkListing,
    from: Vec<Listed>,
    first: usize,
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
/// failed item with the reason (an unavailable video says so), not a dropped one. `at` is its
/// place among all the run's items, `number` its place in its own link.
async fn read_and_take(
    at: usize,
    number: usize,
    item: &LinkItem,
    from: &[Listed],
    library: &Library,
    intake: &Intake<'_>,
    read: &Mutex<Vec<(usize, WantedSong)>>,
) -> Result<Outcome, TaskError> {
    if item.url.is_empty() {
        let title = if item.title.is_empty() {
            format!("Item {} of the playlist", number.saturating_add(1))
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
        let Answers::DownloadLink { urls, recipient } = answers else {
            return std::future::ready(Ok(Answered::Ignored));
        };
        self.answered = Some((urls, recipient));
        std::future::ready(Ok(Answered::Run))
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let (urls, recipient) = self
            .answered
            .take()
            .unwrap_or_else(|| (Vec::new(), Recipient::Me));
        // Each link is listed in turn. One that cannot be read is a failed item and the others go
        // on; when none can be read, the run ends with the first reason.
        let mut listings: Vec<Listing> = Vec::with_capacity(urls.len());
        let mut unread: Vec<Outcome> = Vec::new();
        let mut first_error = None;
        let mut items = 0_usize;
        for (at, url) in urls.iter().enumerate() {
            let headline = if urls.len() == 1 {
                "Reading the link".to_owned()
            } else {
                format!("Reading link {} of {}", at.saturating_add(1), urls.len())
            };
            context.events.send(EventKind::Headline(headline));
            match list_link(url.trim()).await.map_err(TaskError::from) {
                Ok(link) => {
                    let from = link
                        .playlist
                        .iter()
                        .map(|info| Listed::new(&info.name, &info.id))
                        .collect();
                    let first = items;
                    items = items.saturating_add(link.items.len());
                    listings.push(Listing { link, from, first });
                }
                Err(error) if ends_the_run(&error) => return Err(error),
                Err(error) => {
                    unread.push(failed(url, "", &error));
                    first_error.get_or_insert(error);
                }
            }
        }
        if listings.is_empty()
            && let Some(error) = first_error
        {
            return Err(error);
        }
        let library = recipient.make(&context.config)?;
        context.remember_friend(&library)?;
        let read: Mutex<Vec<(usize, WantedSong)>> = Mutex::default();
        let intake = Intake::new(context);
        // Plain loops, not closures: see the note in the Sync Spotify task.
        let mut pending = Vec::with_capacity(items);
        for listing in &listings {
            for (number, item) in listing.link.items.iter().enumerate() {
                let at = listing.first.saturating_add(number);
                pending.push(read_and_take(
                    at,
                    number,
                    item,
                    &listing.from,
                    &library,
                    &intake,
                    &read,
                ));
            }
        }
        let total = items.saturating_add(unread.len());
        let outcomes = stream::iter(unread)
            .map(Ok::<Outcome, TaskError>)
            .chain(stream::iter(pending).buffer_unordered(context.config.download_jobs.get()));
        let tally = drive(outcomes, total, &context.events).await?;
        let mut songs = read.into_inner().unwrap_or_else(PoisonError::into_inner);
        songs.sort_by_key(|(at, _)| *at);
        for listing in &listings {
            let Some(info) = &listing.link.playlist else {
                continue;
            };
            context
                .events
                .send(EventKind::Headline("Writing the playlist file".to_owned()));
            let end = listing.first.saturating_add(listing.link.items.len());
            let listed: Vec<WantedSong> = songs
                .iter()
                .filter(|(at, _)| (listing.first..end).contains(at))
                .map(|(_, song)| song.clone())
                .collect();
            playlists::write(&library, &info.name, &info.id, &listed)?;
        }
        Ok(Flow::done(summary_line(&tally), &tally.counts()))
    }
}
