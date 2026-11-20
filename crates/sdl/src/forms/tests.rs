use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyModifiers};
use sdl_core::event::LIKED_SONGS_ID;

use super::*;
use crate::form::Summary;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn enter(dialog: &mut Dialog) -> Reply {
    dialog.key(press(KeyCode::Enter))
}

fn space(dialog: &mut Dialog) -> Reply {
    dialog.key(press(KeyCode::Char(' ')))
}

fn down(dialog: &mut Dialog) {
    dialog.key(press(KeyCode::Down));
}

fn right(dialog: &mut Dialog) {
    dialog.key(press(KeyCode::Right));
}

fn lists() -> Vec<ListChoice> {
    vec![
        ListChoice {
            name: "Liked Songs".into(),
            id: LIKED_SONGS_ID.into(),
            count: 2_117,
        },
        ListChoice {
            name: "Road trip".into(),
            id: "p1".into(),
            count: 40,
        },
    ]
}

fn device(name: &str, uuid: &str, free: u64) -> Device {
    Device {
        name: name.into(),
        mount_point: PathBuf::from("/Volumes").join(name),
        volume_uuid: Some(uuid.into()),
        free_bytes: free,
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn plan(files: usize, bytes: u64, already: usize, free: Option<u64>) -> PlanSummary {
    PlanSummary {
        files,
        bytes,
        already,
        free_bytes: free,
        refusal: None,
    }
}

fn info(dialog: &Dialog) -> String {
    match dialog.form().summary() {
        Summary::Info(text) | Summary::Warn(text) | Summary::Busy(text) => text.clone(),
        Summary::None => String::new(),
    }
}

// ---- Sync Spotify ----

#[test]
fn spotify_starts_on_what_was_remembered() {
    let state = State {
        ticked_lists: vec![LIKED_SONGS_ID.to_owned()],
        formats: BTreeMap::from([(TaskId::SyncSpotify, Format::Mp3)]),
        ..State::default()
    };
    let dialog = Dialog::spotify(lists(), &state);
    assert_eq!(dialog.form().ticked(LISTS), vec![0]);
    assert_eq!(dialog.form().choice(FORMAT), 1);
    assert!(!dialog.form().toggle(REMEMBER));
    assert_eq!(info(&dialog), "1 ticked · 2,117 songs");
}

#[test]
fn spotify_needs_a_list_before_it_continues() {
    let mut dialog = Dialog::spotify(lists(), &State::default());
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    assert_eq!(dialog.stage(), Stage::Editing);
    assert!(
        dialog
            .hints()
            .contains(&Hint::Missing("Tick a list".into()))
    );
    assert_eq!(info(&dialog), "");
}

#[test]
fn spotify_goes_through_check_then_start() {
    let mut dialog = Dialog::spotify(lists(), &State::default());
    space(&mut dialog);
    down(&mut dialog);
    space(&mut dialog);
    assert_eq!(info(&dialog), "2 ticked · 2,157 songs");
    // Format MP3, remembered.
    down(&mut dialog);
    right(&mut dialog);
    down(&mut dialog);
    space(&mut dialog);
    let reply = enter(&mut dialog);
    assert_eq!(
        reply,
        Reply::Stay(vec![Command::Answer(Answers::Spotify {
            lists: lists(),
            format: FormatChoice {
                format: Format::Mp3,
                remember: true
            },
        })])
    );
    assert_eq!(dialog.stage(), Stage::Checking);
    assert!(dialog.form().is_locked());
    assert!(matches!(dialog.form().summary(), Summary::Busy(_)));
    // Nothing changes while the songs are listed, and Enter does not ask twice.
    assert_eq!(space(&mut dialog), Reply::Stay(Vec::new()));
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    // While it waits, the form shows what the task is doing.
    dialog.apply(&EventKind::Headline(
        "Listing Liked Songs (50 so far)".into(),
    ));
    assert_eq!(
        dialog.form().summary(),
        &Summary::Busy("Listing Liked Songs (50 so far)…".into())
    );

    dialog.apply(&EventKind::NewOfTotal {
        new: 12,
        total: 2_157,
    });
    assert_eq!(dialog.stage(), Stage::Ready);
    assert_eq!(info(&dialog), "12 new of 2,157");
    assert_eq!(
        enter(&mut dialog),
        Reply::Start(vec![Command::Go(TaskId::SyncSpotify)])
    );
}

#[test]
fn changing_a_row_after_the_count_asks_again_before_starting() {
    let mut dialog = Dialog::spotify(lists(), &State::default());
    space(&mut dialog);
    enter(&mut dialog);
    dialog.apply(&EventKind::NewOfTotal { new: 1, total: 2 });
    assert_eq!(dialog.stage(), Stage::Ready);
    down(&mut dialog);
    space(&mut dialog);
    assert_eq!(dialog.stage(), Stage::Editing);
    assert_eq!(info(&dialog), "2 ticked · 2,157 songs");
    assert!(matches!(enter(&mut dialog), Reply::Stay(commands) if commands.len() == 1));
    assert_eq!(dialog.stage(), Stage::Checking);
}

#[test]
fn escape_cancels_in_every_stage() {
    let mut dialog = Dialog::spotify(lists(), &State::default());
    assert_eq!(dialog.key(press(KeyCode::Esc)), Reply::Cancel);
    space(&mut dialog);
    enter(&mut dialog);
    assert_eq!(dialog.key(press(KeyCode::Esc)), Reply::Cancel);
}

// ---- Download link ----

#[test]
fn link_needs_text_then_starts_with_its_format() {
    let state = State {
        formats: BTreeMap::from([(TaskId::DownloadLink, Format::M4a)]),
        ..State::default()
    };
    let mut dialog = Dialog::link(&state);
    assert_eq!(dialog.form().choice(FORMAT), 2);
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    assert!(
        dialog
            .hints()
            .contains(&Hint::Missing("Paste a link".into()))
    );
    assert!(!dialog.hints().contains(&keys::FORM_START.hint()));
    dialog.paste("  https://example.org/list \n");
    assert!(dialog.hints().contains(&keys::FORM_START.hint()));
    assert_eq!(
        enter(&mut dialog),
        Reply::Start(vec![Command::Answer(Answers::Link {
            url: "https://example.org/list".into(),
            format: FormatChoice {
                format: Format::M4a,
                remember: false
            },
        })])
    );
    assert_eq!(
        dialog.chosen_label().as_deref(),
        Some("https://example.org/list")
    );
}

#[test]
fn typing_in_the_link_row_is_text_not_commands() {
    let mut dialog = Dialog::link(&State::default());
    for c in "qsa?r".chars() {
        dialog.key(press(KeyCode::Char(c)));
    }
    assert_eq!(dialog.form().text(LINK), "qsa?r");
}

// ---- Convert ----

#[test]
fn convert_is_filled_from_the_remembered_choice() {
    let state = State {
        convert: ConvertChoice {
            folders: names(&["Youtube"]),
            format: Format::Opus,
            destination: PathBuf::from("/out"),
        },
        ..State::default()
    };
    let dialog = Dialog::convert(&names(&["Spotify", "Youtube"]), &state);
    assert_eq!(dialog.form().ticked(FOLDERS), vec![1]);
    assert_eq!(dialog.form().choice(FORMAT), 2);
    assert_eq!(dialog.form().text(DESTINATION), "/out");
}

#[test]
fn convert_asks_with_its_choice_and_shows_the_plan() {
    let mut dialog = Dialog::convert(&names(&["Spotify", "Youtube"]), &State::default());
    space(&mut dialog);
    let reply = enter(&mut dialog);
    assert_eq!(
        reply,
        Reply::Stay(vec![Command::Answer(Answers::Convert(ConvertChoice {
            folders: names(&["Spotify"]),
            format: Format::Mp3,
            destination: PathBuf::from("~/Music-converted"),
        }))])
    );
    dialog.apply(&EventKind::Plan(plan(120, 0, 5, None)));
    assert_eq!(dialog.stage(), Stage::Ready);
    assert!(!dialog.form().is_locked());
    assert_eq!(info(&dialog), "120 files to convert · 5 already there");
    assert_eq!(
        enter(&mut dialog),
        Reply::Start(vec![Command::Go(TaskId::Convert)])
    );
}

#[test]
fn a_refused_plan_blocks_enter_until_a_row_changes() {
    let mut dialog = Dialog::convert(&names(&["Spotify"]), &State::default());
    space(&mut dialog);
    enter(&mut dialog);
    let mut refused = plan(0, 0, 0, None);
    refused.refusal = Some("The destination is inside a folder you are converting.".into());
    dialog.apply(&EventKind::Plan(refused));
    assert_eq!(dialog.stage(), Stage::Blocked);
    assert_eq!(
        dialog.form().summary(),
        &Summary::Warn("The destination is inside a folder you are converting.".into())
    );
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    assert!(!dialog.hints().contains(&keys::FORM_START.hint()));
    assert!(!dialog.hints().contains(&keys::FORM_CONTINUE.hint()));
    // Fix the destination: the form is open again and Enter checks again.
    down(&mut dialog);
    down(&mut dialog);
    dialog.paste("2");
    assert_eq!(dialog.stage(), Stage::Editing);
    assert_eq!(info(&dialog), "");
    assert!(matches!(enter(&mut dialog), Reply::Stay(commands) if commands.len() == 1));
}

#[test]
fn convert_needs_folders_and_a_destination() {
    let mut dialog = Dialog::convert(&names(&["Spotify"]), &State::default());
    assert!(
        dialog
            .hints()
            .contains(&Hint::Missing("Tick a folder".into()))
    );
    space(&mut dialog);
    down(&mut dialog);
    down(&mut dialog);
    dialog.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(
        dialog.form().missing().as_deref(),
        Some("Type a destination folder")
    );
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
}

#[test]
fn convert_with_no_library_folders_says_so() {
    let dialog = Dialog::convert(&[], &State::default());
    assert_eq!(
        dialog.form().missing().as_deref(),
        Some("Your music folder has no folders yet.")
    );
}

// ---- Sync device ----

fn asked_folders(commands: &[Command]) -> Option<(&Device, Direction)> {
    match commands {
        [Command::Offer(Offer::Folders { device, direction })] => Some((device, *direction)),
        _ => None,
    }
}

#[test]
fn no_device_shows_the_message_and_r_looks_again() {
    let (mut dialog, commands) = Dialog::device(Vec::new(), &State::default());
    assert_eq!(commands, Vec::new());
    assert_eq!(dialog.form().focused(), Some(DEVICE));
    assert!(!dialog.form().is_shown(FOLDERS));
    assert!(!dialog.form().is_shown(DIRECTION));
    assert_eq!(dialog.form().missing().as_deref(), Some("Plug in a device"));
    assert!(dialog.hints().contains(&keys::REFRESH.hint()));
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    assert_eq!(
        dialog.key(press(KeyCode::Char('r'))),
        Reply::Stay(vec![Command::Offer(Offer::Devices)])
    );
    assert!(matches!(dialog.form().summary(), Summary::Busy(_)));
    // A device appears: the form fills and asks for its folders.
    let card = device("CARD", "U-1", 10_000_000_000);
    let commands = dialog.apply(&EventKind::Devices(vec![card.clone()]));
    assert_eq!(asked_folders(&commands), Some((&card, Direction::ToDevice)));
    assert!(dialog.form().is_shown(FOLDERS));
    assert!(dialog.form().is_shown(DEVICE_FORMAT));
}

#[test]
fn devices_are_shown_by_name_and_free_space_and_folders_load_before_ticking() {
    let state = State {
        devices: BTreeMap::from([(
            "U-2".to_owned(),
            DeviceChoice {
                folders: names(&["Spotify"]),
                direction: Direction::ToDevice,
                format: Some(Format::M4a),
            },
        )]),
        ..State::default()
    };
    let one = device("ONE", "U-1", 500_000_000);
    let two = device("TWO", "U-2", 12_300_000_000);
    let (mut dialog, commands) = Dialog::device(vec![one, two.clone()], &state);
    // The remembered device is chosen first, with its remembered format.
    assert_eq!(dialog.form().choice(DEVICE), 1);
    assert_eq!(dialog.form().choice(DEVICE_FORMAT), 2);
    assert_eq!(asked_folders(&commands), Some((&two, Direction::ToDevice)));
    assert_eq!(
        dialog.form().missing().as_deref(),
        Some("Wait for the list")
    );
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    assert_eq!(dialog.form().focused(), Some(FOLDERS));

    dialog.apply(&EventKind::Folders(names(&["Spotify", "Youtube"])));
    assert_eq!(dialog.form().ticked(FOLDERS), vec![0]);
    assert!(dialog.form().is_complete());
}

#[test]
fn only_the_last_folder_answer_counts() {
    let card = device("CARD", "U-1", 1);
    let (mut dialog, _) = Dialog::device(vec![card], &State::default());
    // Flip the direction twice quickly: two more folder lists are asked for.
    dialog.form.focus_on(DIRECTION);
    right(&mut dialog);
    dialog.form.focus_on(DIRECTION);
    right(&mut dialog);
    // The answers come back in order: first the library's, then the device's, then the library's.
    dialog.apply(&EventKind::Folders(names(&["first"])));
    assert_eq!(dialog.form().ticks(FOLDERS), []);
    dialog.apply(&EventKind::Folders(names(&["second"])));
    assert_eq!(dialog.form().ticks(FOLDERS), []);
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    assert_eq!(dialog.form().ticks(FOLDERS).len(), 1);
    assert_eq!(
        dialog
            .form()
            .ticks(FOLDERS)
            .first()
            .map(|t| t.label.as_str()),
        Some("Spotify")
    );
}

#[test]
fn direction_changes_folders_and_hides_the_format_row() -> TestResult {
    let card = device("CARD", "U-1", 1_000);
    let (mut dialog, _) = Dialog::device(vec![card.clone()], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify", "Youtube"])));
    dialog.form.focus_on(FOLDERS);
    space(&mut dialog);
    assert!(dialog.form().is_shown(DEVICE_FORMAT));

    dialog.form.focus_on(DIRECTION);
    let Reply::Stay(commands) = dialog.key(press(KeyCode::Right)) else {
        return Err("the direction change closed the form".into());
    };
    assert_eq!(
        asked_folders(&commands),
        Some((&card, Direction::FromDevice))
    );
    assert!(!dialog.form().is_shown(DEVICE_FORMAT));
    assert!(
        dialog.form().ticks(FOLDERS).len() == 2,
        "old list shows until the new one lands"
    );
    assert_eq!(
        dialog.form().missing().as_deref(),
        Some("Wait for the list")
    );
    // The device's folders arrive; the ticked name carries over when the device has it.
    dialog.apply(&EventKind::Folders(names(&["Spotify", "Other"])));
    assert_eq!(dialog.form().ticked(FOLDERS), vec![0]);

    // Back again: the format row returns.
    dialog.form.focus_on(DIRECTION);
    dialog.key(press(KeyCode::Left));
    assert!(dialog.form().is_shown(DEVICE_FORMAT));
    Ok(())
}

#[test]
fn from_device_with_nothing_in_music_says_so() {
    let card = device("CARD", "U-1", 1_000);
    let (mut dialog, _) = Dialog::device(vec![card], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    dialog.form.focus_on(DIRECTION);
    right(&mut dialog);
    dialog.apply(&EventKind::Folders(Vec::new()));
    assert_eq!(
        dialog.form().missing().as_deref(),
        Some("This device has no folders in its Music folder.")
    );
}

#[test]
fn choosing_another_device_loads_what_was_remembered_for_it() {
    let state = State {
        devices: BTreeMap::from([(
            "U-2".to_owned(),
            DeviceChoice {
                folders: names(&["Youtube"]),
                direction: Direction::FromDevice,
                format: None,
            },
        )]),
        ..State::default()
    };
    let one = device("ONE", "U-1", 1);
    let two = device("TWO", "U-2", 2);
    let (mut dialog, _) = Dialog::device(vec![one, two.clone()], &state);
    // U-2 is remembered, so it starts chosen; move to ONE and back.
    dialog.form.focus_on(DEVICE);
    dialog.key(press(KeyCode::Left));
    assert_eq!(dialog.form().choice(DIRECTION), 0);
    dialog.key(press(KeyCode::Right));
    assert_eq!(dialog.form().choice(DIRECTION), 1);
    assert!(!dialog.form().is_shown(DEVICE_FORMAT));
    // Three folder lists were asked for (the first look and two device changes); the last counts.
    for _ in 0..3 {
        dialog.apply(&EventKind::Folders(names(&["Youtube"])));
    }
    assert_eq!(dialog.form().ticked(FOLDERS), vec![0]);
    assert_eq!(dialog.chosen_label().as_deref(), Some("TWO"));
}

#[test]
fn looking_again_keeps_the_chosen_device_when_it_is_still_there() {
    let one = device("ONE", "U-1", 1);
    let two = device("TWO", "U-2", 2);
    let (mut dialog, _) = Dialog::device(vec![one.clone(), two.clone()], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    dialog.form.focus_on(DEVICE);
    right(&mut dialog);
    dialog.apply(&EventKind::Folders(names(&["Music"])));
    assert_eq!(dialog.chosen_label().as_deref(), Some("TWO"));
    // A third drive is plugged in and the list is read again.
    let three = device("THREE", "U-3", 3);
    let commands = dialog.apply(&EventKind::Devices(vec![one, two, three]));
    assert_eq!(commands, Vec::new());
    assert_eq!(dialog.chosen_label().as_deref(), Some("TWO"));
    assert_eq!(dialog.stage(), Stage::Editing);
    // The chosen device is gone: the first one is chosen and its folders asked for.
    let kept = device("ONE", "U-1", 1);
    let commands = dialog.apply(&EventKind::Devices(vec![kept.clone()]));
    assert_eq!(asked_folders(&commands), Some((&kept, Direction::ToDevice)));
    assert_eq!(dialog.chosen_label().as_deref(), Some("ONE"));
}

#[test]
fn the_device_plan_says_how_much_room_there_is() -> TestResult {
    let card = device("CARD", "U-1", 10_000_000_000);
    let (mut dialog, _) = Dialog::device(vec![card.clone()], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    dialog.form.focus_on(FOLDERS);
    space(&mut dialog);
    let Reply::Stay(commands) = enter(&mut dialog) else {
        return Err("Enter should ask for the plan".into());
    };
    assert_eq!(
        commands,
        vec![Command::Answer(Answers::Device {
            device: card,
            choice: DeviceChoice {
                folders: names(&["Spotify"]),
                direction: Direction::ToDevice,
                format: None,
            },
        })]
    );
    dialog.apply(&EventKind::Plan(plan(
        1_200,
        4_250_000_000,
        30,
        Some(10_000_000_000),
    )));
    assert_eq!(dialog.stage(), Stage::Ready);
    assert_eq!(
        info(&dialog),
        "1,200 files, 4.2 GB to copy · 30 already there · 10.0 GB free on CARD"
    );
    assert_eq!(
        enter(&mut dialog),
        Reply::Start(vec![Command::Go(TaskId::SyncDevice)])
    );
    Ok(())
}

#[test]
fn not_enough_room_is_said_in_plain_words_and_enter_is_unavailable() {
    let card = device("CARD", "U-1", 1_000_000_000);
    let (mut dialog, _) = Dialog::device(vec![card], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    dialog.form.focus_on(FOLDERS);
    space(&mut dialog);
    enter(&mut dialog);
    dialog.apply(&EventKind::Plan(plan(
        10,
        4_000_000_000,
        0,
        Some(1_000_000_000),
    )));
    assert_eq!(dialog.stage(), Stage::Blocked);
    assert_eq!(
        info(&dialog),
        "Not enough room on CARD: 10 files need 4.0 GB, 1.0 GB free. Untick a folder."
    );
    assert_eq!(enter(&mut dialog), Reply::Stay(Vec::new()));
    // Untick the folder: the form is open for a new check.
    dialog.form.focus_on(FOLDERS);
    space(&mut dialog);
    assert_eq!(dialog.stage(), Stage::Editing);
}

#[test]
fn from_device_has_no_free_space_figure() {
    let card = device("CARD", "U-1", 1_000);
    let (mut dialog, _) = Dialog::device(vec![card], &State::default());
    dialog.apply(&EventKind::Folders(names(&["Spotify"])));
    dialog.form.focus_on(DIRECTION);
    right(&mut dialog);
    dialog.apply(&EventKind::Folders(names(&["Music"])));
    dialog.form.focus_on(FOLDERS);
    space(&mut dialog);
    enter(&mut dialog);
    dialog.apply(&EventKind::Plan(plan(1, 1, 0, None)));
    assert_eq!(info(&dialog), "1 file, 1 B to copy · 0 already there");
    assert_eq!(dialog.stage(), Stage::Ready);
}

#[test]
fn events_for_other_steps_change_nothing() {
    let mut dialog = Dialog::spotify(lists(), &State::default());
    dialog.apply(&EventKind::Plan(plan(1, 1, 0, None)));
    dialog.apply(&EventKind::NewOfTotal { new: 1, total: 1 });
    dialog.apply(&EventKind::Folders(names(&["x"])));
    assert_eq!(dialog.stage(), Stage::Editing);
    assert_eq!(dialog.apply(&EventKind::Finished), Vec::new());
}
