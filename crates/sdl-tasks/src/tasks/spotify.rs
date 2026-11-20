//! The Sync Spotify task (F4). The conversation is documented on [`crate::Runner`].
//!
//! Order of what the owner sees: a headline at once ("Checking your Spotify sign-in"); then either
//! the lists, or the notice before the browser sign-in; after the form's answer a headline per page
//! while listing and then `N new of M, K waiting in Review`; after Start the downloads, and the
//! playlist files last.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use futures::StreamExt;
use futures::stream;
use sdl_core::{EventKind, ListChoice, ReviewList, SignInState, WantedSong};
use sdl_library::playlists;
use sdl_spotify::{ListedSongs, Session, list_choices, list_songs, resume, sign_in_browser};

use crate::conversation::{Answered, Answers, Opened, Step};
use crate::progress::{drive, summary_line, with_already};
use crate::tasks::intake::{Intake, Route};
use crate::{Flow, TaskContext, TaskError};

/// The Sync Spotify task.
pub(crate) struct SyncSpotify<'a> {
    context: &'a TaskContext,
    session: Option<Session>,
    keychain_told: bool,
    /// Every list already read, so changing a tick never asks Spotify twice for the same list.
    listed: HashMap<String, ListedSongs>,
    /// The lists ticked at the last answer.
    chosen: Vec<ListChoice>,
}

impl<'a> SyncSpotify<'a> {
    pub(crate) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            session: None,
            keychain_told: false,
            listed: HashMap::new(),
            chosen: Vec::new(),
        }
    }

    /// Tell the owner once, per run, why the Keychain could not be used.
    fn tell_keychain(&mut self, problem: Option<String>) {
        if let (Some(reason), false) = (problem, self.keychain_told) {
            self.keychain_told = true;
            self.context
                .events
                .send(EventKind::SignIn(SignInState::KeychainProblem(reason)));
        }
    }

    /// Signed in: say so, offer the lists, keep the session.
    async fn offer_lists(&mut self, session: Session) -> Result<(), TaskError> {
        let events = &self.context.events;
        events.send(EventKind::SignIn(SignInState::SignedIn));
        events.send(EventKind::Headline("Reading your Spotify lists".to_owned()));
        let choices = list_choices(&session).await?;
        events.send(EventKind::Lists(choices));
        self.session = Some(session);
        Ok(())
    }

    /// The plan for the lists ticked now, from what is on disk and in the settings (no search).
    fn plan(&self) -> Result<Plan, TaskError> {
        let listed: Vec<&ListedSongs> = self
            .chosen
            .iter()
            .filter_map(|list| self.listed.get(&list.id))
            .collect();
        let state = self.context.state();
        Ok(Plan::make(
            &self.context.config.music_root,
            &self.context.review(),
            &state.never,
            &listed,
        )?)
    }
}

/// What a sync would do, counted without any search.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Songs to look for, with the names of the lists that name them, in list order.
    new: Vec<(WantedSong, Vec<String>)>,
    /// Songs already in the library.
    already: usize,
    /// Songs already waiting in the review list.
    waiting: usize,
    /// Distinct songs in the ticked lists.
    total: usize,
}

impl Plan {
    /// Count each song once by its Spotify track ID. A song is new when it is not in the library,
    /// not held and not marked "never".
    fn make(
        music_root: &Path,
        review: &ReviewList,
        never: &BTreeSet<String>,
        listed: &[&ListedSongs],
    ) -> std::io::Result<Self> {
        let mut order: Vec<(WantedSong, Vec<String>)> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for list in listed {
            for song in &list.songs {
                if let Some(&at) = index.get(song.key()) {
                    if let Some((_, names)) = order.get_mut(at)
                        && !names.contains(&list.list.name)
                    {
                        names.push(list.list.name.clone());
                    }
                } else {
                    index.insert(song.key(), order.len());
                    order.push((song.clone(), vec![list.list.name.clone()]));
                }
            }
        }
        let total = order.len();
        let (mut already, mut waiting) = (0_usize, 0_usize);
        let mut new = Vec::new();
        for (song, names) in order {
            if review.contains(song.key()) {
                waiting = waiting.saturating_add(1);
            } else if never.contains(song.key()) {
                // Marked "never": left alone, and not counted as new or already there.
            } else if sdl_core::Place::of(music_root, &song).present()?.is_some() {
                already = already.saturating_add(1);
            } else {
                new.push((song, names));
            }
        }
        Ok(Self {
            new,
            already,
            waiting,
            total,
        })
    }
}

impl Step for SyncSpotify<'_> {
    async fn open(&mut self) -> Result<Opened, TaskError> {
        let events = &self.context.events;
        events.send(EventKind::Headline(
            "Checking your Spotify sign-in".to_owned(),
        ));
        let resumed = resume(&self.context.config.client_id).await?;
        self.tell_keychain(resumed.keychain_problem);
        if let Some(session) = resumed.session {
            self.offer_lists(session).await?;
            return Ok(Opened::Ready);
        }
        self.context
            .events
            .send(EventKind::SignIn(SignInState::NeedsBrowser));
        self.context.events.send(EventKind::Headline(
            "Waiting for you to continue to Spotify".to_owned(),
        ));
        Ok(Opened::WaitsForConsent)
    }

    async fn after_consent(&mut self) -> Result<(), TaskError> {
        self.context.events.send(EventKind::Headline(
            "Allow access in your browser, then come back".to_owned(),
        ));
        let signed_in =
            sign_in_browser(&self.context.config.client_id, &self.context.events).await?;
        self.tell_keychain(signed_in.keychain_problem);
        self.offer_lists(signed_in.session).await
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::SyncSpotify { lists } = answers else {
            return Ok(Answered::Ignored);
        };
        let Some(session) = &self.session else {
            return Ok(Answered::Ignored);
        };
        let events = &self.context.events;
        self.context
            .remember_ticked(lists.iter().map(|list| list.id.clone()).collect())?;
        let missing: Vec<ListChoice> = lists
            .iter()
            .filter(|list| !self.listed.contains_key(&list.id))
            .cloned()
            .collect();
        if !missing.is_empty() {
            events.send(EventKind::Headline("Listing your songs".to_owned()));
            for songs in list_songs(session, &missing, events).await? {
                self.listed.insert(songs.list.id.clone(), songs);
            }
        }
        self.chosen = lists;
        events.send(EventKind::Headline("Counting what is new".to_owned()));
        let plan = self.plan()?;
        events.send(EventKind::NewOfTotal {
            new: plan.new.len(),
            total: plan.total,
            waiting: plan.waiting,
        });
        // A plan with no new song can still start: the run rewrites the playlist files.
        Ok(Answered::Waiting { can_start: true })
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let plan = self.plan()?;
        let total = plan.new.len();
        context.events.send(EventKind::Headline(
            "Finding songs on YouTube Music".to_owned(),
        ));
        let intake = Intake::new(context);
        // Plain loops, not closures: a closure over a borrowed item makes the future unprovable
        // as `Send` for every lifetime (rustc), and the runner spawns it.
        let mut pending = Vec::with_capacity(total);
        for (song, from) in &plan.new {
            pending.push(intake.take(song, from, Route::Search));
        }
        let outcomes = stream::iter(pending).buffer_unordered(context.config.download_jobs.get());
        let tally = drive(outcomes, total, &context.events).await?;
        context
            .events
            .send(EventKind::Headline("Writing playlist files".to_owned()));
        for list in self
            .chosen
            .iter()
            .filter_map(|list| self.listed.get(&list.id))
        {
            playlists::write(
                &context.config.music_root,
                &list.list.name,
                &list.list.id,
                &list.songs,
            )?;
        }
        let line = summary_line(&with_already(tally, plan.already));
        Ok(Flow::done(line, &tally.counts()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sdl_core::{Format, Place, SongSource};

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn song(id: &str, title: &str) -> WantedSong {
        WantedSong {
            source: SongSource::Spotify {
                track_id: id.into(),
            },
            title: title.into(),
            artists: vec!["Band".into()],
            album: Some("Record".into()),
            album_artist: None,
            track_number: None,
            disc_number: None,
            date: None,
            isrc: None,
            duration: Some(Duration::from_secs(200)),
            cover_url: None,
        }
    }

    fn list(id: &str, name: &str, songs: Vec<WantedSong>) -> ListedSongs {
        ListedSongs {
            list: ListChoice {
                name: name.into(),
                id: id.into(),
                count: songs.len(),
            },
            songs,
        }
    }

    #[test]
    fn the_plan_counts_each_song_once_and_searches_nothing() -> TestResult {
        let root = tempfile::tempdir()?;
        let here = song("here", "Here");
        let place = Place::of(root.path(), &here);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::M4a), b"x")?;
        let mut review = ReviewList::default();
        review.hold(sdl_core::HeldSong {
            song: song("held", "Held"),
            from: Vec::new(),
            candidates: Vec::new(),
            reason: "no result".into(),
        });
        let never = BTreeSet::from(["never".to_owned()]);
        let liked = list(
            "liked-songs",
            "Liked Songs",
            vec![
                song("a", "A"),
                here.clone(),
                song("held", "Held"),
                song("never", "Never"),
            ],
        );
        let road = list("p1", "Road trip", vec![song("a", "A"), song("b", "B")]);
        let plan = Plan::make(root.path(), &review, &never, &[&liked, &road])?;
        assert_eq!((plan.total, plan.waiting, plan.already), (5, 1, 1));
        let new: Vec<(&str, Vec<&str>)> = plan
            .new
            .iter()
            .map(|(song, names)| (song.key(), names.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            new,
            [
                ("a", vec!["Liked Songs", "Road trip"]),
                ("b", vec!["Road trip"])
            ]
        );
        Ok(())
    }
}
