use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use sdl_core::{
    Attention, Counts, Device, ErrorInfo, EventKind, FormatChoice, LastRun, ListChoice,
    PlanSummary, RunOutcome, SignInState, State, TaskId, event::LIKED_SONGS_ID,
};

use super::*;
use crate::forms::Stage;
use crate::tasks::Answers;
use crate::theme::Theme;
use crate::ui;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn plan(files: usize, already: usize) -> PlanSummary {
    PlanSummary {
        files,
        bytes: 0,
        already,
        free_bytes: None,
        refusal: None,
    }
}

fn app() -> App {
    App::new(State::default(), 1_000)
}

fn send(app: &mut App, task: TaskId, kind: EventKind) -> Vec<Command> {
    app.apply_event(Event { task, kind })
}

fn counts(total: usize, done: usize, unmatched: usize) -> Counts {
    Counts {
        total,
        done,
        added: done,
        unmatched,
        ..Counts::default()
    }
}

const URL: &str = "https://example.org/list";

fn link_answer() -> Command {
    Command::Answer(Answers::Link {
        url: URL.into(),
        format: FormatChoice::default(),
    })
}

/// Open Download link, paste a link, press Enter. Returns what Enter sent.
fn start_link(app: &mut App) -> Vec<Command> {
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    app.handle_paste(URL);
    app.handle_key(press(KeyCode::Enter))
}

fn choices() -> Vec<ListChoice> {
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

fn card() -> Device {
    Device {
        name: "CARD".into(),
        mount_point: "/Volumes/CARD".into(),
        volume_uuid: Some("U-1".into()),
        free_bytes: 10_000_000_000,
    }
}

/// Open Sync device up to a ready plan: devices, folders, one folder ticked, plan.
fn device_ready(app: &mut App) {
    app.selected = TaskId::SyncDevice;
    app.handle_key(press(KeyCode::Enter));
    send(app, TaskId::SyncDevice, EventKind::Devices(vec![card()]));
    send(
        app,
        TaskId::SyncDevice,
        EventKind::Folders(vec!["Spotify".into()]),
    );
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Char(' ')));
    app.handle_key(press(KeyCode::Enter));
    send(app, TaskId::SyncDevice, EventKind::Plan(plan(3, 9)));
}

fn screen(app: &App, width: u16, height: u16) -> Result<String, Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
    terminal.draw(|frame| ui::render(frame, app, Theme::new(true)))?;
    Ok(text(terminal.backend().buffer()))
}

fn text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width.max(1))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The foreground colour of the first cell of `needle` on screen.
fn colour_of(app: &App, needle: &str) -> Result<Option<Color>, Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
    terminal.draw(|frame| ui::render(frame, app, Theme::new(true)))?;
    let buffer = terminal.backend().buffer();
    let shown = text(buffer);
    for (y, line) in shown.lines().enumerate() {
        if let Some(x) = line.find(needle) {
            let x = u16::try_from(line[..x].chars().count())?;
            let y = u16::try_from(y)?;
            return Ok(buffer.cell((x, y)).map(|cell| cell.fg));
        }
    }
    Ok(None)
}

fn dialog(app: &App) -> Option<&Dialog> {
    match app.popup() {
        Some(Popup::Form(dialog)) => Some(dialog),
        _ => None,
    }
}

// ---- running panel and tasks ----

#[test]
fn running_panel_follows_running_tasks() {
    let mut app = app();
    assert_eq!(app.running(), Vec::<TaskId>::new());
    assert_eq!(start_link(&mut app), vec![link_answer()]);
    assert_eq!(app.running(), vec![TaskId::DownloadLink]);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(10, 4, 1)),
    );
    let view = app.view(TaskId::DownloadLink);
    assert_eq!(view.state.counts.done, 4);
    assert_eq!(view.state.counts.need_attention(), 1);
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(app.running(), Vec::<TaskId>::new());
}

#[test]
fn two_functions_run_side_by_side_and_close_one_by_one() {
    let mut app = app();
    start_link(&mut app);
    device_ready(&mut app);
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Go(TaskId::SyncDevice)]
    );
    assert_eq!(
        app.running(),
        vec![TaskId::DownloadLink, TaskId::SyncDevice]
    );
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(app.running(), vec![TaskId::SyncDevice]);
}

#[test]
fn the_same_function_cannot_start_twice() {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert!(app.popup().is_none());
}

// ---- Download link ----

#[test]
fn download_link_opens_its_form_and_starts_its_task() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Start(TaskId::DownloadLink)]
    );
    assert!(dialog(&app).is_some());
    assert_eq!(app.view(TaskId::DownloadLink).phase, Phase::Preparing);
    assert_eq!(app.running(), Vec::<TaskId>::new());
}

#[test]
fn pasted_newline_never_submits() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    let commands = app.handle_paste("https://a.example/x\nhttps://b.example/y\r\n");
    assert_eq!(commands, Vec::<Command>::new());
    assert_eq!(app.running(), Vec::<TaskId>::new());
    assert_eq!(
        dialog(&app).and_then(Dialog::chosen_label).as_deref(),
        Some("https://a.example/xhttps://b.example/y")
    );
}

#[test]
fn empty_link_does_not_submit_and_escape_leaves() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert!(dialog(&app).is_some());
    assert!(
        app.hints()
            .contains(&Hint::Missing("Paste a link".to_owned()))
    );
    assert_eq!(
        app.handle_key(press(KeyCode::Esc)),
        vec![Command::Cancel(TaskId::DownloadLink)]
    );
    assert!(app.popup().is_none());
    send(&mut app, TaskId::DownloadLink, EventKind::Stopped);
    assert_eq!(app.view(TaskId::DownloadLink).phase, Phase::Idle);
}

#[test]
fn typing_in_the_link_field_does_not_trigger_keys() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    for c in "qsa?".chars() {
        app.handle_key(press(KeyCode::Char(c)));
    }
    assert_eq!(
        dialog(&app).and_then(Dialog::chosen_label).as_deref(),
        Some("qsa?")
    );
    assert!(!app.should_quit());
}

#[test]
fn the_link_shows_in_the_main_panel_once_started() {
    let mut app = app();
    start_link(&mut app);
    assert_eq!(app.selection_text(TaskId::DownloadLink), URL);
}

// ---- Sync Spotify ----

#[test]
fn spotify_flow_sign_in_form_check_start() -> TestResult {
    let state = State {
        ticked_lists: vec![LIKED_SONGS_ID.to_owned()],
        ..State::default()
    };
    let mut app = App::new(state, 0);

    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Start(TaskId::SyncSpotify)]
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::SignIn(SignInState::OpeningBrowser))
    ));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::SignIn(SignInState::WaitingForBrowser),
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::SignIn(SignInState::WaitingForBrowser))
    ));
    assert_eq!(app.running(), Vec::<TaskId>::new());

    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    let Some(form) = dialog(&app) else {
        return Err("the lists did not open the form".into());
    };
    assert_eq!(form.form().ticked("lists"), vec![0]);

    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Char(' ')));
    let commands = app.handle_key(press(KeyCode::Enter));
    assert_eq!(
        commands,
        vec![Command::Answer(Answers::Spotify {
            lists: choices(),
            format: FormatChoice::default(),
        })]
    );
    assert_eq!(app.running(), Vec::<TaskId>::new());

    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::NewOfTotal {
            new: 12,
            total: 2_157,
        },
    );
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Go(TaskId::SyncSpotify)]
    );
    assert_eq!(app.running(), vec![TaskId::SyncSpotify]);
    assert!(app.popup().is_none());
    Ok(())
}

#[test]
fn enter_with_nothing_ticked_does_not_continue() {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert_eq!(dialog(&app).map(Dialog::stage), Some(Stage::Editing));
    assert!(
        app.hints()
            .contains(&Hint::Missing("Tick a list".to_owned()))
    );
}

#[test]
fn escape_during_sign_in_cancels_and_blocks_a_restart_until_stopped() {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    let commands = app.handle_key(press(KeyCode::Esc));
    assert_eq!(commands, vec![Command::Cancel(TaskId::SyncSpotify)]);
    assert!(app.popup().is_none());
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    send(&mut app, TaskId::SyncSpotify, EventKind::Stopped);
    assert!(app.popup().is_none());
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Start(TaskId::SyncSpotify)]
    );
}

#[test]
fn leaving_the_spotify_form_cancels_the_waiting_task() {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    assert_eq!(
        app.handle_key(press(KeyCode::Esc)),
        vec![Command::Cancel(TaskId::SyncSpotify)]
    );
    assert!(app.popup().is_none());
    assert_eq!(app.view(TaskId::SyncSpotify).phase, Phase::Cancelling);
}

// ---- Convert ----

#[test]
fn convert_opens_looking_then_its_form_and_runs_from_the_plan() {
    let mut app = app();
    app.selected = TaskId::Convert;
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Start(TaskId::Convert)]
    );
    assert!(matches!(app.popup(), Some(Popup::Looking(TaskId::Convert))));
    send(
        &mut app,
        TaskId::Convert,
        EventKind::Folders(vec!["Spotify".into(), "Youtube".into()]),
    );
    assert!(dialog(&app).is_some());
    app.handle_key(press(KeyCode::Char(' ')));
    let commands = app.handle_key(press(KeyCode::Enter));
    assert!(matches!(
        commands.as_slice(),
        [Command::Answer(Answers::Convert(choice))] if choice.folders == ["Spotify"]
    ));
    send(&mut app, TaskId::Convert, EventKind::Plan(plan(4, 1)));
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Go(TaskId::Convert)]
    );
    assert_eq!(app.running(), vec![TaskId::Convert]);
}

#[test]
fn a_refused_convert_plan_keeps_the_form_open() {
    let mut app = app();
    app.selected = TaskId::Convert;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::Convert,
        EventKind::Folders(vec!["Spotify".into()]),
    );
    app.handle_key(press(KeyCode::Char(' ')));
    app.handle_key(press(KeyCode::Enter));
    let mut refused = plan(0, 0);
    refused.refusal = Some("The destination is inside a folder you are converting.".into());
    send(&mut app, TaskId::Convert, EventKind::Plan(refused));
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert_eq!(app.running(), Vec::<TaskId>::new());
    assert_eq!(dialog(&app).map(Dialog::stage), Some(Stage::Blocked));
}

// ---- Sync device ----

#[test]
fn device_flow_ends_with_the_eject_question_naming_the_device() {
    let mut app = app();
    app.selected = TaskId::SyncDevice;
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Start(TaskId::SyncDevice)]
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::Looking(TaskId::SyncDevice))
    ));
    let asked = send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Devices(vec![card()]),
    );
    assert!(matches!(asked.as_slice(), [Command::Offer(_)]));
    send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Folders(vec!["Spotify".into()]),
    );
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Char(' ')));
    assert!(matches!(
        app.handle_key(press(KeyCode::Enter)).as_slice(),
        [Command::Answer(Answers::Device { .. })]
    ));
    send(&mut app, TaskId::SyncDevice, EventKind::Plan(plan(3, 9)));
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Go(TaskId::SyncDevice)]
    );
    assert_eq!(app.selection_text(TaskId::SyncDevice), "CARD");
    send(&mut app, TaskId::SyncDevice, EventKind::Finished);
    assert_eq!(app.running(), Vec::<TaskId>::new());
    assert!(matches!(app.popup(), Some(Popup::EjectConfirm(name)) if name == "CARD"));
    assert_eq!(
        app.handle_key(press(KeyCode::Char('y'))),
        vec![Command::Eject]
    );
    assert!(matches!(app.popup(), Some(Popup::Ejecting)));
    send(&mut app, TaskId::SyncDevice, EventKind::Finished);
    assert!(app.popup().is_none());
    assert_eq!(app.view(TaskId::SyncDevice).phase, Phase::Idle);
    assert_eq!(
        app.selection_text(TaskId::SyncDevice),
        "CARD · ejected, safe to unplug"
    );
}

#[test]
fn declining_the_eject_closes_the_popup() {
    let mut app = app();
    device_ready(&mut app);
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncDevice, EventKind::Finished);
    assert_eq!(
        app.handle_key(press(KeyCode::Char('n'))),
        Vec::<Command>::new()
    );
    assert!(app.popup().is_none());
    assert_eq!(app.view(TaskId::SyncDevice).phase, Phase::Idle);
}

#[test]
fn no_device_found_offers_to_look_again() -> TestResult {
    let mut app = app();
    app.selected = TaskId::SyncDevice;
    app.handle_key(press(KeyCode::Enter));
    let none = send(&mut app, TaskId::SyncDevice, EventKind::Devices(Vec::new()));
    assert_eq!(none, Vec::<Command>::new());
    let shown = screen(&app, 80, 24)?;
    assert!(shown.contains("No device found."), "{shown}");
    assert!(shown.contains("Plug one in"), "{shown}");
    assert!(shown.contains("r look again"), "{shown}");
    assert!(matches!(
        app.handle_key(press(KeyCode::Char('r'))).as_slice(),
        [Command::Offer(_)]
    ));
    // Esc still leaves, and the device row on the main screen shows no device.
    assert_eq!(
        app.handle_key(press(KeyCode::Esc)),
        vec![Command::Cancel(TaskId::SyncDevice)]
    );
    assert_eq!(
        app.selection_text(TaskId::SyncDevice),
        "no device chosen yet"
    );
    Ok(())
}

#[test]
fn an_early_devices_event_after_cancelling_is_ignored() {
    let mut app = app();
    app.selected = TaskId::SyncDevice;
    app.handle_key(press(KeyCode::Enter));
    app.handle_key(press(KeyCode::Esc));
    let late = send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Devices(vec![card()]),
    );
    assert_eq!(late, Vec::<Command>::new());
    assert!(app.popup().is_none());
}

// ---- stopping, quitting, errors ----

#[test]
fn stop_needs_confirmation_and_ends_with_stopped() {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Char('s')));
    assert!(matches!(
        app.popup(),
        Some(Popup::ConfirmStop(TaskId::DownloadLink))
    ));
    assert_eq!(
        app.handle_key(press(KeyCode::Char('n'))),
        Vec::<Command>::new()
    );
    assert_eq!(app.running(), vec![TaskId::DownloadLink]);

    app.handle_key(press(KeyCode::Char('s')));
    let commands = app.handle_key(press(KeyCode::Char('y')));
    assert_eq!(commands, vec![Command::Stop(TaskId::DownloadLink)]);
    send(&mut app, TaskId::DownloadLink, EventKind::Stopped);
    assert_eq!(app.running(), Vec::<TaskId>::new());
}

#[test]
fn stop_key_does_nothing_when_the_function_is_not_running() {
    let mut app = app();
    app.handle_key(press(KeyCode::Char('s')));
    assert!(app.popup().is_none());
}

#[test]
fn quit_asks_only_while_a_task_runs() {
    let mut idle = app();
    assert_eq!(
        idle.handle_key(press(KeyCode::Char('q'))),
        vec![Command::Quit]
    );
    assert!(idle.should_quit());

    let mut busy = app();
    start_link(&mut busy);
    assert_eq!(
        busy.handle_key(press(KeyCode::Char('q'))),
        Vec::<Command>::new()
    );
    assert!(matches!(busy.popup(), Some(Popup::ConfirmQuit)));
    assert!(!busy.should_quit());
    busy.handle_key(press(KeyCode::Char('n')));
    assert!(!busy.should_quit());
    busy.handle_key(press(KeyCode::Char('q')));
    assert_eq!(
        busy.handle_key(press(KeyCode::Char('y'))),
        vec![Command::Stop(TaskId::DownloadLink), Command::Quit]
    );
    assert!(busy.should_quit());
}

#[test]
fn quitting_with_only_a_form_open_stops_nothing() {
    let mut app = app();
    start_link(&mut app);
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    app.selected = TaskId::Convert;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::Convert,
        EventKind::Folders(vec!["A".into()]),
    );
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Char('q')));
    assert!(app.should_quit());
}

#[test]
fn ctrl_c_quits_like_q() {
    let mut app = app();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.handle_key(ctrl_c), vec![Command::Quit]);
}

#[test]
fn error_event_ends_the_run_and_opens_the_popup() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    let info = ErrorInfo {
        message: "yt-dlp was not found.".into(),
        advice: "Start sdl with `mise run tui` so it is on PATH.".into(),
    };
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Error(info.clone()),
    );
    assert_eq!(app.running(), Vec::<TaskId>::new());
    let Some(Popup::Error(_, shown)) = app.popup() else {
        return Err("no error popup".into());
    };
    assert_eq!(shown, &info);
    app.handle_key(press(KeyCode::Enter));
    assert!(app.popup().is_none());
    // q closes it too, as the help says.
    send(&mut app, TaskId::DownloadLink, EventKind::Error(info));
    app.handle_key(press(KeyCode::Char('q')));
    assert!(app.popup().is_none());
    Ok(())
}

#[test]
fn rate_limit_shows_the_wait_or_says_unknown() {
    let with = rate_limit_info(Some(Duration::from_secs(600)));
    assert!(with.message.contains("about 10 min"));
    let without = rate_limit_info(None);
    assert!(without.message.contains("Wait unknown"));
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::RateLimited { wait: None },
    );
    assert!(matches!(app.popup(), Some(Popup::Error(..))));
    assert_eq!(app.running(), Vec::<TaskId>::new());
}

fn info(message: &str) -> ErrorInfo {
    ErrorInfo {
        message: message.into(),
        advice: "do this".into(),
    }
}

#[test]
fn another_functions_popup_never_hides_a_waiting_step_for_good() {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::SyncDevice;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Error(info("no card")),
    );
    app.handle_key(press(KeyCode::Enter));
    app.selected = TaskId::SyncSpotify;
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Error(info("boom")),
    );
    assert!(matches!(app.popup(), Some(Popup::Error(..))));
    app.handle_key(press(KeyCode::Enter));
    assert_eq!(dialog(&app).map(Dialog::task), Some(TaskId::SyncSpotify));
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(dialog(&app).map(Dialog::task), Some(TaskId::SyncSpotify));
}

#[test]
fn an_error_after_cancelling_is_still_shown() {
    let mut app = app();
    app.selected = TaskId::SyncDevice;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Devices(vec![card()]),
    );
    assert_eq!(
        app.handle_key(press(KeyCode::Esc)),
        vec![Command::Cancel(TaskId::SyncDevice)]
    );
    send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Headline("late".into()),
    );
    assert_eq!(app.view(TaskId::SyncDevice).state.headline, "");
    send(
        &mut app,
        TaskId::SyncDevice,
        EventKind::Error(info("unmount failed")),
    );
    assert!(
        matches!(app.popup(), Some(Popup::Error(_, shown)) if shown.message == "unmount failed")
    );
    assert_eq!(app.view(TaskId::SyncDevice).phase, Phase::Idle);
}

#[test]
fn an_error_after_the_run_ended_is_still_shown() {
    let mut app = app();
    start_link(&mut app);
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Error(info("state not saved")),
    );
    assert!(matches!(app.popup(), Some(Popup::Error(..))));
}

// ---- attention ----

fn attention_app(count: usize) -> App {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    for n in 0..count {
        send(
            &mut app,
            TaskId::DownloadLink,
            EventKind::Attention(Attention::unmatched(
                format!("Song {n}"),
                "Artist".into(),
                "no match".into(),
            )),
        );
    }
    app
}

#[test]
fn attention_list_scrolls_and_closes() {
    let app = attention_app(0);
    assert!(!app.hints().contains(&keys::ATTENTION.hint()));
    let mut app = attention_app(3);
    assert!(app.hints().contains(&keys::ATTENTION.hint()));
    app.handle_key(press(KeyCode::Char('a')));
    for _ in 0..3 {
        app.handle_key(press(KeyCode::Down));
    }
    assert!(matches!(
        app.popup(),
        Some(Popup::Attention { cursor: 2, .. })
    ));
    app.handle_key(press(KeyCode::Esc));
    assert!(app.popup().is_none());
}

#[test]
fn attention_survives_the_end_of_the_run_until_the_next_run() {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention::failed("T".into(), String::new(), "r".into())),
    );
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(app.view(TaskId::DownloadLink).state.attention.len(), 1);
    // Opening the form and leaving it keeps the last run's list.
    app.handle_key(press(KeyCode::Enter));
    app.handle_key(press(KeyCode::Esc));
    send(&mut app, TaskId::DownloadLink, EventKind::Stopped);
    assert_eq!(app.view(TaskId::DownloadLink).state.attention.len(), 1);
    start_link(&mut app);
    assert_eq!(app.view(TaskId::DownloadLink).state.attention.len(), 0);
}

#[test]
fn a_long_attention_reason_wraps_instead_of_being_cut_off() -> TestResult {
    let mut app = attention_app(0);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention::failed(
            "A long one".into(),
            "Someone".into(),
            "The server answered with an unusual error that explains in many words exactly what went wrong with this one song ENDWORD".into(),
        )),
    );
    app.handle_key(press(KeyCode::Char('a')));
    for width in [60, 80, 120] {
        let shown = screen(&app, width, 30)?;
        assert!(shown.contains("ENDWORD"), "{width}: {shown}");
        assert!(shown.contains("The server answered"), "{width}: {shown}");
    }
    Ok(())
}

#[test]
fn the_attention_cursor_item_stays_on_screen_with_wrapped_items() -> TestResult {
    let mut app = attention_app(0);
    for n in 0..12 {
        send(
            &mut app,
            TaskId::DownloadLink,
            EventKind::Attention(Attention::failed(
                format!("Song {n}"),
                String::new(),
                "word ".repeat(30),
            )),
        );
    }
    app.handle_key(press(KeyCode::Char('a')));
    for _ in 0..9 {
        app.handle_key(press(KeyCode::Down));
    }
    let shown = screen(&app, 70, 30)?;
    assert!(shown.contains("▶ Song 9"), "{shown}");
    app.handle_key(press(KeyCode::End));
    assert!(screen(&app, 70, 30)?.contains("▶ Song 11"));
    Ok(())
}

#[test]
fn unmatched_is_yellow_and_failed_is_red_in_the_attention_list() -> TestResult {
    let mut app = attention_app(0);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention::unmatched(
            "One".into(),
            String::new(),
            "no good match".into(),
        )),
    );
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention::failed(
            "Two".into(),
            String::new(),
            "download broke".into(),
        )),
    );
    app.handle_key(press(KeyCode::Char('a')));
    let theme = Theme::new(true);
    assert_eq!(colour_of(&app, "no good match")?, theme.attention().fg);
    assert_eq!(colour_of(&app, "download broke")?, theme.failed().fg);
    assert_ne!(theme.attention().fg, theme.failed().fg);
    Ok(())
}

// ---- main panel ----

#[test]
fn help_opens_and_closes() {
    let mut app = app();
    app.handle_key(press(KeyCode::Char('?')));
    assert!(matches!(app.popup(), Some(Popup::Help { .. })));
    app.handle_key(press(KeyCode::Esc));
    assert!(app.popup().is_none());
}

#[test]
fn help_scroll_stops_at_the_last_full_page() {
    let mut app = app();
    // A short screen: the help does not fit and can scroll.
    app.set_viewport(80, 12);
    let last = popups::help_max_scroll(12);
    assert!(last > 0);
    app.handle_key(press(KeyCode::Char('?')));
    for _ in 0..100 {
        app.handle_key(press(KeyCode::Down));
    }
    assert!(matches!(app.popup(), Some(Popup::Help { scroll }) if *scroll == last));
    // One press up moves at once: the scroll never went past the end.
    app.handle_key(press(KeyCode::Up));
    assert!(matches!(app.popup(), Some(Popup::Help { scroll }) if *scroll == last - 1));
    app.handle_key(press(KeyCode::End));
    assert!(matches!(app.popup(), Some(Popup::Help { scroll }) if *scroll == last));
    app.handle_key(press(KeyCode::PageDown));
    assert!(matches!(app.popup(), Some(Popup::Help { scroll }) if *scroll == last));
    // The screen grows: what showed is now clamped, and a tall screen has nothing to scroll.
    app.set_viewport(80, 60);
    assert!(matches!(app.popup(), Some(Popup::Help { scroll: 0 })));
    app.handle_key(press(KeyCode::Down));
    assert!(matches!(app.popup(), Some(Popup::Help { scroll: 0 })));
}

#[test]
fn tick_ages_running_tasks_only() {
    let mut app = app();
    start_link(&mut app);
    app.tick(Duration::from_secs(2), 1_002);
    assert_eq!(
        app.view(TaskId::DownloadLink).elapsed,
        Duration::from_secs(2)
    );
    assert_eq!(app.view(TaskId::SyncDevice).elapsed, Duration::ZERO);
    assert_eq!(app.now, 1_002);
}

#[test]
fn selection_text_per_function() {
    let mut app = app();
    assert_eq!(
        app.selection_text(TaskId::SyncSpotify),
        "no lists chosen yet"
    );
    app.state.ticked_lists = vec![LIKED_SONGS_ID.into(), "a".into(), "b".into()];
    assert_eq!(
        app.selection_text(TaskId::SyncSpotify),
        "Liked Songs and 2 playlists"
    );
    app.state.ticked_lists = vec!["a".into()];
    assert_eq!(app.selection_text(TaskId::SyncSpotify), "1 playlist");
    assert_eq!(app.selection_text(TaskId::DownloadLink), "no link yet");
    assert_eq!(
        app.selection_text(TaskId::SyncDevice),
        "no device chosen yet"
    );
    assert_eq!(
        app.selection_text(TaskId::Convert),
        "MP3 to ~/Music-converted"
    );
}

#[test]
fn tone_follows_the_saved_outcome() {
    assert_eq!(tone(RunOutcome::Done), Tone::Done);
    assert_eq!(tone(RunOutcome::NeedsAttention), Tone::Attention);
    assert_eq!(tone(RunOutcome::Stopped), Tone::Stopped);
    assert_eq!(tone(RunOutcome::Failed), Tone::Failed);
}

#[test]
fn the_runners_state_replaces_the_screens() {
    let mut app = app();
    let mut state = State::default();
    state.last_run.insert(
        TaskId::SyncDevice,
        LastRun {
            line: "3 copied".into(),
            at: 900,
            outcome: RunOutcome::Done,
        },
    );
    app.set_state(state);
    assert_eq!(app.state.last_run.len(), 1);
}

fn last_run(outcome: RunOutcome, line: &str) -> LastRun {
    LastRun {
        line: line.into(),
        at: 400,
        outcome,
    }
}

#[test]
fn the_last_run_line_takes_its_colour_from_the_saved_outcome() -> TestResult {
    let theme = Theme::new(true);
    let mut app = app();
    app.state
        .last_run
        .insert(TaskId::SyncSpotify, last_run(RunOutcome::Done, "all fine"));
    app.state.last_run.insert(
        TaskId::DownloadLink,
        last_run(RunOutcome::NeedsAttention, "some odd"),
    );
    app.state
        .last_run
        .insert(TaskId::Convert, last_run(RunOutcome::Failed, "it broke"));
    app.state
        .last_run
        .insert(TaskId::SyncDevice, last_run(RunOutcome::Stopped, "halted"));
    assert_eq!(colour_of(&app, "all fine")?, theme.done().fg);
    assert_eq!(colour_of(&app, "some odd")?, theme.attention().fg);
    assert_eq!(colour_of(&app, "it broke")?, theme.failed().fg);
    assert_eq!(colour_of(&app, "halted")?, theme.skipped().fg);
    // The wording does not decide the colour: a line that says "failed" but was stored as done is
    // green.
    app.state.last_run.insert(
        TaskId::SyncSpotify,
        last_run(RunOutcome::Done, "failed nothing"),
    );
    assert_eq!(colour_of(&app, "failed nothing")?, theme.done().fg);
    Ok(())
}

#[test]
fn main_panel_shows_selection_and_last_run() -> TestResult {
    let mut app = app();
    app.state.last_run.insert(
        TaskId::SyncSpotify,
        last_run(RunOutcome::Done, "12 added, 3 skipped"),
    );
    let shown = screen(&app, 80, 24)?;
    assert!(shown.contains("Sync Spotify"));
    assert!(shown.contains("Download link"));
    assert!(shown.contains("Sync device"));
    assert!(shown.contains("Convert"));
    assert!(shown.contains("12 added, 3 skipped"));
    assert!(shown.contains("10\u{a0}min\u{a0}ago"));
    assert!(shown.contains("Last run  never"));
    assert!(!shown.contains("Running"));
    Ok(())
}

#[test]
fn the_device_row_shows_the_devices_name() -> TestResult {
    let mut app = app();
    device_ready(&mut app);
    app.handle_key(press(KeyCode::Enter));
    let shown = screen(&app, 100, 24)?;
    assert!(shown.contains("Sync device  CARD"), "{shown}");
    Ok(())
}

#[test]
fn running_panel_shows_count_estimate_and_attention() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(2_117, 412, 3)),
    );
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Headline("Downloading".into()),
    );
    app.tick(Duration::from_secs(60), 1_060);
    let shown = screen(&app, 100, 24)?;
    assert!(shown.contains("Running"));
    assert!(shown.contains("Downloading"));
    assert!(shown.contains("412 of 2,117"));
    assert!(shown.contains("19%"));
    assert!(shown.contains("3 need attention"));
    assert!(shown.contains("left"));
    Ok(())
}

#[test]
fn narrow_screen_puts_the_running_panel_below() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(10, 5, 0)),
    );
    let shown = screen(&app, 50, 30)?;
    let lines: Vec<&str> = shown.lines().collect();
    let running = lines.iter().position(|l| l.contains("Running"));
    let main = lines.iter().position(|l| l.contains("Sync device"));
    assert!(matches!((main, running), (Some(m), Some(r)) if m < r));
    Ok(())
}

#[test]
fn short_screen_shows_the_rows_the_counts_and_a_one_line_footer() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(10, 5, 0)),
    );
    for height in [11, 8, 5] {
        let shown = screen(&app, 80, height)?;
        assert!(shown.contains("Sync Spotify"), "{height}: {shown}");
        assert!(shown.contains("5 of 10"), "{height}: {shown}");
        assert!(!shown.contains("Last run"), "{height}: {shown}");
        let footer = shown.lines().last().unwrap_or_default();
        assert!(footer.contains("q quit"), "{height}: {shown}");
        assert!(footer.contains("s stop"), "{height}: {shown}");
    }
    Ok(())
}

#[test]
fn every_popup_draws_at_every_size() -> TestResult {
    let state = State::default();
    let (device_form, _) = Dialog::device(vec![card()], &state);
    let (no_device, _) = Dialog::device(Vec::new(), &state);
    let popups = vec![
        Popup::SignIn(SignInState::Failed("denied".into())),
        Popup::Looking(TaskId::Convert),
        Popup::Looking(TaskId::SyncDevice),
        Popup::Form(Dialog::spotify(choices(), &state)),
        Popup::Form(Dialog::link(&state)),
        Popup::Form(Dialog::convert(&vec!["Spotify".to_owned(); 30], &state)),
        Popup::Form(device_form),
        Popup::Form(no_device),
        Popup::EjectConfirm("CARD".into()),
        Popup::Ejecting,
        Popup::ConfirmQuit,
        Popup::ConfirmStop(TaskId::SyncDevice),
        Popup::Error(
            TaskId::SyncDevice,
            ErrorInfo {
                message: "m".repeat(200),
                advice: "a".repeat(200),
            },
        ),
        Popup::Help { scroll: 0 },
        Popup::Help { scroll: 9_999 },
        Popup::Attention {
            task: TaskId::SyncSpotify,
            cursor: 0,
        },
    ];
    for popup in popups {
        for (width, height) in [(120, 40), (60, 12), (30, 8), (1, 1), (10, 3), (40, 5)] {
            let mut app = app();
            app.popups = vec![popup.clone()];
            screen(&app, width, height)?;
        }
    }
    Ok(())
}

// ---- the step form on screen ----

#[test]
fn the_spotify_form_shows_lists_counts_format_and_the_count_of_new_songs() -> TestResult {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    let shown = screen(&app, 100, 30)?;
    for wanted in [
        "Sync Spotify",
        "Lists",
        "Liked Songs",
        "2,117",
        "Road trip",
        "Format",
        "Original",
        "Remember this format",
        "Tick a list",
    ] {
        assert!(shown.contains(wanted), "{wanted}: {shown}");
    }
    app.handle_key(press(KeyCode::Char(' ')));
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::NewOfTotal {
            new: 12,
            total: 2_157,
        },
    );
    let shown = screen(&app, 100, 30)?;
    assert!(shown.contains("12 new of 2,157"), "{shown}");
    assert!(shown.contains("Enter start"), "{shown}");
    Ok(())
}

#[test]
fn the_footer_names_what_is_missing_and_offers_enter_only_when_complete() -> TestResult {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    let empty = screen(&app, 80, 24)?;
    assert!(empty.contains("Paste a link"), "{empty}");
    assert!(!empty.contains("Enter start"), "{empty}");
    app.handle_paste("https://example.org/");
    let filled = screen(&app, 80, 24)?;
    assert!(filled.contains("Enter start"), "{filled}");
    assert!(!filled.contains("Paste a link"), "{filled}");
    Ok(())
}

#[test]
fn the_form_fits_forty_columns_and_a_short_screen() -> TestResult {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::Lists(
            (0..40)
                .map(|n| ListChoice {
                    name: format!("Playlist number {n} with a rather long name indeed"),
                    id: format!("p{n}"),
                    count: n,
                })
                .collect(),
        ),
    );
    for (width, height) in [(40, 24), (40, 12), (40, 9)] {
        let shown = screen(&app, width, height)?;
        assert!(shown.contains("Format") || height < 12, "{shown}");
        assert!(shown.contains("Lists"), "{width}x{height}: {shown}");
    }
    Ok(())
}

#[test]
fn help_lists_every_key() -> TestResult {
    let mut app = app();
    app.set_viewport(100, 60);
    app.handle_key(press(KeyCode::Char('?')));
    let shown = screen(&app, 100, 60)?;
    for group in keys::HELP {
        for key in group.keys {
            assert!(shown.contains(key.help_key), "help is missing {}", key.key);
        }
    }
    Ok(())
}

#[test]
fn colour_is_painted_and_no_colour_is_plain() -> TestResult {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    let mut terminal = Terminal::new(TestBackend::new(60, 20))?;
    terminal.draw(|frame| ui::render(frame, &app, Theme::new(true)))?;
    let painted = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .all(|cell| cell.bg != Color::default());
    assert!(painted);
    terminal.draw(|frame| ui::render(frame, &app, Theme::new(false)))?;
    let plain = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .all(|cell| cell.bg == Color::default() && cell.fg == Color::default());
    assert!(plain);
    Ok(())
}

#[test]
fn hints_show_only_keys_that_work_now() {
    let mut app = app();
    assert!(app.hints().contains(&keys::RUN.hint()));
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    assert!(!app.hints().contains(&keys::RUN.hint()));
    assert!(app.hints().contains(&keys::STOP.hint()));
}

#[test]
fn a_narrow_footer_drops_the_moving_keys_first() -> TestResult {
    let mut app = attention_app(0);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention::failed("T".into(), String::new(), "r".into())),
    );
    let shown = screen(&app, 40, 24)?;
    let footer = shown.lines().last().unwrap_or_default();
    assert!(footer.contains("s stop"));
    assert!(footer.contains("? help"));
    assert!(footer.contains("q quit"));
    assert!(!footer.contains("choose"));
    Ok(())
}

#[test]
fn a_narrow_screen_keeps_every_running_count() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(30, 3, 1)),
    );
    app.selected = TaskId::SyncSpotify;
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    app.handle_key(press(KeyCode::Char(' ')));
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::NewOfTotal {
            new: 9,
            total: 2_117,
        },
    );
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::Counts(counts(2_117, 412, 2)),
    );
    assert_eq!(app.running().len(), 2);
    for width in [40, 59] {
        let shown = screen(&app, width, 24)?;
        assert!(shown.contains("412 of 2,117"), "{shown}");
        assert!(shown.contains("3 of 30"), "{shown}");
    }
    Ok(())
}

#[test]
fn the_window_keeps_the_cursor_near_the_middle_and_fills_the_end() {
    let one = |_: usize| 1;
    // Everything fits: nothing scrolls.
    assert_eq!(popups::window_top(5, 4, 8, one), 0);
    // Twenty entries, room for eight: the cursor sits mid-window, then the end holds the window.
    assert_eq!(popups::window_top(20, 0, 8, one), 0);
    assert_eq!(popups::window_top(20, 10, 8, one), 7);
    assert_eq!(popups::window_top(20, 19, 8, one), 12);
    // Taller entries count by their lines: three-line entries in nine lines.
    let three = |_: usize| 3;
    assert_eq!(popups::window_top(10, 5, 9, three), 4);
    assert_eq!(popups::window_top(10, 9, 9, three), 7);
    // An entry taller than the room still starts the window.
    assert_eq!(popups::window_top(4, 2, 2, three), 2);
    assert_eq!(popups::window_top(0, 0, 5, one), 0);
}

// UXHARNESS-START
#[test]
fn ux_harness_print() -> TestResult {
    let show = |title: &str, app: &App, w: u16, h: u16| -> TestResult {
        println!("===== {title} ({w}x{h})\n{}", screen(app, w, h)?);
        Ok(())
    };
    let mut a = app();
    a.handle_key(press(KeyCode::Enter));
    show("sign-in opening", &a, 80, 24)?;
    send(&mut a, TaskId::SyncSpotify, EventKind::SignIn(SignInState::WaitingForBrowser));
    show("sign-in waiting", &a, 80, 24)?;
    send(&mut a, TaskId::SyncSpotify, EventKind::SignIn(SignInState::SignedIn));
    show("signed in", &a, 80, 24)?;
    let mut lists = choices();
    for (i, name) in ["Workout", "Chill evenings with a very long playlist name that goes on", "Focus", "Discover", "Old school", "Rainy day", "Party", "Sleep", "Jazz"].iter().enumerate() {
        lists.push(ListChoice { name: (*name).into(), id: format!("x{i}"), count: i * 37 });
    }
    send(&mut a, TaskId::SyncSpotify, EventKind::Lists(lists));
    show("lists form", &a, 80, 24)?;
    a.handle_key(press(KeyCode::Char(' ')));
    show("lists ticked", &a, 80, 24)?;
    a.handle_key(press(KeyCode::Enter));
    send(&mut a, TaskId::SyncSpotify, EventKind::Headline("Listing songs: page 3 of 43".into()));
    show("listing", &a, 80, 24)?;
    send(&mut a, TaskId::SyncSpotify, EventKind::NewOfTotal { new: 12, total: 2_117 });
    show("new of total", &a, 80, 24)?;
    a.handle_key(press(KeyCode::Enter));
    send(&mut a, TaskId::SyncSpotify, EventKind::Headline("Downloading songs".into()));
    send(&mut a, TaskId::SyncSpotify, EventKind::Counts(Counts { total: 12, done: 5, added: 3, skipped: 0, unmatched: 1, failed: 1 }));
    a.tick(Duration::from_secs(40), 1_040);
    show("spotify running", &a, 80, 24)?;
    show("spotify running narrow", &a, 50, 24)?;
    show("spotify running compact", &a, 50, 11)?;
    send(&mut a, TaskId::SyncSpotify, EventKind::RateLimited { wait: Some(Duration::from_secs(3_700)) });
    show("rate limited", &a, 80, 24)?;
    let mut b = app();
    b.handle_key(press(KeyCode::Enter));
    send(&mut b, TaskId::SyncSpotify, EventKind::SignIn(SignInState::Failed("Spotify said: access_denied".into())));
    show("sign-in failed", &b, 80, 24)?;
    let mut c = app();
    c.selected = TaskId::DownloadLink;
    c.handle_key(press(KeyCode::Enter));
    send(&mut c, TaskId::DownloadLink, EventKind::Error(ErrorInfo { message: "yt-dlp was not found on PATH.".into(), advice: "Start sdl with `mise run tui` so it is on PATH.".into() }));
    show("error", &c, 80, 24)?;
    show("error", &c, 40, 24)?;
    Ok(())
}
// UXHARNESS-END
