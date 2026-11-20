//! Every state of the screen drawn to a `TestBackend` at the sizes the spec names (120, 80, 59
//! and 40 columns; 24 and 11 rows), and clicks routed through the rectangles each draw recorded.

use std::time::Instant;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Size;
use ratatui::style::Color;
use sdl_core::{
    Attention, Counts, ErrorInfo, Event, EventKind, LastRun, RunOutcome, SignInState, TaskId,
};
use sdl_tasks::Command;

use super::theme::Theme;
use super::{panels, popups};
use crate::app::{
    App, Clock, Cmd, Confirm, Hits, MainTarget, Msg, Popup, PopupTarget, Settings, Waiting,
};
use crate::dialogs::Dialog;

/// The rows of text, what was recorded, and the terminal.
type Drawn = (Vec<String>, Hits, Terminal<TestBackend>);

type TestResult = Result<(), Box<dyn std::error::Error>>;

const COLUMNS: [u16; 4] = [120, 80, 59, 40];
const ROWS: [u16; 2] = [24, 11];

fn clock() -> Clock {
    Clock {
        instant: Instant::now(),
        epoch: 10_000,
    }
}

fn app(width: u16, height: u16) -> App {
    App::new(
        Settings::default(),
        false,
        Size::new(width, height),
        clock(),
    )
}

fn feed(app: &mut App, task: TaskId, kind: EventKind) -> Vec<Cmd> {
    app.update(Msg::Task(Event { task, kind }), clock())
}

/// Three functions running: one with a known total, one with none (a spinner), one more.
fn run_three(app: &mut App) {
    for task in [TaskId::SyncSpotify, TaskId::DownloadLink, TaskId::Convert] {
        if let Some(view) = app.view_mut(task) {
            view.begin();
        }
        feed(app, task, EventKind::Started);
        feed(app, task, EventKind::Headline("Working".into()));
    }
    feed(
        app,
        TaskId::SyncSpotify,
        EventKind::Counts(Counts {
            total: Some(2_117),
            done: 412,
            held: 3,
            ..Counts::default()
        }),
    );
}

fn with_attention(app: &mut App, count: usize) {
    let items = (0..count)
        .map(|n| {
            Attention::held(
                format!("Song number {n}"),
                "An Artist".to_owned(),
                "No YouTube Music match was close enough, so nothing was downloaded.".to_owned(),
            )
        })
        .collect();
    app.settings.state.last_run.insert(
        TaskId::SyncSpotify,
        LastRun {
            line: "9 added".into(),
            at: 9_000,
            outcome: RunOutcome::NeedsAttention,
            attention: items,
        },
    );
}

/// Draw the whole screen; the text row by row, and what was recorded.
fn draw(app: &App, theme: Theme) -> Result<Drawn, Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(app.size.width, app.size.height))?;
    let mut hits = Hits::default();
    terminal.draw(|frame| {
        let _ = panels::draw(frame, app, theme, &mut hits);
        let _ = popups::draw(frame, app, theme, &mut hits);
    })?;
    let buffer = terminal.backend().buffer();
    let width = usize::from(buffer.area.width);
    let cells: Vec<&str> = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    let rows = cells.chunks(width).map(<[&str]>::concat).collect();
    Ok((rows, hits, terminal))
}

fn text(app: &App) -> Result<String, Box<dyn std::error::Error>> {
    Ok(draw(app, Theme::new(true))?.0.join("\n"))
}

fn needs(shown: &str, words: &[&str], size: (u16, u16)) {
    for word in words {
        assert!(
            shown.contains(word),
            "{word:?} missing at {}x{}:\n{shown}",
            size.0,
            size.1
        );
    }
}

fn each_size(mut check: impl FnMut(u16, u16) -> TestResult) -> TestResult {
    for width in COLUMNS {
        for height in ROWS {
            check(width, height)?;
        }
    }
    Ok(())
}

fn click(app: &mut App, column: u16, row: u16) -> Vec<Cmd> {
    app.update(
        Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        clock(),
    )
}

#[test]
fn the_idle_main_screen_shows_every_function_and_how_to_leave_at_every_size() -> TestResult {
    each_size(|width, height| {
        let app = app(width, height);
        let (rows, hits, _) = draw(&app, Theme::new(true))?;
        let shown = rows.join("\n");
        for task in TaskId::ALL {
            needs(&shown, &[task.name()], (width, height));
        }
        needs(&shown, &["? help", "q quit"], (width, height));
        assert_eq!(hits.main.len(), 5);
        Ok(())
    })
}

#[test]
fn running_tasks_show_their_count_where_the_size_allows() -> TestResult {
    each_size(|width, height| {
        let mut app = app(width, height);
        run_three(&mut app);
        let shown = text(&app)?;
        needs(&shown, &["412 of 2,117"], (width, height));
        if height >= 12 {
            needs(&shown, &["Running", "3 need attention"], (width, height));
        } else {
            // Under 12 rows: only the function rows, the counts and one footer line.
            assert!(!shown.contains("Running"), "{shown}");
        }
        Ok(())
    })
}

#[test]
fn what_does_not_fit_in_the_running_panel_is_counted_not_dropped() -> TestResult {
    let mut app = app(59, 24);
    run_three(&mut app);
    for task in [TaskId::Review, TaskId::SyncDevice] {
        if let Some(view) = app.view_mut(task) {
            view.begin();
        }
        feed(&mut app, task, EventKind::Started);
    }
    let shown = text(&app)?;
    needs(&shown, &["412 of 2,117", "more running"], (59, 24));
    Ok(())
}

#[test]
fn help_lists_keys_and_mouse_and_can_be_closed_at_every_size() -> TestResult {
    each_size(|width, height| {
        let mut app = app(width, height);
        app.popups.push(Popup::Help { scroll: 0 });
        let (rows, hits, _) = draw(&app, Theme::new(true))?;
        let shown = rows.join("\n");
        needs(
            &shown,
            &["Help", "Main screen", "[Esc] Close"],
            (width, height),
        );
        assert!(hits.popup.area_of(&PopupTarget::Close).is_some());
        assert!(hits.popup.area_of(&PopupTarget::Scrollbar).is_some());
        Ok(())
    })
}

#[test]
fn help_scrolled_to_the_end_shows_the_mouse_actions() -> TestResult {
    let mut app = app(80, 24);
    app.popups.push(Popup::Help { scroll: usize::MAX });
    let shown = text(&app)?;
    needs(&shown, &["Mouse", "wheel", "drag"], (80, 24));
    Ok(())
}

#[test]
fn the_attention_list_wraps_reasons_and_shows_a_close_button() -> TestResult {
    each_size(|width, height| {
        let mut app = app(width, height);
        with_attention(&mut app, 4);
        app.popups.push(Popup::Attention {
            task: TaskId::SyncSpotify,
            scroll: 0,
        });
        let (rows, hits, _) = draw(&app, Theme::new(true))?;
        let shown = rows.join("\n");
        needs(
            &shown,
            &["needs attention", "Song number 0", "[Esc] Close"],
            (width, height),
        );
        assert!(hits.popup.area_of(&PopupTarget::Item(0)).is_some());
        // A reason longer than the popup is wrapped: its end shows on a later row.
        if height >= 24 {
            needs(&shown, &["downloaded."], (width, height));
        }
        Ok(())
    })
}

#[test]
fn an_empty_attention_list_says_so() -> TestResult {
    let mut app = app(80, 24);
    app.popups.push(Popup::Attention {
        task: TaskId::Convert,
        scroll: 0,
    });
    needs(&text(&app)?, &["Nothing needs attention."], (80, 24));
    Ok(())
}

#[test]
fn questions_name_their_keys_at_every_size() -> TestResult {
    let questions = [
        (Confirm::Stop(TaskId::Convert), "Stop Convert?"),
        (Confirm::Quit, "Quit and stop"),
        (
            Confirm::Eject {
                device: "SNOWSKY".into(),
            },
            "empties the device's Trash",
        ),
        (Confirm::Browser, "opens your browser"),
    ];
    each_size(|width, height| {
        for (question, words) in &questions {
            let mut app = app(width, height);
            app.popups.push(Popup::Confirm(question.clone()));
            let (rows, hits, _) = draw(&app, Theme::new(true))?;
            let shown = rows.join("\n");
            needs(&shown, &["[y]", "[n]"], (width, height));
            // Wrapped text can split a phrase across rows: look for its first word.
            let first = words.split(' ').next().unwrap_or(words);
            needs(&shown, &[first], (width, height));
            assert!(hits.popup.area_of(&PopupTarget::Yes).is_some());
            assert!(hits.popup.area_of(&PopupTarget::No).is_some());
        }
        Ok(())
    })
}

#[test]
fn errors_and_waiting_popups_show_what_happened_and_the_way_out() -> TestResult {
    each_size(|width, height| {
        let mut app = app(width, height);
        app.popups.push(Popup::Error(ErrorInfo {
            message: "yt-dlp was not found on PATH.".into(),
            advice: "Start sdl with mise run tui.".into(),
        }));
        let shown = text(&app)?;
        needs(
            &shown,
            &["went wrong", "yt-dlp", "[Esc] Close"],
            (width, height),
        );
        let mut app = self::app(width, height);
        app.popups.push(Popup::Waiting(Waiting::Browser));
        needs(&text(&app)?, &["Waiting", "[Esc] Cancel"], (width, height));
        app.popups
            .push(Popup::Waiting(Waiting::Eject("DISC".into())));
        let (rows, hits, _) = draw(&app, Theme::new(true))?;
        needs(&rows.join("\n"), &["Ejecting DISC"], (width, height));
        assert!(hits.popup.is_empty(), "nothing to click while ejecting");
        Ok(())
    })
}

#[test]
fn every_dialog_draws_inside_the_terminal_at_every_size() -> TestResult {
    each_size(|width, height| {
        for task in TaskId::ALL {
            let mut app = app(width, height);
            let (dialog, _) = Dialog::open(task, &app.settings);
            app.popups.push(Popup::Dialog(Box::new(dialog)));
            let (rows, hits, _) = draw(&app, Theme::new(true))?;
            assert!(!hits.form.is_empty(), "{} at {width}x{height}", task.name());
            // The footer row stays the footer: the popup never covers the keys that work.
            let footer = rows.last().ok_or("no rows")?;
            assert!(footer.contains("Esc"), "{footer:?} at {width}x{height}");
        }
        Ok(())
    })
}

#[test]
fn without_colour_nothing_is_painted() -> TestResult {
    let mut app = app(100, 30);
    run_three(&mut app);
    app.popups.push(Popup::Help { scroll: 0 });
    let (_, _, terminal) = draw(&app, Theme::new(false))?;
    for cell in terminal.backend().buffer().content() {
        assert_eq!(cell.fg, Color::Reset, "{}", cell.symbol());
        assert_eq!(cell.bg, Color::Reset, "{}", cell.symbol());
    }
    let (_, _, painted) = draw(&app, Theme::new(true))?;
    assert!(
        painted
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.bg != Color::Reset)
    );
    assert_eq!(Theme::new(false).base(), None);
    Ok(())
}

#[test]
fn the_popup_rectangle_is_the_frame_that_was_drawn() -> TestResult {
    let mut app = app(100, 30);
    app.popups.push(Popup::Confirm(Confirm::Quit));
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut hits = Hits::default();
    let mut frame_rect = None;
    terminal.draw(|frame| {
        frame_rect = popups::draw(frame, &app, Theme::new(true), &mut hits);
    })?;
    let rect = frame_rect.ok_or("no popup rectangle")?;
    let buffer = terminal.backend().buffer();
    let corner = buffer.cell((rect.x, rect.y)).ok_or("no corner cell")?;
    assert!(
        matches!(corner.symbol(), "┌" | "╭" | "┏" | "╔"),
        "{}",
        corner.symbol()
    );
    Ok(())
}

#[test]
fn clicking_a_function_row_chooses_it_and_a_second_click_runs_it() -> TestResult {
    let mut app = app(100, 30);
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    let area = app
        .hits
        .main
        .area_of(&MainTarget::Row(TaskId::DownloadLink))
        .ok_or("no row")?;
    assert_eq!(click(&mut app, area.x, area.y), Vec::new());
    assert_eq!(app.selected, TaskId::DownloadLink);
    assert!(app.popups.is_empty());
    assert_eq!(
        click(&mut app, area.x, area.y),
        vec![Cmd::Start(TaskId::DownloadLink)]
    );
    assert!(matches!(app.top(), Some(Popup::Dialog(_))));
    Ok(())
}

#[test]
fn clicking_a_running_task_chooses_its_function() -> TestResult {
    let mut app = app(100, 30);
    run_three(&mut app);
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    let area = app
        .hits
        .main
        .area_of(&MainTarget::Running(TaskId::Convert))
        .ok_or("no running task")?;
    let _ = click(&mut app, area.x, area.y);
    assert_eq!(app.selected, TaskId::Convert);
    Ok(())
}

#[test]
fn buttons_answer_questions_and_a_click_outside_changes_nothing() -> TestResult {
    let mut app = app(100, 30);
    app.popups.push(Popup::Confirm(Confirm::Browser));
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    assert_eq!(click(&mut app, 0, 0), Vec::new());
    assert_eq!(app.popups.len(), 1);
    let yes = app.hits.popup.area_of(&PopupTarget::Yes).ok_or("no yes")?;
    assert_eq!(
        click(&mut app, yes.x, yes.y),
        vec![Cmd::Send(TaskId::SyncSpotify, Command::Go)]
    );
    assert!(app.popups.is_empty());
    Ok(())
}

#[test]
fn the_waiting_popup_cancel_button_sends_cancel() -> TestResult {
    let mut app = app(100, 30);
    feed(
        &mut app,
        TaskId::SyncSpotify,
        EventKind::SignIn(SignInState::WaitingForBrowser),
    );
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    let cancel = app
        .hits
        .popup
        .area_of(&PopupTarget::No)
        .ok_or("no cancel")?;
    assert_eq!(
        click(&mut app, cancel.x, cancel.y),
        vec![Cmd::Send(TaskId::SyncSpotify, Command::Cancel)]
    );
    Ok(())
}

#[test]
fn the_close_button_closes_every_popup_that_has_one() -> TestResult {
    for popup in [
        Popup::Help { scroll: 0 },
        Popup::Attention {
            task: TaskId::Convert,
            scroll: 0,
        },
        Popup::Error(ErrorInfo {
            message: "m".into(),
            advice: "a".into(),
        }),
    ] {
        let mut app = app(60, 14);
        app.popups.push(popup);
        let (_, hits, _) = draw(&app, Theme::new(true))?;
        app.hits = hits;
        let close = app
            .hits
            .popup
            .area_of(&PopupTarget::Close)
            .ok_or("no close")?;
        let _ = click(&mut app, close.x, close.y);
        assert!(app.popups.is_empty());
    }
    Ok(())
}

#[test]
fn the_wheel_scrolls_a_long_list_and_a_click_on_an_item_brings_it_up() -> TestResult {
    let mut app = app(80, 24);
    with_attention(&mut app, 12);
    app.popups.push(Popup::Attention {
        task: TaskId::SyncSpotify,
        scroll: 0,
    });
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    let wheel = |app: &mut App, kind| {
        app.update(
            Msg::Mouse(MouseEvent {
                kind,
                column: 5,
                row: 5,
                modifiers: KeyModifiers::NONE,
            }),
            clock(),
        )
    };
    let _ = wheel(&mut app, MouseEventKind::ScrollDown);
    assert!(matches!(
        app.top(),
        Some(Popup::Attention { scroll: 1, .. })
    ));
    let _ = wheel(&mut app, MouseEventKind::ScrollUp);
    let _ = wheel(&mut app, MouseEventKind::ScrollUp);
    assert!(matches!(
        app.top(),
        Some(Popup::Attention { scroll: 0, .. })
    ));
    let second = app
        .hits
        .popup
        .area_of(&PopupTarget::Item(1))
        .ok_or("no item")?;
    let _ = click(&mut app, second.x, second.y);
    assert!(matches!(app.top(), Some(Popup::Attention { scroll, .. }) if *scroll > 0));
    Ok(())
}

#[test]
fn the_scrollbar_moves_the_list() -> TestResult {
    let mut app = app(80, 24);
    with_attention(&mut app, 12);
    app.popups.push(Popup::Attention {
        task: TaskId::SyncSpotify,
        scroll: 0,
    });
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    let bar = app
        .hits
        .popup
        .area_of(&PopupTarget::Scrollbar)
        .ok_or("no bar")?;
    let _ = click(&mut app, bar.x, bar.bottom().saturating_sub(1));
    assert!(matches!(app.top(), Some(Popup::Attention { scroll, .. }) if *scroll > 0));
    Ok(())
}

#[test]
fn a_dialog_gets_its_clicks_through_its_form_rectangles() -> TestResult {
    let mut app = app(100, 30);
    let (dialog, _) = Dialog::open(TaskId::DownloadLink, &app.settings);
    app.popups.push(Popup::Dialog(Box::new(dialog)));
    let (_, hits, _) = draw(&app, Theme::new(true))?;
    app.hits = hits;
    assert!(app.hits.form.area_of(&sdl_form::Target::Cancel).is_some());
    // A click outside the popup reaches nobody: the dialog stays and nothing is sent.
    assert_eq!(click(&mut app, 0, 0), Vec::new());
    assert_eq!(app.popups.len(), 1);
    Ok(())
}

#[test]
fn a_mouse_move_is_not_an_event_the_screen_draws_for() {
    let moved = crossterm::event::Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::NONE,
    });
    assert!(super::from_terminal(moved).is_none());
}
