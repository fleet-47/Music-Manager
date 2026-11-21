//! The Review task (F8). The conversation is documented on [`crate::Runner`].
//!
//! The held songs are read from the runner. The form's one answer ("Download N chosen") starts the
//! run: "never" choices are settled first (they need no download), then each chosen song goes
//! through intake from its download step and leaves the review list the moment it lands.

use futures::StreamExt;
use futures::stream;
use sdl_core::{Attention, EventKind, HeldSong, Place};

use crate::conversation::{Answered, Answers, Opened, Resolution, ReviewChoice, Step};
use crate::progress::{Outcome, drive, summary_line};
use crate::tasks::intake::{Intake, Route};
use crate::{Flow, TaskContext, TaskError};
use sdl_core::words::thousands;

/// The Review task.
pub(crate) struct Review<'a> {
    context: &'a TaskContext,
    choices: Vec<ReviewChoice>,
}

impl<'a> Review<'a> {
    pub(crate) const fn new(context: &'a TaskContext) -> Self {
        Self {
            context,
            choices: Vec::new(),
        }
    }
}

/// Where the audio of a decided song comes from, or why the choice cannot be carried out.
/// `Later` and `Never` download nothing and are settled by the caller.
fn route(held: &HeldSong, resolution: &Resolution) -> Result<Route, String> {
    match resolution {
        Resolution::Candidate(video_id) => held
            .candidates
            .iter()
            .find(|candidate| &candidate.video_id == video_id)
            .map(|candidate| Route::Download(candidate.url()))
            .ok_or_else(|| "That match is no longer in the list. Choose again.".to_owned()),
        Resolution::OwnAudio => match &held.song.source {
            sdl_core::SongSource::Link { item_url, .. } => Ok(Route::OwnAudio(item_url.clone())),
            sdl_core::SongSource::Spotify { .. } => {
                Err("Only a song that came from a link has its own audio.".to_owned())
            }
        },
        Resolution::PastedLink(url) if url.trim().is_empty() => {
            Err("No link was pasted. Paste one, or choose something else.".to_owned())
        }
        Resolution::PastedLink(url) => Ok(Route::Download(url.trim().to_owned())),
        Resolution::Later | Resolution::Never => {
            Err("Nothing to download for this choice.".to_owned())
        }
    }
}

/// Take one decided song, and take it off the list when it landed (or was there already). A song
/// that failed, or whose choice cannot be carried out, stays held.
async fn settle(
    context: &TaskContext,
    intake: &Intake<'_>,
    held: &HeldSong,
    route: &Result<Route, String>,
) -> Result<Outcome, TaskError> {
    let outcome = match route {
        Ok(route) => intake.take(&held.song, &held.from, route.clone()).await?,
        Err(reason) => Outcome::Failed(Attention::failed(
            held.song.title.clone(),
            held.song.artists_joined(),
            reason.clone(),
        )),
    };
    // A skip leaves the list only when the song really is in the library: a second held song with
    // the same place is skipped in this run, but stays held if the first one failed.
    let landed = match outcome {
        Outcome::Added | Outcome::AddedMissing { .. } => true,
        Outcome::Skipped => Place::of(&context.config.music_root, &held.song)
            .present()?
            .is_some(),
        Outcome::Held(_) | Outcome::Failed(_) => false,
    };
    if landed {
        context.update_review(|review| {
            review.resolve(held.song.key());
        })?;
    }
    Ok(outcome)
}

impl Step for Review<'_> {
    fn open(&mut self) -> impl Future<Output = Result<Opened, TaskError>> + Send {
        let held = self.context.review().len();
        self.context.events.send(EventKind::Headline(format!(
            "{} held {}: choose what to do with each",
            thousands(held),
            if held == 1 { "song" } else { "songs" }
        )));
        std::future::ready(Ok(Opened::Ready))
    }

    fn answer(
        &mut self,
        answers: Answers,
    ) -> impl Future<Output = Result<Answered, TaskError>> + Send {
        let Answers::Review { choices } = answers else {
            return std::future::ready(Ok(Answered::Ignored));
        };
        self.choices = choices;
        std::future::ready(Ok(Answered::Run))
    }

    async fn go(&mut self) -> Result<Flow, TaskError> {
        let context = self.context;
        let held = context.review();
        let mut jobs: Vec<(HeldSong, Result<Route, String>)> = Vec::new();
        for choice in std::mem::take(&mut self.choices) {
            // A song that was resolved meanwhile is no longer in the list; nothing to do.
            let Some(song) = held.songs().iter().find(|h| h.song.key() == choice.key) else {
                continue;
            };
            match &choice.resolution {
                Resolution::Later => {}
                Resolution::Never => {
                    context.mark_never(choice.key.clone())?;
                    context.update_review(|review| {
                        review.resolve(&choice.key);
                    })?;
                }
                other => jobs.push((song.clone(), route(song, other))),
            }
        }
        let total = jobs.len();
        let noun = if total == 1 { "song" } else { "songs" };
        context.events.send(EventKind::Headline(format!(
            "Downloading {} chosen {noun}",
            thousands(total)
        )));
        let intake = Intake::new(context);
        // Plain loops, not closures: see the note in the Sync Spotify task.
        let mut pending = Vec::with_capacity(total);
        for (song, route) in &jobs {
            pending.push(settle(context, &intake, song, route));
        }
        let outcomes = stream::iter(pending).buffer_unordered(context.config.download_jobs.get());
        let tally = drive(outcomes, total, &context.events).await?;
        Ok(Flow::done(summary_line(&tally), &tally.counts()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sdl_core::{
        Candidate, Event, EventReceiver, Format, HeldSong, Place, RunOutcome, SongSource, TaskId,
        WantedSong,
    };

    use super::*;
    use crate::tasks::intake::tests::runner;
    use crate::{Command, Runner};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn held(key: &str, link: bool) -> HeldSong {
        HeldSong {
            song: WantedSong {
                source: if link {
                    SongSource::Link {
                        item_url: format!("https://example.test/{key}"),
                        video_id: key.into(),
                        direct: false,
                    }
                } else {
                    SongSource::Spotify {
                        track_id: key.into(),
                    }
                },
                title: format!("Song {key}"),
                artists: vec!["Band".into()],
                album: Some("Record".into()),
                album_artist: None,
                track_number: None,
                disc_number: None,
                date: None,
                isrc: None,
                duration: Some(Duration::from_secs(200)),
                cover_url: None,
            },
            from: vec!["Liked Songs".into()],
            candidates: vec![Candidate {
                video_id: "vid".into(),
                title: "Song".into(),
                artists: vec!["Band".into()],
                album: None,
                duration: None,
            }],
            reason: "no result passed".into(),
        }
    }

    fn choice(key: &str, resolution: Resolution) -> Command {
        Command::Answer(Answers::Review {
            choices: vec![ReviewChoice {
                key: key.into(),
                resolution,
            }],
        })
    }

    async fn until_ended(rx: &mut EventReceiver) -> Result<Vec<Event>, &'static str> {
        let mut seen = Vec::new();
        loop {
            let event = rx.recv().await.ok_or("the events ended early")?;
            let end = matches!(
                event.kind,
                EventKind::Finished | EventKind::Stopped | EventKind::Error(_)
            );
            seen.push(event);
            if end {
                return Ok(seen);
            }
        }
    }

    fn start_with_held(
        dir: &std::path::Path,
        songs: Vec<HeldSong>,
    ) -> Result<(Runner, EventReceiver), Box<dyn std::error::Error>> {
        let (mut runner, rx) = runner(dir);
        for song in songs {
            runner.context(TaskId::Review).hold(song)?;
        }
        runner.start(TaskId::Review)?;
        Ok((runner, rx))
    }

    #[test]
    fn each_choice_has_its_route_or_a_plain_reason() {
        let from_link = held("a", true);
        let from_spotify = held("b", false);
        assert_eq!(
            route(&from_spotify, &Resolution::Candidate("vid".into())),
            Ok(Route::Download(
                "https://music.youtube.com/watch?v=vid".into()
            ))
        );
        assert!(route(&from_spotify, &Resolution::Candidate("gone".into())).is_err());
        assert_eq!(
            route(&from_link, &Resolution::OwnAudio),
            Ok(Route::OwnAudio("https://example.test/a".into()))
        );
        assert!(route(&from_spotify, &Resolution::OwnAudio).is_err());
        assert_eq!(
            route(
                &from_spotify,
                &Resolution::PastedLink(" https://x.test/y ".into())
            ),
            Ok(Route::Download("https://x.test/y".into()))
        );
        assert!(route(&from_spotify, &Resolution::PastedLink("  ".into())).is_err());
    }

    #[tokio::test]
    async fn never_and_later_need_no_download_and_the_state_is_saved() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, mut rx) =
            start_with_held(dir.path(), vec![held("n", false), held("l", false)])?;
        assert!(runner.send(TaskId::Review, choice("n", Resolution::Never)));
        assert!(runner.send(TaskId::Review, choice("l", Resolution::Later)));
        let seen = until_ended(&mut rx).await?;
        assert_eq!(seen.last().map(|e| &e.kind), Some(&EventKind::Finished));
        assert!(seen.iter().any(|e| e.kind == EventKind::Started));
        let review = runner.review();
        assert!(!review.contains("n"), "never leaves the list");
        assert!(review.contains("l"), "later stays");
        assert!(runner.state().never.contains("n"));
        let run = runner.state().last_run.get(&TaskId::Review).cloned();
        assert_eq!(
            run.map(|r| (r.line, r.outcome)),
            Some(("0 added".to_owned(), RunOutcome::Done))
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_song_landed_meanwhile_leaves_the_list_as_skipped() -> TestResult {
        let dir = tempfile::tempdir()?;
        let landed = held("s", false);
        let place = Place::of(&dir.path().join("music"), &landed.song);
        std::fs::create_dir_all(place.dir())?;
        std::fs::write(place.path(Format::Opus), b"x")?;
        let (runner, mut rx) = start_with_held(dir.path(), vec![landed])?;
        assert!(runner.send(
            TaskId::Review,
            choice("s", Resolution::Candidate("vid".into()))
        ));
        let seen = until_ended(&mut rx).await?;
        assert_eq!(seen.last().map(|e| &e.kind), Some(&EventKind::Finished));
        assert!(runner.review().is_empty());
        let line = runner
            .state()
            .last_run
            .get(&TaskId::Review)
            .map(|r| r.line.clone());
        assert_eq!(line.as_deref(), Some("0 added, 1 already there"));
        Ok(())
    }

    #[tokio::test]
    async fn a_choice_that_cannot_be_carried_out_fails_the_item_and_keeps_it_held() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (runner, mut rx) = start_with_held(dir.path(), vec![held("s", false)])?;
        assert!(runner.send(TaskId::Review, choice("s", Resolution::OwnAudio)));
        let seen = until_ended(&mut rx).await?;
        assert!(seen.iter().any(
            |e| matches!(&e.kind, EventKind::Attention(a) if a.reason.contains("came from a link"))
        ));
        assert!(runner.review().contains("s"));
        let outcome = runner
            .state()
            .last_run
            .get(&TaskId::Review)
            .map(|r| r.outcome);
        assert_eq!(outcome, Some(RunOutcome::NeedsAttention));
        Ok(())
    }
}
