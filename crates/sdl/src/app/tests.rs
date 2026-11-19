use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use sdl_core::{
    Attention, CardSummary, Counts, ErrorInfo, EventKind, LastRun, ListChoice, SignInState, State,
    TaskId, event::LIKED_SONGS_ID,
};

use super::*;
use crate::theme::Theme;
use crate::ui;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn app() -> App {
    App::new(State::default(), "card ABCD".to_owned(), 1_000)
}

fn send(app: &mut App, task: TaskId, kind: EventKind) {
    app.apply_event(Event { task, kind });
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

fn start_link(app: &mut App) -> Vec<Command> {
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    app.handle_paste("https://example.org/list");
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

#[test]
fn running_panel_follows_running_tasks() {
    let mut app = app();
    assert_eq!(app.running(), Vec::<TaskId>::new());
    let commands = start_link(&mut app);
    assert_eq!(
        commands,
        vec![Command::StartLink("https://example.org/list".into())]
    );
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
    app.selected = TaskId::SyncCard;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::CardSummary(CardSummary {
            to_copy: 3,
            already: 9,
        }),
    );
    app.handle_key(press(KeyCode::Enter));
    assert_eq!(app.running(), vec![TaskId::DownloadLink, TaskId::SyncCard]);
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(app.running(), vec![TaskId::SyncCard]);
}

#[test]
fn the_same_function_cannot_start_twice() {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert!(app.popup().is_none());
}

#[test]
fn pasted_newline_never_submits() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    app.handle_paste("https://a.example/x\nhttps://b.example/y\r\n");
    assert!(matches!(app.popup(), Some(Popup::Link(_))));
    assert_eq!(app.running(), Vec::<TaskId>::new());
    let Some(Popup::Link(input)) = app.popup() else {
        return;
    };
    assert_eq!(input.value(), "https://a.example/xhttps://b.example/y");
}

#[test]
fn empty_link_does_not_submit_and_esc_closes() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert!(matches!(app.popup(), Some(Popup::Link(_))));
    assert_eq!(app.handle_key(press(KeyCode::Esc)), Vec::<Command>::new());
    assert!(app.popup().is_none());
    assert_eq!(app.running(), Vec::<TaskId>::new());
}

#[test]
fn typing_in_the_link_field_does_not_trigger_keys() {
    let mut app = app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    for c in "qsa?".chars() {
        app.handle_key(press(KeyCode::Char(c)));
    }
    let Some(Popup::Link(input)) = app.popup() else {
        return;
    };
    assert_eq!(input.value(), "qsa?");
    assert!(!app.should_quit());
}

#[test]
fn spotify_flow_sign_in_lists_plan_start() {
    let state = State {
        ticked_lists: vec![LIKED_SONGS_ID.to_owned()],
        ..State::default()
    };
    let mut app = App::new(state, "card".into(), 0);

    let commands = app.handle_key(press(KeyCode::Enter));
    assert_eq!(commands, vec![Command::StartSpotify]);
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
    let Some(Popup::Lists(list)) = app.popup() else {
        return;
    };
    assert_eq!(list.ticked, vec![true, false]);

    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Char(' ')));
    let commands = app.handle_key(press(KeyCode::Enter));
    assert_eq!(commands, vec![Command::Choose(choices())]);
    assert_eq!(app.running(), Vec::<TaskId>::new());

    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::NewOfTotal {
            new: 12,
            total: 2_157,
        },
    );
    let commands = app.handle_key(press(KeyCode::Enter));
    assert_eq!(commands, vec![Command::Go(TaskId::SyncSpotify)]);
    assert_eq!(app.running(), vec![TaskId::SyncSpotify]);
    assert!(app.popup().is_none());
}

#[test]
fn enter_with_nothing_ticked_does_not_continue() {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    let Some(Popup::Lists(list)) = app.popup() else {
        return;
    };
    assert_eq!(list.stage, ListStage::Choosing);
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
        vec![Command::StartSpotify]
    );
}

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
        vec![Command::Quit]
    );
    assert!(busy.should_quit());
}

#[test]
fn ctrl_c_quits_like_q() {
    let mut app = app();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert_eq!(app.handle_key(ctrl_c), vec![Command::Quit]);
}

#[test]
fn error_event_ends_the_run_and_opens_the_popup() {
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
        return;
    };
    assert_eq!(shown, &info);
    app.handle_key(press(KeyCode::Enter));
    assert!(app.popup().is_none());
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

#[test]
fn card_flow_ends_with_the_unmount_question() {
    let mut app = app();
    app.selected = TaskId::SyncCard;
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::StartCard]
    );
    assert_eq!(app.handle_key(press(KeyCode::Enter)), Vec::<Command>::new());
    assert!(matches!(app.popup(), Some(Popup::CardPlan(None))));
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::CardSummary(CardSummary {
            to_copy: 3,
            already: 9,
        }),
    );
    assert_eq!(
        app.handle_key(press(KeyCode::Enter)),
        vec![Command::Go(TaskId::SyncCard)]
    );
    send(&mut app, TaskId::SyncCard, EventKind::Finished);
    assert_eq!(app.running(), Vec::<TaskId>::new());
    assert!(matches!(app.popup(), Some(Popup::UnmountConfirm)));
    assert_eq!(
        app.handle_key(press(KeyCode::Char('y'))),
        vec![Command::FinishCard]
    );
    assert!(matches!(app.popup(), Some(Popup::Unmounting)));
    send(&mut app, TaskId::SyncCard, EventKind::Finished);
    assert!(app.popup().is_none());
    assert_eq!(app.view(TaskId::SyncCard).phase, Phase::Idle);
}

#[test]
fn declining_the_unmount_closes_the_popup() {
    let mut app = app();
    app.selected = TaskId::SyncCard;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::CardSummary(CardSummary {
            to_copy: 0,
            already: 1,
        }),
    );
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncCard, EventKind::Finished);
    assert_eq!(
        app.handle_key(press(KeyCode::Char('n'))),
        Vec::<Command>::new()
    );
    assert!(app.popup().is_none());
    assert_eq!(app.view(TaskId::SyncCard).phase, Phase::Idle);
}

#[test]
fn attention_list_scrolls_and_closes() {
    let mut app = app();
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    assert!(app.hints().iter().all(|(key, _)| *key != "a"));
    for n in 0..3 {
        send(
            &mut app,
            TaskId::DownloadLink,
            EventKind::Attention(Attention {
                title: format!("Song {n}"),
                artist: "Artist".into(),
                reason: "no match".into(),
            }),
        );
    }
    assert!(app.hints().iter().any(|(key, _)| *key == "a"));
    app.handle_key(press(KeyCode::Char('a')));
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Down));
    app.handle_key(press(KeyCode::Down));
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
        EventKind::Attention(Attention {
            title: "T".into(),
            artist: String::new(),
            reason: "r".into(),
        }),
    );
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert_eq!(app.view(TaskId::DownloadLink).state.attention.len(), 1);
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    app.handle_paste("x");
    app.handle_key(press(KeyCode::Enter));
    assert_eq!(app.view(TaskId::DownloadLink).state.attention.len(), 0);
}

#[test]
fn help_opens_and_closes() {
    let mut app = app();
    app.handle_key(press(KeyCode::Char('?')));
    assert!(matches!(app.popup(), Some(Popup::Help { .. })));
    app.handle_key(press(KeyCode::Esc));
    assert!(app.popup().is_none());
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
    assert_eq!(app.view(TaskId::SyncCard).elapsed, Duration::ZERO);
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
    assert_eq!(app.selection_text(TaskId::SyncCard), "card ABCD");
}

#[test]
fn tone_reads_the_runner_lines() {
    assert_eq!(tone("412 added, 12 skipped"), Tone::Done);
    assert_eq!(tone("412 added, 3 need attention"), Tone::Attention);
    assert_eq!(tone("stopped"), Tone::Stopped);
    assert_eq!(tone("stopped: Spotify rate limit reached"), Tone::Attention);
    assert_eq!(tone("failed: yt-dlp was not found."), Tone::Failed);
}

#[test]
fn the_runners_state_replaces_the_screens() {
    let mut app = app();
    let mut state = State::default();
    state.last_run.insert(
        TaskId::SyncCard,
        LastRun {
            line: "3 copied".into(),
            at: 900,
        },
    );
    app.set_state(state);
    assert_eq!(app.state.last_run.len(), 1);
}

#[test]
fn main_panel_shows_selection_and_last_run() -> TestResult {
    let mut app = app();
    app.state.last_run.insert(
        TaskId::SyncSpotify,
        LastRun {
            line: "12 added, 3 skipped".into(),
            at: 400,
        },
    );
    let shown = screen(&app, 80, 24)?;
    assert!(shown.contains("Sync Spotify"));
    assert!(shown.contains("Download link"));
    assert!(shown.contains("Sync card"));
    assert!(shown.contains("12 added, 3 skipped"));
    assert!(shown.contains("10 min ago"));
    assert!(shown.contains("Last run  never"));
    assert!(!shown.contains("Running"));
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
    let main = lines.iter().position(|l| l.contains("Sync card"));
    assert!(matches!((main, running), (Some(m), Some(r)) if m < r));
    Ok(())
}

#[test]
fn short_screen_shows_only_rows_and_counts() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Counts(counts(10, 5, 0)),
    );
    let shown = screen(&app, 80, 8)?;
    assert!(shown.contains("Sync Spotify"));
    assert!(shown.contains("5 of 10"));
    assert!(!shown.contains("Last run"));
    assert!(!shown.contains("quit"));
    Ok(())
}

#[test]
fn every_popup_draws_at_every_size() -> TestResult {
    let popups = vec![
        Popup::SignIn(SignInState::Failed("denied".into())),
        Popup::CardPlan(Some(CardSummary {
            to_copy: 2,
            already: 5,
        })),
        Popup::UnmountConfirm,
        Popup::Unmounting,
        Popup::ConfirmQuit,
        Popup::ConfirmStop(TaskId::SyncCard),
        Popup::Error(
            TaskId::SyncCard,
            ErrorInfo {
                message: "m".repeat(200),
                advice: "a".repeat(200),
            },
        ),
        Popup::Help { scroll: 0 },
        Popup::Help { scroll: 9_999 },
        Popup::Link(Input::new("https://example.org/".repeat(10))),
        Popup::Attention {
            task: TaskId::SyncSpotify,
            cursor: 0,
        },
    ];
    for popup in popups {
        for (width, height) in [(120, 40), (60, 12), (30, 8), (1, 1), (10, 3)] {
            let mut app = app();
            app.popups = vec![popup.clone()];
            screen(&app, width, height)?;
        }
    }
    Ok(())
}

#[test]
fn list_popup_shows_counts_then_new_of_total() -> TestResult {
    let mut app = app();
    app.handle_key(press(KeyCode::Enter));
    send(&mut app, TaskId::SyncSpotify, EventKind::Lists(choices()));
    let shown = screen(&app, 100, 30)?;
    assert!(shown.contains("2,117"));
    assert!(shown.contains("Road trip"));
    send(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::NewOfTotal {
            new: 12,
            total: 2_157,
        },
    );
    let shown = screen(&app, 100, 30)?;
    assert!(shown.contains("12 new of 2,157"));
    Ok(())
}

#[test]
fn help_lists_every_key() -> TestResult {
    let mut app = app();
    app.handle_key(press(KeyCode::Char('?')));
    let shown = screen(&app, 100, 60)?;
    for group in keys::HELP {
        for (key, _) in group.keys {
            assert!(shown.contains(key), "help is missing {key}");
        }
    }
    Ok(())
}

#[test]
fn colour_is_painted_and_no_colour_is_plain() -> TestResult {
    let app = app();
    let mut terminal = Terminal::new(TestBackend::new(60, 20))?;
    terminal.draw(|frame| ui::render(frame, &app, Theme::new(true)))?;
    let painted = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .all(|cell| cell.bg != ratatui::style::Color::default());
    assert!(painted);
    terminal.draw(|frame| ui::render(frame, &app, Theme::new(false)))?;
    let plain = terminal.backend().buffer().content().iter().all(|cell| {
        cell.bg == ratatui::style::Color::default() && cell.fg == ratatui::style::Color::default()
    });
    assert!(plain);
    Ok(())
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
    app.selected = TaskId::SyncCard;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncCard,
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
    assert!(matches!(app.popup(), Some(Popup::Lists(_))));
    send(&mut app, TaskId::DownloadLink, EventKind::Finished);
    assert!(matches!(app.popup(), Some(Popup::Lists(_))));
}

#[test]
fn an_error_after_cancelling_is_still_shown() {
    let mut app = app();
    app.selected = TaskId::SyncCard;
    app.handle_key(press(KeyCode::Enter));
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::CardSummary(CardSummary {
            to_copy: 1,
            already: 0,
        }),
    );
    assert_eq!(
        app.handle_key(press(KeyCode::Esc)),
        vec![Command::Cancel(TaskId::SyncCard)]
    );
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::Headline("late".into()),
    );
    assert_eq!(app.view(TaskId::SyncCard).state.headline, "");
    send(
        &mut app,
        TaskId::SyncCard,
        EventKind::Error(info("unmount failed")),
    );
    assert!(
        matches!(app.popup(), Some(Popup::Error(_, shown)) if shown.message == "unmount failed")
    );
    assert_eq!(app.view(TaskId::SyncCard).phase, Phase::Idle);
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

#[test]
fn hints_show_only_keys_that_work_now() {
    let mut app = app();
    assert!(app.hints().contains(&("Enter", "run")));
    start_link(&mut app);
    app.selected = TaskId::DownloadLink;
    assert!(!app.hints().contains(&("Enter", "run")));
    let mut app = self::app();
    app.selected = TaskId::DownloadLink;
    app.handle_key(press(KeyCode::Enter));
    assert!(!app.hints().iter().any(|(key, _)| *key == "Enter"));
    app.handle_paste("x");
    assert!(app.hints().contains(&("Enter", "start")));
}

#[test]
fn a_narrow_footer_drops_the_moving_keys_first() -> TestResult {
    let mut app = app();
    start_link(&mut app);
    send(
        &mut app,
        TaskId::DownloadLink,
        EventKind::Attention(Attention {
            title: "T".into(),
            artist: String::new(),
            reason: "r".into(),
        }),
    );
    app.selected = TaskId::DownloadLink;
    let shown = screen(&app, 40, 24)?;
    let footer = shown.lines().last().unwrap_or_default();
    assert!(footer.contains("s stop"));
    assert!(footer.contains("? help"));
    assert!(footer.contains("q quit"));
    assert!(!footer.contains("choose"));
    Ok(())
}

#[test]
fn help_fits_forty_columns() {
    for group in keys::HELP {
        for (key, what) in group.keys {
            assert!(key.chars().count() < keys::KEY_WIDTH, "{key}");
            // 40 columns less borders, padding and the scrollbar.
            assert!(keys::KEY_WIDTH + what.chars().count() <= 35, "{what}");
        }
    }
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
