//! The Sync Spotify task (F4). The conversation is documented on [`crate::Runner`].
//!
//! Order of what the owner sees: a headline at once ("Checking your Spotify sign-in"); then either
//! the lists, or the notice before the browser sign-in; after the form's answer a headline per page
//! while listing and then `N new of M, K waiting in Review`; after Start the downloads, and the
//! playlist files last.

use std::collections::HashMap;

use futures::StreamExt;
use futures::stream;
use sdl_core::{
    Attention, EventKind, Library, ListChoice, ReviewList, SignInState, State, WantedSong,
};
use sdl_library::playlists;
use sdl_spotify::{ListedSongs, Session, list_choices, list_songs, resume, sign_in_browser};

use crate::conversation::{Answered, Answers, Offer, Opened, Recipient, Step};
use crate::progress::{drive, summary_line, with_already};
use crate::tasks::intake::{Intake, Listed, Route};
use crate::tasks::send_libraries;
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
    /// Whose library the last answer chose.
    recipient: Recipient,
}

impl<'a> SyncSpotify<'a> {
    pub(crate) fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            session: None,
            keychain_told: false,
            listed: HashMap::new(),
            chosen: Vec::new(),
            recipient: Recipient::Me,
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

    /// A Keychain write that failed after sign-in (a refresh during the run) is told once, as an
    /// attention note; the run goes on. Nothing is said when the owner was already told (`SignIn`).
    fn note_keychain(&mut self) {
        let problem = self.session.as_ref().and_then(Session::keychain_problem);
        self.note_late_keychain(problem);
    }

    fn note_late_keychain(&mut self, problem: Option<String>) {
        if let (Some(reason), false) = (problem, self.keychain_told) {
            self.keychain_told = true;
            self.context
                .events
                .send(EventKind::Attention(Attention::failed(
                    "Spotify sign-in".to_owned(),
                    String::new(),
                    reason,
                )));
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
        self.note_keychain();
        Ok(())
    }

    /// The plan for the lists ticked now, from what is on disk and in the settings (no search).
    fn plan(&self) -> Result<Plan, TaskError> {
        let listed: Vec<&ListedSongs> = self
            .chosen
            .iter()
            .filter_map(|list| self.listed.get(&list.id))
            .collect();
        let library = self.recipient.library(&self.context.config)?;
        Ok(Plan::make(
            &library,
            &self.context.review(),
            &self.context.state(),
            &listed,
        )?)
    }
}

/// What a sync would do, counted without any search.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// Songs to look for, with the names of the lists that name them, in list order.
    new: Vec<(WantedSong, Vec<Listed>)>,
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
        library: &Library,
        review: &ReviewList,
        state: &State,
        listed: &[&ListedSongs],
    ) -> std::io::Result<Self> {
        let mut order: Vec<(WantedSong, Vec<Listed>)> = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        for list in listed {
            for song in &list.songs {
                if let Some(&at) = index.get(song.key()) {
                    let listed = Listed::new(&list.list.name, &list.list.id);
                    if let Some((_, lists)) = order.get_mut(at)
                        && !lists.contains(&listed)
                    {
                        lists.push(listed);
                    }
                } else {
                    index.insert(song.key(), order.len());
                    order.push((
                        song.clone(),
                        vec![Listed::new(&list.list.name, &list.list.id)],
                    ));
                }
            }
        }
        let total = order.len();
        let id = library.id();
        let (mut already, mut waiting) = (0_usize, 0_usize);
        let mut new = Vec::new();
        for (song, lists) in order {
            if review.contains(&id, song.key()) {
                waiting = waiting.saturating_add(1);
            } else if state.is_never(&id, song.key()) {
                // Marked "never": left alone, and not counted as new or already there.
            } else if sdl_core::Place::of(library, &song).present()?.is_some() {
                already = already.saturating_add(1);
            } else {
                new.push((song, lists));
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
        send_libraries(self.context)?;
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

    fn offer(&mut self, offer: Offer) -> impl Future<Output = Result<bool, TaskError>> + Send {
        let sent = if offer == Offer::Libraries {
            send_libraries(self.context)
        } else {
            Ok(())
        };
        std::future::ready(sent.map(|()| false))
    }

    async fn answer(&mut self, answers: Answers) -> Result<Answered, TaskError> {
        let Answers::SyncSpotify { lists, recipient } = answers else {
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
        self.note_keychain();
        self.chosen = lists;
        self.recipient = recipient;
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
        let library = self.recipient.make(&context.config)?;
        context.remember_friend(&library)?;
        let intake = Intake::new(context);
        // Plain loops, not closures: a closure over a borrowed item makes the future unprovable
        // as `Send` for every lifetime (rustc), and the runner spawns it.
        let mut pending = Vec::with_capacity(total);
        for (song, from) in &plan.new {
            pending.push(intake.take(&library, song, from, Route::Search));
        }
        let outcomes = stream::iter(pending).buffer_unordered(context.config.download_jobs.get());
        let tally = drive(outcomes, total, &context.events).await?;
        self.note_keychain();
        context
            .events
            .send(EventKind::Headline("Writing playlist files".to_owned()));
        for list in self
            .chosen
            .iter()
            .filter_map(|list| self.listed.get(&list.id))
        {
            playlists::write(&library, &list.list.name, &list.list.id, &list.songs)?;
        }
        let line = summary_line(&with_already(tally, plan.already));
        Ok(Flow::done(line, &tally.counts()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sdl_core::{Format, LibraryId, Never, Place, SongSource};

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
        let lib = Library::at("Me", root.path(), true);
        let place = Place::of(&lib, &here);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::M4a), b"x")?;
        let mut review = ReviewList::default();
        review.hold(sdl_core::HeldSong {
            library: LibraryId::Own,
            song: song("held", "Held"),
            from: Vec::new(),
            from_files: Vec::new(),
            candidates: Vec::new(),
            reason: "no result".into(),
        });
        let state = State {
            never: [Never {
                library: LibraryId::Own,
                key: "never".into(),
            }]
            .into(),
            ..State::default()
        };
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
        let plan = Plan::make(&lib, &review, &state, &[&liked, &road])?;
        assert_eq!((plan.total, plan.waiting, plan.already), (5, 1, 1));
        let new: Vec<(&str, Vec<&str>)> = plan
            .new
            .iter()
            .map(|(song, lists)| (song.key(), lists.iter().map(|l| l.name.as_str()).collect()))
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

    #[test]
    fn the_new_count_is_for_the_chosen_library() -> TestResult {
        let root = tempfile::tempdir()?;
        let mine = Library::at("Me", &root.path().join("Me"), true);
        let sam = Library::at("Sam", &root.path().join("Sam"), false);
        let here = song("here", "Here");
        let place = Place::of(&mine, &here);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::Opus), b"x")?;
        let liked = list("liked-songs", "Liked Songs", vec![here, song("b", "B")]);
        let count = |library: &Library| -> Result<(usize, usize), std::io::Error> {
            let plan = Plan::make(
                library,
                &ReviewList::default(),
                &State::default(),
                &[&liked],
            )?;
            Ok((plan.new.len(), plan.already))
        };
        assert_eq!(count(&mine)?, (1, 1));
        assert_eq!(count(&sam)?, (2, 0), "Sam has neither song");
        Ok(())
    }

    #[test]
    fn a_keychain_failure_during_the_run_is_noted_once_and_does_not_stop_it() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, mut rx) = crate::tasks::intake::tests::runner(dir.path());
        let context = runner.context(sdl_core::TaskId::SyncSpotify);
        let mut task = SyncSpotify::new(&context);
        task.note_late_keychain(None);
        task.note_late_keychain(Some("cannot save".into()));
        task.note_late_keychain(Some("cannot save again".into()));
        let mut notes = Vec::new();
        while let Ok(event) = rx.try_recv() {
            notes.push(event.kind);
        }
        assert_eq!(
            notes,
            [EventKind::Attention(Attention::failed(
                "Spotify sign-in".into(),
                String::new(),
                "cannot save".into()
            ))]
        );
        Ok(())
    }
}
