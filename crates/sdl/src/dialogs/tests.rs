//! The dialogs driven through keys, mouse events and task events; nothing here touches the
//! network, the runner or the terminal.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{
    KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use sdl_core::{
    Candidate, ConvertChoice, Device, DeviceChoice, Direction, EventKind, Format, HeldSong,
    Library, LibraryId, ListChoice, PlanSummary, ReviewList, Selection, SongSource, State, TaskId,
    WantedSong,
};
use sdl_form::{Focus, FormStyle, Target, Value, render};
use sdl_tasks::{Answers, Command, Offer, Resolution, ReviewChoice};

use super::{Dialog, Step};
use crate::app::{Cmd, Settings};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn code(code: crossterm::event::KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn ch(c: char) -> KeyEvent {
    code(crossterm::event::KeyCode::Char(c))
}

fn commands(step: Step) -> Vec<Cmd> {
    match step {
        Step::Stay(commands) | Step::Close(commands) => commands,
    }
}

fn is_closed(step: &Step) -> bool {
    matches!(step, Step::Close(_))
}

/// A tester: one dialog with the settings it was opened with.
struct Rig {
    dialog: Dialog,
    settings: Settings,
}

impl Rig {
    fn open(task: TaskId, settings: Settings) -> (Self, Vec<Cmd>) {
        let (dialog, opened) = Dialog::open(task, &settings);
        (Self { dialog, settings }, opened)
    }

    fn event(&mut self, kind: &EventKind) -> Step {
        self.dialog.on_event(kind, &self.settings)
    }

    fn press(&mut self, key: KeyEvent) -> Step {
        self.dialog.key(key, &self.settings)
    }

    fn enter(&mut self) -> Vec<Cmd> {
        commands(self.press(code(crossterm::event::KeyCode::Enter)))
    }

    /// Tab until the keyboard is on the field `key`.
    fn focus(&mut self, key: &'static str) {
        for _ in 0..40 {
            if *self.dialog.form().focus() == Focus::Field(key) {
                return;
            }
            let _ = self.press(code(crossterm::event::KeyCode::Tab));
        }
    }

    /// Tab until the keyboard is on the primary button.
    fn focus_primary(&mut self) {
        for _ in 0..40 {
            if *self.dialog.form().focus() == Focus::Primary {
                return;
            }
            let _ = self.press(code(crossterm::event::KeyCode::Tab));
        }
    }

    fn hits(&self) -> sdl_form::HitMap<Target> {
        let area = Rect::new(0, 0, 100, 60);
        let mut buf = Buffer::empty(area);
        render(self.dialog.form(), area, &mut buf, &FormStyle::default())
    }

    /// A left click in the middle of what was drawn for `target`.
    fn click(&mut self, target: &Target) -> Result<Step, Box<dyn std::error::Error>> {
        let hits = self.hits();
        let area = hits
            .area_of(target)
            .ok_or_else(|| format!("{target:?} was not drawn"))?;
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x.saturating_add(area.width / 2),
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        Ok(self.dialog.mouse(event, &hits, &self.settings))
    }

    fn busy(&self) -> Option<&str> {
        self.dialog.form().busy()
    }

    fn ticked(&self, key: &'static str) -> Vec<String> {
        match self.dialog.form().value(key) {
            Some(Value::Ticked(ids)) => ids,
            _ => Vec::new(),
        }
    }
}

/// The libraries the task lists: the owner's own, `Me`, then `others`.
fn libraries(others: &[&str]) -> EventKind {
    let mut all = vec![Library::at("Me", &PathBuf::from("/music/Me"), true)];
    all.extend(
        others
            .iter()
            .map(|name| Library::at(name, &PathBuf::from("/music").join(name), false)),
    );
    EventKind::Libraries(all)
}

fn send(task: TaskId, command: Command) -> Cmd {
    Cmd::send(task, command)
}

fn song(key: &str, title: &str, link: bool) -> WantedSong {
    WantedSong {
        source: if link {
            SongSource::Link {
                item_url: key.into(),
                video_id: "v".into(),
                direct: false,
            }
        } else {
            SongSource::Spotify {
                track_id: key.into(),
            }
        },
        title: title.into(),
        artists: vec!["Artist".into()],
        album: Some("Album".into()),
        album_artist: None,
        track_number: None,
        disc_number: None,
        date: None,
        isrc: None,
        duration: Some(Duration::from_secs(221)),
        cover_url: None,
    }
}

fn held(key: &str, title: &str, link: bool) -> HeldSong {
    HeldSong {
        library: sdl_core::LibraryId::Own,
        song: song(key, title, link),
        from: vec!["Liked Songs".into()],
        from_files: Vec::new(),
        candidates: vec![
            Candidate {
                video_id: "same".into(),
                title: title.into(),
                artists: vec!["Artist".into()],
                album: Some("Album".into()),
                duration: Some(Duration::from_secs(222)),
            },
            Candidate {
                video_id: "other".into(),
                title: format!("{title} (live)"),
                artists: vec!["Someone else".into()],
                album: Some("Live at home".into()),
                duration: Some(Duration::from_secs(300)),
            },
        ],
        reason: "No result matched well enough.".into(),
    }
}

pub(crate) fn review_settings() -> Settings {
    let mut review = ReviewList::default();
    review.hold(held("t1", "First", false));
    review.hold(held("https://x/y", "Second", true));
    Settings {
        review,
        ..Settings::default()
    }
}

fn device(name: &str, uuid: &str, free: u64) -> Device {
    Device {
        name: name.into(),
        mount_point: PathBuf::from(format!("/Volumes/{name}")),
        volume_uuid: Some(uuid.into()),
        free_bytes: free,
        disk_image: false,
    }
}

fn plan(files: usize, bytes: u64, free: Option<u64>) -> PlanSummary {
    PlanSummary {
        files,
        bytes,
        estimated: false,
        already: 3,
        free_bytes: free,
        refusal: None,
    }
}

// ----- Sync Spotify -----

fn lists() -> Vec<ListChoice> {
    vec![
        ListChoice {
            name: "Liked Songs".into(),
            id: "liked-songs".into(),
            count: 2117,
        },
        ListChoice {
            name: "Road trip".into(),
            id: "p1".into(),
            count: 40,
        },
    ]
}

fn spotify_settings() -> Settings {
    Settings {
        state: State {
            ticked_lists: vec!["liked-songs".into()],
            ..State::default()
        },
        ..Settings::default()
    }
}

#[test]
fn sync_spotify_walks_from_the_lists_to_start() {
    let (mut rig, opened) = Rig::open(TaskId::SyncSpotify, spotify_settings());
    assert_eq!(opened, vec![Cmd::Start(TaskId::SyncSpotify)]);
    assert_eq!(rig.busy(), Some("Signing in to Spotify"));
    assert!(
        rig.event(&EventKind::Lists(lists()))
            .eq(&Step::Stay(Vec::new()))
    );
    assert_eq!(rig.busy(), None);
    assert_eq!(rig.ticked("lists"), vec!["liked-songs".to_owned()]);
    assert_eq!(rig.dialog.form().primary(), "Continue");
    // Continue sends the ticked lists, and the form says what it waits for.
    let sent = rig.enter();
    assert_eq!(
        sent,
        vec![send(
            TaskId::SyncSpotify,
            Command::Answer(Answers::SyncSpotify {
                lists: vec![lists().remove(0)],
                recipient: sdl_tasks::Recipient::Me,
            })
        )]
    );
    assert_eq!(rig.busy(), Some("Listing your songs"));
    let _ = rig.event(&EventKind::Headline("Reading Liked Songs".into()));
    assert_eq!(rig.busy(), Some("Reading Liked Songs"));
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 412,
        total: 2117,
        waiting: 3,
        unreadable: 0,
    });
    assert_eq!(rig.busy(), None);
    assert_eq!(
        rig.dialog.form().summary(),
        Some("412 new of 2,117 · 3 waiting in Review")
    );
    assert_eq!(rig.dialog.form().primary(), "Start");
    // Start sends Go; the dialog closes on Started.
    assert_eq!(rig.enter(), vec![send(TaskId::SyncSpotify, Command::Go)]);
    assert_eq!(rig.busy(), Some("Starting"));
    assert!(is_closed(&rig.event(&EventKind::Started)));
}

#[test]
fn a_run_with_nothing_new_can_still_start() {
    let (mut rig, _) = Rig::open(TaskId::SyncSpotify, spotify_settings());
    let _ = rig.event(&EventKind::Lists(lists()));
    let _ = rig.enter();
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 0,
        total: 5,
        waiting: 0,
        unreadable: 0,
    });
    assert_eq!(rig.dialog.form().primary(), "Start");
    assert_eq!(rig.enter(), vec![send(TaskId::SyncSpotify, Command::Go)]);
}

#[test]
fn changing_the_ticks_after_the_summary_goes_back_to_continue() {
    let (mut rig, _) = Rig::open(TaskId::SyncSpotify, spotify_settings());
    let _ = rig.event(&EventKind::Lists(lists()));
    let _ = rig.enter();
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 1,
        total: 1,
        waiting: 0,
        unreadable: 0,
    });
    rig.focus("lists");
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let _ = rig.press(ch(' '));
    assert_eq!(rig.dialog.form().summary(), None);
    assert_eq!(rig.dialog.form().primary(), "Continue");
    assert_eq!(rig.ticked("lists").len(), 2);
}

#[test]
fn a_plan_that_arrives_after_a_change_is_not_shown() {
    let (mut rig, _) = Rig::open(TaskId::SyncSpotify, spotify_settings());
    let _ = rig.event(&EventKind::Lists(lists()));
    let _ = rig.enter();
    // The owner ticks another list while the first plan is still being made.
    rig.focus("lists");
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let _ = rig.press(ch(' '));
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 1,
        total: 1,
        waiting: 0,
        unreadable: 0,
    });
    assert_eq!(rig.dialog.form().summary(), None);
    assert_eq!(rig.dialog.form().primary(), "Continue");
    assert_eq!(rig.busy(), None);
}

#[test]
fn sync_spotify_cannot_continue_with_nothing_ticked_and_cancel_tells_the_task() {
    let (mut rig, _) = Rig::open(TaskId::SyncSpotify, Settings::default());
    let _ = rig.event(&EventKind::Lists(lists()));
    assert_eq!(rig.enter(), Vec::new());
    assert_eq!(
        rig.dialog.form().missing(),
        vec!["Tick at least one list".to_owned()]
    );
    let step = rig.press(code(crossterm::event::KeyCode::Esc));
    assert!(is_closed(&step));
    assert_eq!(
        commands(step),
        vec![send(TaskId::SyncSpotify, Command::Cancel)]
    );
}

#[test]
fn sync_spotify_continues_and_starts_with_the_mouse() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::SyncSpotify, spotify_settings());
    let _ = rig.event(&EventKind::Lists(lists()));
    let sent = commands(rig.click(&Target::Primary)?);
    assert!(matches!(
        sent.as_slice(),
        [Cmd::Send(TaskId::SyncSpotify, command)] if matches!(**command, Command::Answer(_))
    ));
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 2,
        total: 2,
        waiting: 0,
        unreadable: 0,
    });
    let sent = commands(rig.click(&Target::Primary)?);
    assert_eq!(sent, vec![send(TaskId::SyncSpotify, Command::Go)]);
    Ok(())
}

// ----- Download link -----

#[test]
fn download_link_starts_with_a_pasted_link() {
    let (mut rig, _) = Rig::open(TaskId::DownloadLink, Settings::default());
    assert_eq!(rig.busy(), None);
    assert_eq!(rig.dialog.form().primary(), "Start");
    assert!(rig.enter().is_empty(), "no link yet");
    rig.focus("link");
    let _ = rig.dialog.paste("not a link", &rig.settings);
    assert_eq!(rig.enter(), Vec::new());
    assert!(
        rig.dialog
            .form()
            .missing()
            .iter()
            .any(|line| line == "Fix the link above")
    );
    // Clear the field (Ctrl-U) and paste a link, with a stray space and a line break.
    let _ = rig.press(KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..ch('u')
    });
    let _ = rig
        .dialog
        .paste("https://music.youtube.com/watch?v=abc\n", &rig.settings);
    let sent = rig.enter();
    assert_eq!(
        sent,
        vec![send(
            TaskId::DownloadLink,
            Command::Answer(Answers::DownloadLink {
                urls: vec!["https://music.youtube.com/watch?v=abc".into()],
                recipient: sdl_tasks::Recipient::Me,
            })
        )]
    );
    assert_eq!(rig.busy(), Some("Starting"));
    let _ = rig.event(&EventKind::Headline("Reading the link".into()));
    assert_eq!(rig.busy(), Some("Reading the link"));
    assert!(is_closed(&rig.event(&EventKind::Started)));
}

// ----- Review -----

#[test]
fn review_lists_the_held_songs_and_blocks_until_one_is_chosen() {
    let (rig, opened) = Rig::open(TaskId::Review, review_settings());
    assert_eq!(opened, vec![Cmd::Start(TaskId::Review)]);
    assert_eq!(rig.busy(), None);
    assert_eq!(rig.dialog.form().primary(), "Download 0 chosen");
    assert_eq!(
        rig.dialog.form().missing(),
        vec!["Open a song and choose what to do with it".to_owned()]
    );
}

#[test]
fn review_walk_from_the_list_to_download() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    // Enter opens the first song's detail.
    assert_eq!(rig.enter(), Vec::new());
    assert_eq!(rig.dialog.form().title(), "Held song");
    assert!(rig.dialog.form().notes()[0].starts_with("Wanted: First"));
    assert_eq!(rig.dialog.form().cancel(), "Back");
    assert!(
        rig.dialog
            .form()
            .notes()
            .iter()
            .any(|n| n.contains("First"))
    );
    // Nothing chosen yet; Down chooses the first candidate; `o` opens it in the browser.
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    assert_eq!(
        rig.dialog.form().value("choice"),
        Some(Value::Choice(Some("same".into())))
    );
    let step = rig.press(ch('o'));
    assert_eq!(
        commands(step),
        vec![Cmd::Open("https://music.youtube.com/watch?v=same".into())]
    );
    assert!(rig.dialog.hints().iter().any(|hint| hint.key == "o"));
    // Done records it and shows the list again, with the choice in words.
    rig.focus_primary();
    assert_eq!(rig.enter(), Vec::new());
    assert_eq!(rig.dialog.form().title(), "Review");
    assert_eq!(rig.dialog.form().primary(), "Download 1 chosen");
    assert_eq!(rig.dialog.form().missing(), Vec::<String>::new());
    // Start sends the decided songs and says it is starting.
    rig.focus_primary();
    let sent = rig.enter();
    assert_eq!(
        sent,
        vec![send(
            TaskId::Review,
            Command::Answer(Answers::Review {
                choices: vec![ReviewChoice {
                    library: sdl_core::LibraryId::Own,
                    key: "t1".into(),
                    resolution: Resolution::Candidate("same".into()),
                }]
            })
        )]
    );
    assert_eq!(rig.busy(), Some("Starting"));
    assert!(is_closed(&rig.event(&EventKind::Started)));
}

#[test]
fn review_flags_only_what_differs_from_the_wanted_song() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    let _ = rig.enter();
    // Draw with a flagged colour that is distinct, and read back what was drawn in it.
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);
    let style = FormStyle {
        flagged: ratatui::style::Style::new().fg(ratatui::style::Color::Yellow),
        ..FormStyle::default()
    };
    let _ = render(rig.dialog.form(), area, &mut buf, &style);
    let yellow: String = area
        .rows()
        .flat_map(Rect::columns)
        .filter_map(|position| buf.cell((position.x, position.y)))
        .filter(|cell| cell.fg == ratatui::style::Color::Yellow)
        .map(|cell| cell.symbol().to_owned())
        .collect();
    assert!(yellow.contains("live"), "the other title differs: {yellow}");
    assert!(yellow.contains("Someone"), "the other artist differs");
    assert!(yellow.contains("Live"), "the other album differs");
    assert!(yellow.contains("5:00"), "the other length differs");
    assert!(
        !yellow.contains("Artist"),
        "the matching artist is not flagged"
    );
    assert!(
        !yellow.contains("Album"),
        "the matching album is not flagged"
    );
    assert!(
        !yellow.contains("3:42"),
        "a length within 3 s is not flagged"
    );
}

#[test]
fn escape_in_the_detail_goes_back_and_keeps_nothing() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    let _ = rig.enter();
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let step = rig.press(code(crossterm::event::KeyCode::Esc));
    assert!(!is_closed(&step));
    assert!(commands(step).is_empty(), "nothing is sent for going back");
    assert_eq!(rig.dialog.form().title(), "Review");
    assert_eq!(rig.dialog.form().primary(), "Download 0 chosen");
    // Esc on the list cancels the run.
    let step = rig.press(code(crossterm::event::KeyCode::Esc));
    assert!(is_closed(&step));
    assert_eq!(commands(step), vec![send(TaskId::Review, Command::Cancel)]);
}

#[test]
fn review_offers_the_links_own_audio_only_for_link_songs_and_a_pasted_link_asks_for_one()
-> TestResult {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    // A click on the second row opens that song (a link item).
    let step = rig.click(&Target::Row {
        field: "songs",
        index: 1,
    })?;
    assert!(!is_closed(&step));
    assert_eq!(rig.dialog.form().title(), "Held song");
    assert!(rig.dialog.form().notes()[0].starts_with("Wanted: Second"));
    let ids = |rig: &Rig| -> Vec<&'static str> {
        ["own-audio", "paste", "later", "never"]
            .into_iter()
            .filter(|id| {
                rig.hits()
                    .area_of(&Target::Row {
                        field: "choice",
                        index: match *id {
                            "own-audio" => 2,
                            "paste" => 3,
                            "later" => 4,
                            _ => 5,
                        },
                    })
                    .is_some()
            })
            .collect()
    };
    assert_eq!(ids(&rig).len(), 4, "own audio, paste, later and never");
    // Paste a link (row 3): the link field appears and Done waits for a valid link.
    let _ = rig.click(&Target::Row {
        field: "choice",
        index: 3,
    })?;
    assert!(
        rig.dialog
            .form()
            .missing()
            .contains(&"Paste a link".to_owned())
    );
    let _ = rig.dialog.paste("https://youtu.be/abc", &rig.settings);
    // The paste goes to the focused field, which is the radio group: focus the link field.
    let _ = rig.click(&Target::Field("pasted"))?;
    let _ = rig.dialog.paste("https://youtu.be/abc", &rig.settings);
    assert_eq!(rig.dialog.form().missing(), Vec::<String>::new());
    rig.focus_primary();
    let _ = rig.enter();
    assert_eq!(rig.dialog.form().primary(), "Download 1 chosen");
    rig.focus_primary();
    let sent = rig.enter();
    assert_eq!(
        sent,
        vec![send(
            TaskId::Review,
            Command::Answer(Answers::Review {
                choices: vec![ReviewChoice {
                    library: sdl_core::LibraryId::Own,
                    key: "https://x/y".into(),
                    resolution: Resolution::PastedLink("https://youtu.be/abc".into()),
                }]
            })
        )]
    );
    // A Spotify song's detail has no own-audio row: three fixed rows after its two candidates.
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    let _ = rig.enter();
    assert!(
        rig.hits()
            .area_of(&Target::Row {
                field: "choice",
                index: 5
            })
            .is_none(),
        "two candidates, paste, later and never make four rows"
    );
    Ok(())
}

#[test]
fn leaving_for_later_is_not_counted_and_never_is() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    // First song: Never download (the last of its four rows: two candidates, paste, later, never).
    let _ = rig.enter();
    for _ in 0..5 {
        let _ = rig.press(code(crossterm::event::KeyCode::Down));
    }
    assert_eq!(
        rig.dialog.form().value("choice"),
        Some(Value::Choice(Some("never".into())))
    );
    rig.focus_primary();
    let _ = rig.enter();
    assert_eq!(rig.dialog.form().primary(), "Download 1 chosen");
    // Second song: Leave for later.
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let _ = rig.enter();
    for _ in 0..5 {
        let _ = rig.press(code(crossterm::event::KeyCode::Down));
    }
    // Rows for a link song: two candidates, own audio, paste, later, never.
    assert_eq!(
        rig.dialog.form().value("choice"),
        Some(Value::Choice(Some("later".into())))
    );
    rig.focus_primary();
    let _ = rig.enter();
    assert_eq!(rig.dialog.form().primary(), "Download 1 chosen");
}

#[test]
fn a_song_that_leaves_the_list_while_open_drops_its_choice() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    let _ = rig.enter();
    assert_eq!(rig.dialog.form().title(), "Held song");
    assert!(rig.dialog.form().notes()[0].starts_with("Wanted: First"));
    // Another task landed the first song: the runner's list no longer holds it.
    let _ = rig.settings.review.resolve(&sdl_core::LibraryId::Own, "t1");
    let step = rig.event(&EventKind::HeldCount(1));
    assert!(!is_closed(&step));
    assert_eq!(rig.dialog.form().title(), "Review", "back to the list");
    assert_eq!(rig.dialog.form().primary(), "Download 0 chosen");
}

// ----- Sync device -----

fn device_settings(last: Option<&str>) -> Settings {
    let mut state = State::default();
    state.devices.insert(
        "U1".into(),
        DeviceChoice {
            direction: Direction::ToDevice,
            what: Selection {
                whole_library: true,
                playlists: Vec::new(),
            },
            from_folders: Vec::new(),
            format: Some(Format::M4a),
            quality: sdl_core::Quality::Small,
            keep_cover: false,
            ..DeviceChoice::default()
        },
    );
    state.last_device = last.map(str::to_owned);
    Settings {
        state,
        ..Settings::default()
    }
}

/// The command sent to `task`, when `cmd` is one.
fn sent_to(cmd: &Cmd, task: TaskId) -> Option<&Command> {
    match cmd {
        Cmd::Send(to, command) if *to == task => Some(command),
        _ => None,
    }
}

fn answer_of(commands: &[Cmd]) -> Option<(&Device, &DeviceChoice)> {
    commands
        .iter()
        .find_map(|cmd| match sent_to(cmd, TaskId::SyncDevice)? {
            Command::Answer(Answers::SyncDevice { device, choice }) => Some((device, choice)),
            _ => None,
        })
}

#[test]
fn sync_device_follows_the_devices_and_the_remembered_choices() -> TestResult {
    let (mut rig, opened) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    assert_eq!(opened, vec![Cmd::Start(TaskId::SyncDevice)]);
    assert_eq!(rig.busy(), Some("Looking for devices"));
    assert!(
        rig.dialog
            .form()
            .notes()
            .iter()
            .any(|n| n.contains(".m3u8"))
    );
    // The devices arrive first, then the playlists and the libraries: one plan, asked for once
    // all are in.
    let step = rig.event(&EventKind::Devices(vec![
        device("PHILIPS", "U2", 5_000_000_000),
        device("SNOWSKY", "U1", 12_300_000_000),
    ]));
    assert!(commands(step).is_empty(), "the playlists are still to come");
    assert_eq!(rig.busy(), Some("Looking for devices"));
    let step = rig.event(&EventKind::Playlists(vec!["Road trip".into()]));
    assert!(commands(step).is_empty(), "the libraries are still to come");
    let step = rig.event(&libraries(&[]));
    let sent = commands(step);
    let (chosen, choice) = answer_of(&sent).ok_or("an answer")?;
    assert_eq!(chosen.id(), "U1", "the last device is preselected");
    assert_eq!(choice.format, Some(Format::M4a));
    assert_eq!(choice.quality, sdl_core::Quality::Small);
    assert!(choice.what.whole_library && !choice.keep_cover);
    assert_eq!(rig.busy(), Some("Making the plan"));
    // The plan line, then Start.
    let _ = rig.event(&EventKind::Plan(PlanSummary {
        estimated: true,
        ..plan(412, 3_200_000_000, Some(12_300_000_000))
    }));
    assert_eq!(
        rig.dialog.form().summary(),
        Some("412 files, about 3.2 GB to copy · 3 already there · 12.3 GB free on SNOWSKY")
    );
    assert_eq!(rig.dialog.form().primary(), "Start");
    rig.focus_primary();
    assert_eq!(rig.enter(), vec![send(TaskId::SyncDevice, Command::Go)]);
    assert!(is_closed(&rig.event(&EventKind::Started)));
    Ok(())
}

#[test]
fn a_device_that_goes_as_start_is_pressed_leaves_the_form_answerable() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device(
        "SNOWSKY",
        "U1",
        12_300_000_000,
    )]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Plan(plan(1, 1, Some(12_300_000_000))));
    rig.focus_primary();
    assert_eq!(rig.enter(), vec![send(TaskId::SyncDevice, Command::Go)]);
    assert_eq!(rig.busy(), Some("Starting"));
    // The device goes before the run starts, so the task does not start it: the form must not
    // wait for a `Started` that never comes.
    let _ = rig.event(&EventKind::Devices(Vec::new()));
    assert_eq!(rig.busy(), None);
    let step = rig.press(code(crossterm::event::KeyCode::Esc));
    assert!(is_closed(&step));
    assert_eq!(
        commands(step),
        vec![send(TaskId::SyncDevice, Command::Cancel)]
    );
}

#[test]
fn a_mounted_disk_image_is_listed_with_its_label() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(None));
    let image = Device {
        disk_image: true,
        ..device("SDLTEST", "U3", 40_000_000)
    };
    let _ = rig.event(&EventKind::Devices(vec![device("SNOWSKY", "U1", 9), image]));
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);
    let _ = render(rig.dialog.form(), area, &mut buf, &FormStyle::default());
    let drawn = area
        .rows()
        .map(|row| {
            row.columns()
                .filter_map(|p| buf.cell((p.x, p.y)))
                .map(|cell| cell.symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(drawn.contains("SDLTEST (disk image)"), "{drawn}");
    assert!(!drawn.contains("SNOWSKY (disk image)"));
}

#[test]
fn the_device_list_updates_when_volumes_change() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(Vec::new()));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    assert_eq!(rig.busy(), None);
    assert!(
        rig.dialog
            .form()
            .missing()
            .contains(&"No device to choose yet".to_owned())
    );
    // A volume appears: it is offered; the remembered device is chosen when it comes.
    let _ = rig.event(&EventKind::Devices(vec![device("PHILIPS", "U2", 1)]));
    assert_eq!(
        rig.dialog.form().value("device"),
        Some(Value::Choice(None)),
        "a device nobody chose is not chosen for them"
    );
    let step = rig.event(&EventKind::Devices(vec![
        device("PHILIPS", "U2", 1),
        device("SNOWSKY", "U1", 9),
    ]));
    assert!(answer_of(&commands(step)).is_some());
    assert_eq!(
        rig.dialog.form().value("device"),
        Some(Value::Choice(Some("U1".into())))
    );
    // It goes: nothing is chosen and nothing is planned.
    let _ = rig.event(&EventKind::Devices(vec![device("PHILIPS", "U2", 1)]));
    assert_eq!(rig.dialog.form().value("device"), Some(Value::Choice(None)));
    assert_eq!(rig.dialog.form().summary(), None);
}

#[test]
fn the_only_device_is_chosen_when_none_was_synced_before() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(None));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Devices(vec![device("SDLTEST", "U9", 1)]));
    assert_eq!(
        rig.dialog.form().value("device"),
        Some(Value::Choice(Some("U9".into())))
    );
    // Two devices and nothing remembered: the owner picks.
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(None));
    let _ = rig.event(&EventKind::Devices(vec![
        device("A", "U8", 1),
        device("B", "U9", 1),
    ]));
    assert_eq!(rig.dialog.form().value("device"), Some(Value::Choice(None)));
}

#[test]
fn not_enough_room_says_so_with_the_numbers_and_start_is_unavailable() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device(
        "SNOWSKY",
        "U1",
        1_100_000_000,
    )]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Plan(plan(
        10,
        3_200_000_000,
        Some(1_100_000_000),
    )));
    assert_eq!(
        rig.dialog.form().missing(),
        vec!["Not enough room on SNOWSKY: 3.2 GB to copy, 1.1 GB free".to_owned()]
    );
    assert!(rig.dialog.form().summary().is_some());
    assert!(rig.enter().is_empty(), "Start does nothing");
    // A plan's own refusal is said in its words.
    let _ = rig.event(&EventKind::Plan(PlanSummary {
        refusal: Some("The library has no songs to copy.".into()),
        ..plan(0, 0, Some(1_100_000_000))
    }));
    assert!(
        rig.dialog
            .form()
            .missing()
            .contains(&"The library has no songs to copy.".to_owned())
    );
}

#[test]
fn from_device_reads_the_folders_then_plans_and_the_format_rows_follow_the_format() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device("SNOWSKY", "U1", 9)]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let visible = |rig: &Rig, key: &str| {
        rig.dialog
            .form()
            .fields()
            .iter()
            .any(|field| field.key() == key && field.is_visible())
    };
    // Converting to M4A: quality and cover show; folders do not.
    assert!(visible(&rig, "quality") && visible(&rig, "cover") && !visible(&rig, "folders"));
    let _ = rig.event(&EventKind::Plan(plan(1, 1, Some(9))));
    // Choose From device in the direction group (the second option, by the mouse).
    let step = rig.click(&Target::Row {
        field: "direction",
        index: 1,
    })?;
    assert_eq!(
        commands(step),
        vec![send(
            TaskId::SyncDevice,
            Command::Offer(Offer::Folders(device("SNOWSKY", "U1", 9)))
        )]
    );
    assert_eq!(rig.busy(), Some("Reading the device's folders"));
    assert!(visible(&rig, "folders") && !visible(&rig, "what") && !visible(&rig, "quality"));
    // The folders arrive; nothing ticked yet, so no plan is asked for.
    let step = rig.event(&EventKind::Folders(vec!["Music".into(), "Podcasts".into()]));
    assert_eq!(commands(step), Vec::new());
    assert_eq!(rig.busy(), None);
    // Tick one: a plan is asked for at once.
    let _ = rig.press(code(crossterm::event::KeyCode::Tab));
    assert!(matches!(rig.dialog.form().focus(), Focus::Field("folders")));
    let step = rig.press(ch(' '));
    let sent = commands(step);
    let (_, choice) = answer_of(&sent).ok_or("an answer")?;
    assert_eq!(choice.direction, Direction::FromDevice);
    assert_eq!(choice.from_folders, vec!["Music".to_owned()]);
    Ok(())
}

#[test]
fn r_looks_for_devices_again_except_where_it_is_typed() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device("SNOWSKY", "U1", 9)]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    rig.focus("device");
    assert!(rig.dialog.hints().iter().any(|hint| hint.key == "r"));
    let step = rig.press(ch('r'));
    assert_eq!(
        commands(step),
        vec![send(TaskId::SyncDevice, Command::Offer(Offer::Devices))]
    );
    assert_eq!(rig.busy(), Some("Looking for devices"));
    // In a tick list the letter is typed into the filter.
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device("SNOWSKY", "U1", 9)]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    rig.focus("what");
    assert!(matches!(rig.dialog.form().focus(), Focus::Field("what")));
    assert!(!rig.dialog.hints().iter().any(|hint| hint.key == "r"));
    assert_eq!(commands(rig.press(ch('r'))), Vec::new());
}

#[test]
fn choosing_another_device_sets_the_form_from_what_it_remembered() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![
        device("SNOWSKY", "U1", 9),
        device("PHILIPS", "U2", 9),
    ]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    assert_eq!(
        rig.dialog.form().value("format"),
        Some(Value::Choice(Some("m4a".into())))
    );
    // PHILIPS was never synced: the defaults (As is, whole library not ticked).
    let _ = rig.click(&Target::Row {
        field: "device",
        index: 1,
    })?;
    assert_eq!(
        rig.dialog.form().value("format"),
        Some(Value::Choice(Some("as-is".into())))
    );
    assert_eq!(rig.ticked("what"), Vec::<String>::new());
    Ok(())
}

// ----- Convert -----

fn convert_settings() -> Settings {
    Settings {
        state: State {
            convert: ConvertChoice {
                what: Selection {
                    whole_library: true,
                    playlists: Vec::new(),
                },
                destination: PathBuf::from("/tmp/out"),
                ..ConvertChoice::default()
            },
            ..State::default()
        },
        ..Settings::default()
    }
}

fn convert_answer(commands: &[Cmd]) -> Option<&ConvertChoice> {
    commands
        .iter()
        .find_map(|cmd| match sent_to(cmd, TaskId::Convert)? {
            Command::Answer(Answers::Convert(choice)) => Some(choice),
            _ => None,
        })
}

#[test]
fn convert_plans_the_remembered_choices_then_starts() -> TestResult {
    let (mut rig, opened) = Rig::open(TaskId::Convert, convert_settings());
    assert_eq!(opened, vec![Cmd::Start(TaskId::Convert)]);
    assert_eq!(rig.busy(), Some("Reading your playlists"));
    let step = rig.event(&EventKind::Playlists(vec!["Road trip".into()]));
    assert!(commands(step).is_empty(), "the libraries are still to come");
    let step = rig.event(&libraries(&[]));
    let sent = commands(step);
    let choice = convert_answer(&sent).ok_or("an answer")?;
    assert_eq!(choice.format, Format::Mp3);
    assert!(choice.what.whole_library && choice.keep_cover);
    assert_eq!(choice.destination, PathBuf::from("/tmp/out"));
    let _ = rig.event(&EventKind::Plan(plan(12, 0, None)));
    assert_eq!(
        rig.dialog.form().summary(),
        Some("12 files to convert · 3 already there")
    );
    assert_eq!(rig.dialog.form().primary(), "Start");
    rig.focus_primary();
    assert_eq!(rig.enter(), vec![send(TaskId::Convert, Command::Go)]);
    assert!(is_closed(&rig.event(&EventKind::Started)));
    Ok(())
}

#[test]
fn convert_says_why_a_destination_is_refused() {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Plan(PlanSummary {
        refusal: Some("The destination is inside your library.".into()),
        ..plan(12, 0, None)
    }));
    assert_eq!(
        rig.dialog.form().missing(),
        vec!["The destination is inside your library.".to_owned()]
    );
    assert!(rig.enter().is_empty(), "Start does nothing");
}

#[test]
fn typing_a_destination_makes_no_plan_until_enter() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Plan(plan(12, 0, None)));
    // Move to the destination field and type.
    rig.focus("destination");
    assert!(matches!(
        rig.dialog.form().focus(),
        Focus::Field("destination")
    ));
    let step = rig.press(ch('2'));
    assert!(commands(step).is_empty(), "no plan per keystroke");
    assert_eq!(rig.dialog.form().summary(), None);
    assert_eq!(rig.dialog.form().primary(), "Continue");
    let sent = rig.enter();
    let choice = convert_answer(&sent).ok_or("Enter continues")?;
    assert_eq!(choice.destination, PathBuf::from("/tmp/out2"));
    Ok(())
}

#[test]
fn a_new_format_relabels_the_quality_and_makes_a_new_plan() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let _ = rig.event(&libraries(&[]));
    let _ = rig.event(&EventKind::Plan(plan(12, 0, None)));
    let label = |rig: &Rig| -> Result<String, Box<dyn std::error::Error>> {
        let area = Rect::new(0, 0, 100, 60);
        let mut buf = Buffer::empty(area);
        let _ = render(rig.dialog.form(), area, &mut buf, &FormStyle::default());
        Ok(area
            .rows()
            .map(|row| {
                row.columns()
                    .filter_map(|p| buf.cell((p.x, p.y)))
                    .map(|cell| cell.symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"))
    };
    assert!(label(&rig)?.contains("High (about 190 kbps)"));
    // The format dropdown: Enter opens, Down, Enter chooses M4A.
    rig.focus("format");
    let _ = rig.press(code(crossterm::event::KeyCode::Enter));
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let step = rig.press(code(crossterm::event::KeyCode::Enter));
    let sent = commands(step);
    let choice = convert_answer(&sent).ok_or("a new plan is asked for")?;
    assert_eq!(choice.format, Format::M4a);
    assert!(label(&rig)?.contains("High (about 192 kbps)"));
    assert_eq!(rig.dialog.form().summary(), None);
    Ok(())
}

#[test]
fn convert_cancel_tells_the_task() {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let step = rig.press(code(crossterm::event::KeyCode::Esc));
    assert!(is_closed(&step));
    assert_eq!(commands(step), vec![send(TaskId::Convert, Command::Cancel)]);
}

// ----- Libraries (F12) -----

/// The form as the owner sees it, drawn on a tall terminal.
fn drawn(rig: &Rig) -> String {
    let area = Rect::new(0, 0, 100, 60);
    let mut buf = Buffer::empty(area);
    let _ = render(rig.dialog.form(), area, &mut buf, &FormStyle::default());
    area.rows()
        .map(|row| {
            row.columns()
                .filter_map(|p| buf.cell((p.x, p.y)))
                .map(|cell| cell.symbol().to_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_visible(rig: &Rig, key: &str) -> bool {
    rig.dialog
        .form()
        .fields()
        .iter()
        .any(|field| field.key() == key && field.is_visible())
}

fn choice_of(rig: &Rig, key: &'static str) -> Option<String> {
    match rig.dialog.form().value(key) {
        Some(Value::Choice(id)) => id,
        _ => None,
    }
}

/// Open the dropdown `key`, press `keys`, and choose with Enter.
fn pick(rig: &mut Rig, key: &'static str, keys: &[crossterm::event::KeyCode]) -> Vec<Cmd> {
    rig.focus(key);
    let _ = rig.press(code(crossterm::event::KeyCode::Enter));
    for each in keys {
        let _ = rig.press(code(*each));
    }
    commands(rig.press(code(crossterm::event::KeyCode::Enter)))
}

/// A download form open with the libraries Kim and Sam listed and Sam used last, and what it must
/// have before it can start (a ticked list, or a link).
fn download_rig(task: TaskId, others: &[&str]) -> Rig {
    let mut settings = spotify_settings();
    settings.state.last_friend.insert(task, "Sam".to_owned());
    let (mut rig, _) = Rig::open(task, settings);
    let _ = rig.event(&libraries(others));
    if task == TaskId::SyncSpotify {
        let _ = rig.event(&EventKind::Lists(lists()));
    } else {
        rig.focus("link");
        let _ = rig
            .dialog
            .paste("https://music.youtube.com/watch?v=abc", &rig.settings);
    }
    rig
}

/// The recipient of the answer the form gives now.
fn recipient_sent(rig: &mut Rig) -> Option<sdl_tasks::Recipient> {
    rig.focus_primary();
    rig.enter().into_iter().find_map(|cmd| match cmd {
        Cmd::Send(_, command) => match *command {
            Command::Answer(
                Answers::SyncSpotify { recipient, .. } | Answers::DownloadLink { recipient, .. },
            ) => Some(recipient),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn the_download_forms_start_for_me_and_offer_a_library_when_it_is_turned_off() -> TestResult {
    for task in [TaskId::SyncSpotify, TaskId::DownloadLink] {
        let mut rig = download_rig(task, &["Kim", "Sam"]);
        // For me is the first field and is on; nothing about libraries shows.
        assert_eq!(*rig.dialog.form().fields()[0].key(), *"for_me");
        assert_eq!(rig.dialog.form().value("for_me"), Some(Value::Flag(true)));
        assert!(!is_visible(&rig, "library") && !is_visible(&rig, "new_library"));
        // Off: the dropdown shows with the library used last chosen.
        rig.focus("for_me");
        let _ = rig.press(ch(' '));
        assert!(is_visible(&rig, "library") && !is_visible(&rig, "new_library"));
        assert_eq!(choice_of(&rig, "library"), Some("Sam".to_owned()));
        // It offers the other libraries, then New library…; not the owner's own.
        rig.focus("library");
        let _ = rig.press(code(crossterm::event::KeyCode::Enter));
        let shown = drawn(&rig);
        let kim = shown.find("Kim").ok_or("no Kim")?;
        let sam = shown.rfind("Sam").ok_or("no Sam")?;
        let new = shown.find("New library…").ok_or("no New library…")?;
        assert!(kim < sam && sam < new, "{shown}");
        assert!(!shown.contains("Me"), "{shown}");
        let _ = rig.press(code(crossterm::event::KeyCode::Esc));
        assert_eq!(
            recipient_sent(&mut rig),
            Some(sdl_tasks::Recipient::Existing("Sam".to_owned()))
        );
        // Another library, chosen with the keyboard; then For me again.
        let mut rig = download_rig(task, &["Kim", "Sam"]);
        rig.focus("for_me");
        let _ = rig.press(ch(' '));
        let _ = pick(&mut rig, "library", &[crossterm::event::KeyCode::Up]);
        assert_eq!(
            recipient_sent(&mut rig),
            Some(sdl_tasks::Recipient::Existing("Kim".to_owned()))
        );
        let mut rig = download_rig(task, &["Kim", "Sam"]);
        assert_eq!(recipient_sent(&mut rig), Some(sdl_tasks::Recipient::Me));
    }
    Ok(())
}

#[test]
fn the_for_me_toggle_is_on_every_time_a_download_form_opens_and_works_with_the_mouse() -> TestResult
{
    let mut rig = download_rig(TaskId::DownloadLink, &["Sam"]);
    let _ = rig.click(&Target::Field("for_me"))?;
    assert_eq!(rig.dialog.form().value("for_me"), Some(Value::Flag(false)));
    assert!(is_visible(&rig, "library"));
    // The library last used is remembered, but For me is on again at the next open.
    let mut again = download_rig(TaskId::DownloadLink, &["Sam"]);
    assert_eq!(again.dialog.form().value("for_me"), Some(Value::Flag(true)));
    assert_eq!(recipient_sent(&mut again), Some(sdl_tasks::Recipient::Me));
    Ok(())
}

#[test]
fn a_new_library_needs_a_name_that_is_not_taken() {
    for task in [TaskId::SyncSpotify, TaskId::DownloadLink] {
        let mut rig = download_rig(task, &["Kim", "Sam"]);
        rig.focus("for_me");
        let _ = rig.press(ch(' '));
        let _ = pick(&mut rig, "library", &[crossterm::event::KeyCode::Down]);
        assert_eq!(choice_of(&rig, "library"), Some("/new".to_owned()));
        assert!(is_visible(&rig, "new_library"));
        // Empty: the form says what is missing and does not start.
        let missing = rig.dialog.form().missing();
        assert_eq!(missing, vec!["Type a name for the library".to_owned()]);
        assert_eq!(recipient_sent(&mut rig), None);
        // Taken, whatever the case.
        rig.focus("new_library");
        let _ = rig.dialog.paste("sam", &rig.settings);
        assert_eq!(
            rig.dialog.form().missing(),
            vec!["A library named Sam already exists. Choose another name.".to_owned()]
        );
        assert_eq!(recipient_sent(&mut rig), None);
        // Good.
        rig.focus("new_library");
        let _ = rig.press(KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..ch('u')
        });
        let _ = rig.dialog.paste("Lee", &rig.settings);
        assert_eq!(rig.dialog.form().missing(), Vec::<String>::new());
        assert_eq!(
            recipient_sent(&mut rig),
            Some(sdl_tasks::Recipient::New("Lee".to_owned()))
        );
    }
}

#[test]
fn with_no_other_library_the_only_choice_is_a_new_one() {
    let mut rig = download_rig(TaskId::DownloadLink, &[]);
    rig.focus("for_me");
    let _ = rig.press(ch(' '));
    assert_eq!(choice_of(&rig, "library"), Some("/new".to_owned()));
    assert!(is_visible(&rig, "new_library"));
}

fn sam_device_settings() -> Settings {
    let mut settings = device_settings(Some("U1"));
    if let Some(saved) = settings.state.devices.get_mut("U1") {
        saved.library = LibraryId::Other("Sam".into());
        saved.last_synced = Some(LibraryId::Own);
    }
    settings
}

fn playlists_of(library: &str) -> Cmd {
    send(
        TaskId::SyncDevice,
        Command::Offer(Offer::Playlists(match library {
            "" => LibraryId::Own,
            name => LibraryId::Other(name.to_owned()),
        })),
    )
}

#[test]
fn sync_device_starts_with_the_remembered_library_and_its_playlists() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, sam_device_settings());
    assert_eq!(rig.dialog.form().fields()[0].key(), "library");
    assert!(matches!(rig.dialog.form().focus(), Focus::Field("library")));
    let _ = rig.event(&EventKind::Devices(vec![device(
        "SNOWSKY",
        "U1",
        9_000_000_000,
    )]));
    // The owner's own playlists come first, as the task sends them at open.
    let _ = rig.event(&EventKind::Playlists(vec!["Mine".into()]));
    let step = rig.event(&libraries(&["Kim", "Sam"]));
    assert!(commands(step.clone()).contains(&playlists_of("Sam")));
    assert!(
        answer_of(&commands(step)).is_none(),
        "the playlists are awaited"
    );
    assert_eq!(rig.busy(), Some("Reading the playlists"));
    let step = rig.event(&EventKind::Playlists(vec!["Sam's mix".into()]));
    let sent = commands(step);
    let (_, choice) = answer_of(&sent).ok_or("an answer")?;
    assert_eq!(choice.library, LibraryId::Other("Sam".into()));
    // The What list is the chosen library's.
    let shown = drawn(&rig);
    assert!(
        shown.contains("Sam's mix") && !shown.contains("Mine"),
        "{shown}"
    );
    // The library was last synced from the owner's: the note says the two merge there.
    let notes = rig.dialog.form().notes().join(" ");
    assert!(
        notes.contains("last synced from library Me")
            && notes.contains("will merge the two libraries on this device"),
        "{notes}"
    );
    // Back to the owner's own library: its playlists are asked for, and nothing merges.
    let sent = pick(
        &mut rig,
        "library",
        &[crossterm::event::KeyCode::Up, crossterm::event::KeyCode::Up],
    );
    assert!(sent.contains(&playlists_of("")), "{sent:?}");
    assert!(!rig.dialog.form().notes().join(" ").contains("merge"));
    assert_eq!(rig.busy(), Some("Reading the playlists"));
    let step = rig.event(&EventKind::Playlists(vec!["Mine".into()]));
    let sent = commands(step);
    let (_, choice) = answer_of(&sent).ok_or("an answer")?;
    assert_eq!(choice.library, LibraryId::Own);
    assert!(drawn(&rig).contains("Mine"));
    Ok(())
}

#[test]
fn a_remembered_library_that_is_gone_gives_way_to_the_owners_own() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, sam_device_settings());
    let _ = rig.event(&EventKind::Devices(vec![device("SNOWSKY", "U1", 9)]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let step = rig.event(&libraries(&["Kim"]));
    assert_eq!(choice_of(&rig, "library"), Some("/own".to_owned()));
    assert!(!commands(step.clone()).contains(&playlists_of("Sam")));
    assert!(answer_of(&commands(step)).is_some());
}

#[test]
fn convert_starts_with_the_remembered_library_and_follows_a_change() -> TestResult {
    let mut settings = convert_settings();
    settings.state.convert.library = LibraryId::Other("Sam".into());
    let (mut rig, _) = Rig::open(TaskId::Convert, settings);
    assert!(matches!(rig.dialog.form().focus(), Focus::Field("library")));
    let _ = rig.event(&EventKind::Playlists(vec!["Mine".into()]));
    let step = rig.event(&libraries(&["Kim", "Sam"]));
    let asked = Cmd::send(
        TaskId::Convert,
        Command::Offer(Offer::Playlists(LibraryId::Other("Sam".into()))),
    );
    assert_eq!(commands(step), vec![asked]);
    assert_eq!(rig.busy(), Some("Reading your playlists"));
    let step = rig.event(&EventKind::Playlists(vec!["Sam's mix".into()]));
    let sent = commands(step);
    let choice = convert_answer(&sent).ok_or("an answer")?;
    assert_eq!(choice.library, LibraryId::Other("Sam".into()));
    assert!(drawn(&rig).contains("Sam's mix"));
    // The owner's own first in the dropdown; choosing Kim asks for Kim's playlists.
    let sent = pick(&mut rig, "library", &[crossterm::event::KeyCode::Up]);
    assert_eq!(
        sent,
        vec![Cmd::send(
            TaskId::Convert,
            Command::Offer(Offer::Playlists(LibraryId::Other("Kim".into())))
        )]
    );
    assert_eq!(rig.dialog.form().summary(), None);
    let step = rig.event(&EventKind::Playlists(vec!["Kim's mix".into()]));
    let sent = commands(step);
    let choice = convert_answer(&sent).ok_or("a new plan")?;
    assert_eq!(choice.library, LibraryId::Other("Kim".into()));
    Ok(())
}

#[test]
fn convert_opens_on_the_owners_own_library_when_none_was_remembered() -> TestResult {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    let sent = commands(rig.event(&libraries(&["Sam"])));
    let choice = convert_answer(&sent).ok_or("an answer")?;
    assert_eq!(choice.library, LibraryId::Own);
    assert!(
        !sent.iter().any(
            |cmd| matches!(cmd, Cmd::Send(_, command) if matches!(**command, Command::Offer(_)))
        )
    );
    assert!(drawn(&rig).contains("Me"));
    Ok(())
}

#[test]
fn review_names_a_songs_library_when_it_is_not_the_owners_own() -> TestResult {
    let mut review = ReviewList::default();
    review.hold(held("t1", "First", false));
    let mut sams = held("t1", "First", false);
    sams.library = LibraryId::Other("Sam".into());
    review.hold(sams);
    let settings = Settings {
        review,
        ..Settings::default()
    };
    let (mut rig, _) = Rig::open(TaskId::Review, settings);
    let shown = drawn(&rig);
    assert_eq!(shown.matches("library Sam").count(), 1, "{shown}");
    assert_eq!(shown.matches("First").count(), 2, "{shown}");
    // Open Sam's song: the detail names the library. Never download it.
    let _ = rig.click(&Target::Row {
        field: "songs",
        index: 1,
    })?;
    assert!(
        rig.dialog
            .form()
            .notes()
            .iter()
            .any(|note| note == "Goes in library Sam")
    );
    choose_never(&mut rig);
    rig.focus_primary();
    let _ = rig.enter();
    // The same key held for two libraries stays two songs: only Sam's is decided.
    rig.focus_primary();
    let sent = rig.enter();
    assert_eq!(
        sent,
        vec![send(
            TaskId::Review,
            Command::Answer(Answers::Review {
                choices: vec![ReviewChoice {
                    library: LibraryId::Other("Sam".into()),
                    key: "t1".into(),
                    resolution: Resolution::Never,
                }]
            })
        )]
    );
    // The owner's own song has no library line.
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    assert!(!drawn(&rig).contains("library"));
    let _ = rig.enter();
    assert!(
        !rig.dialog
            .form()
            .notes()
            .iter()
            .any(|n| n.contains("library"))
    );
    Ok(())
}

/// Choose `Never download` in the open detail.
fn choose_never(rig: &mut Rig) {
    rig.focus("choice");
    for _ in 0..6 {
        if choice_of(rig, "choice").as_deref() == Some("never") {
            break;
        }
        let _ = rig.press(code(crossterm::event::KeyCode::Down));
    }
}
