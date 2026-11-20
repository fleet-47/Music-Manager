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
    ListChoice, PlanSummary, ReviewList, Selection, SongSource, State, TaskId, WantedSong,
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

fn send(task: TaskId, command: Command) -> Cmd {
    Cmd::Send(task, command)
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
        song: song(key, title, link),
        from: vec!["Liked Songs".into()],
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

fn review_settings() -> Settings {
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
                lists: vec![lists().remove(0)]
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
    });
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
    let _ = rig.press(code(crossterm::event::KeyCode::Down));
    let _ = rig.press(ch(' '));
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 1,
        total: 1,
        waiting: 0,
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
        [Cmd::Send(TaskId::SyncSpotify, Command::Answer(_))]
    ));
    let _ = rig.event(&EventKind::NewOfTotal {
        new: 2,
        total: 2,
        waiting: 0,
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
    let _ = rig.dialog.paste("not a link", &rig.settings);
    assert_eq!(rig.enter(), Vec::new());
    assert!(
        rig.dialog
            .form()
            .missing()
            .iter()
            .any(|line| line.contains("https://"))
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
                url: "https://music.youtube.com/watch?v=abc".into()
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
        vec!["Choose what to do with at least one song".to_owned()]
    );
}

#[test]
fn review_walk_from_the_list_to_download() {
    let (mut rig, _) = Rig::open(TaskId::Review, review_settings());
    // Enter opens the first song's detail.
    assert_eq!(rig.enter(), Vec::new());
    assert_eq!(rig.dialog.form().title(), "First");
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
    assert_eq!(rig.dialog.form().title(), "Second");
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
    assert_eq!(rig.dialog.form().title(), "First");
    // Another task landed the first song: the runner's list no longer holds it.
    let _ = rig.settings.review.resolve("t1");
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
        },
    );
    state.last_device = last.map(str::to_owned);
    Settings {
        state,
        ..Settings::default()
    }
}

fn answer_of(commands: &[Cmd]) -> Option<(&Device, &DeviceChoice)> {
    commands.iter().find_map(|cmd| match cmd {
        Cmd::Send(TaskId::SyncDevice, Command::Answer(Answers::SyncDevice { device, choice })) => {
            Some((device, choice))
        }
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
    // The devices arrive first, then the playlists: one plan, asked for once both are in.
    let step = rig.event(&EventKind::Devices(vec![
        device("PHILIPS", "U2", 5_000_000_000),
        device("SNOWSKY", "U1", 12_300_000_000),
    ]));
    assert!(commands(step).is_empty(), "the playlists are still to come");
    assert_eq!(rig.busy(), Some("Looking for devices"));
    let step = rig.event(&EventKind::Playlists(vec!["Road trip".into()]));
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
    assert_eq!(rig.enter(), vec![send(TaskId::SyncDevice, Command::Go)]);
    assert!(is_closed(&rig.event(&EventKind::Started)));
    Ok(())
}

#[test]
fn the_device_list_updates_when_volumes_change() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(Vec::new()));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
    assert_eq!(rig.busy(), None);
    assert!(
        rig.dialog
            .form()
            .missing()
            .contains(&"Choose a device".to_owned())
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
fn not_enough_room_says_so_with_the_numbers_and_start_is_unavailable() {
    let (mut rig, _) = Rig::open(TaskId::SyncDevice, device_settings(Some("U1")));
    let _ = rig.event(&EventKind::Devices(vec![device(
        "SNOWSKY",
        "U1",
        1_100_000_000,
    )]));
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
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
    let _ = rig.press(code(crossterm::event::KeyCode::Tab));
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
    for _ in 0..2 {
        let _ = rig.press(code(crossterm::event::KeyCode::Tab));
    }
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
    commands.iter().find_map(|cmd| match cmd {
        Cmd::Send(TaskId::Convert, Command::Answer(Answers::Convert(choice))) => Some(choice),
        _ => None,
    })
}

#[test]
fn convert_plans_the_remembered_choices_then_starts() -> TestResult {
    let (mut rig, opened) = Rig::open(TaskId::Convert, convert_settings());
    assert_eq!(opened, vec![Cmd::Start(TaskId::Convert)]);
    assert_eq!(rig.busy(), Some("Reading your playlists"));
    let step = rig.event(&EventKind::Playlists(vec!["Road trip".into()]));
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
    assert_eq!(rig.enter(), vec![send(TaskId::Convert, Command::Go)]);
    assert!(is_closed(&rig.event(&EventKind::Started)));
    Ok(())
}

#[test]
fn convert_says_why_a_destination_is_refused() {
    let (mut rig, _) = Rig::open(TaskId::Convert, convert_settings());
    let _ = rig.event(&EventKind::Playlists(Vec::new()));
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
    let _ = rig.event(&EventKind::Plan(plan(12, 0, None)));
    // Move to the destination field and type.
    for _ in 0..4 {
        let _ = rig.press(code(crossterm::event::KeyCode::Tab));
    }
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
    // The format dropdown is the second field: Tab, Enter opens, Down, Enter chooses M4A.
    let _ = rig.press(code(crossterm::event::KeyCode::Tab));
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
