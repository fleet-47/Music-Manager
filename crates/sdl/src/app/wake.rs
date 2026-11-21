//! When the loop next wakes with nothing to read (F2). One pure function decides it from the app
//! state; with nothing running and no effect playing it says "never", and the loop waits on input
//! alone.

use std::time::Duration;

use super::App;

/// About 30 frames a second while a `tachyonfx` effect plays.
pub const FRAME: Duration = Duration::from_millis(33);

/// The spinner's step.
pub const SPINNER: Duration = Duration::from_millis(100);

/// How long until the next wake, or `None` when no timer is needed.
///
/// * an effect is playing (`App::animating`): [`FRAME`];
/// * a spinner is on screen: [`SPINNER`]. That is a running function while the running panel
///   shows its detail (it spins beside the name; the same wake reads the time estimate again),
///   or a popup with a spinner (a form that is busy, a waiting popup);
/// * otherwise `None`: no timer exists.
///
/// The shortest applies. Progress bars and counts are not timed: they change only when a
/// progress event arrives.
#[must_use]
pub fn next_wake(app: &App) -> Option<Duration> {
    let detail = app.shows_detail() && app.shows_running_panel();
    let spinner_in_panel = detail && app.running().any(super::FunctionView::shows_spinner);
    let spinner_in_popup = app.top().is_some_and(super::Popup::shows_spinner);
    [
        app.animating.then_some(FRAME),
        (spinner_in_panel || spinner_in_popup).then_some(SPINNER),
    ]
    .into_iter()
    .flatten()
    .min()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use ratatui::layout::Size;
    use sdl_core::{Counts, Event, EventKind, TaskId};

    use super::*;
    use crate::app::{Clock, Confirm, Msg, Popup, Settings, Waiting};

    fn app(size: Size) -> App {
        let clock = Clock {
            instant: Instant::now(),
            epoch: 0,
        };
        App::new(Settings::default(), true, size, clock)
    }

    fn feed(app: &mut App, task: TaskId, kind: EventKind) {
        let clock = app.clock;
        let _ = app.update(Msg::Task(Event { task, kind }), clock);
    }

    fn run(app: &mut App, task: TaskId, total: Option<usize>) {
        feed(app, task, EventKind::Started);
        feed(
            app,
            task,
            EventKind::Counts(Counts {
                total,
                ..Counts::default()
            }),
        );
    }

    #[test]
    fn idle_has_no_timer() {
        assert_eq!(next_wake(&app(Size::new(100, 30))), None);
    }

    #[test]
    fn an_effect_wakes_at_thirty_frames_a_second() {
        let mut app = app(Size::new(100, 30));
        app.animating = true;
        assert_eq!(next_wake(&app), Some(FRAME));
    }

    #[test]
    fn a_running_task_with_a_known_total_still_spins_beside_its_name() {
        let mut app = app(Size::new(100, 30));
        run(&mut app, TaskId::Convert, Some(10));
        assert_eq!(next_wake(&app), Some(SPINNER));
    }

    #[test]
    fn a_spinner_on_screen_wakes_every_tenth_of_a_second() {
        let mut app = app(Size::new(100, 30));
        run(&mut app, TaskId::DownloadLink, None);
        assert_eq!(next_wake(&app), Some(SPINNER));
    }

    #[test]
    fn a_short_terminal_shows_no_spinner_and_no_clock() {
        let mut app = app(Size::new(100, 8));
        run(&mut app, TaskId::DownloadLink, None);
        assert_eq!(next_wake(&app), None);
        app.animating = true;
        assert_eq!(next_wake(&app), Some(FRAME));
    }

    #[test]
    fn a_waiting_popup_spins_and_a_question_does_not() {
        let mut app = app(Size::new(100, 30));
        app.popups.push(Popup::Confirm(Confirm::Quit));
        assert_eq!(next_wake(&app), None);
        app.popups.push(Popup::Waiting(Waiting::Browser));
        assert_eq!(next_wake(&app), Some(SPINNER));
    }

    #[test]
    fn the_shortest_timer_wins() {
        let mut app = app(Size::new(100, 30));
        run(&mut app, TaskId::Convert, Some(10));
        app.animating = true;
        assert_eq!(next_wake(&app), Some(FRAME));
    }

    #[test]
    fn a_busy_form_spins_and_a_function_at_its_form_has_no_clock() {
        let mut app = app(Size::new(100, 30));
        if let Some(view) = app.view_mut(TaskId::Convert) {
            view.begin();
        }
        let (dialog, _) = crate::dialogs::Dialog::open(TaskId::Convert, &app.settings);
        app.popups.push(Popup::Dialog(Box::new(dialog)));
        assert_eq!(next_wake(&app), Some(SPINNER));
        if let Some(Popup::Dialog(dialog)) = app.popups.last_mut() {
            dialog.form_mut().set_busy(None);
        }
        assert_eq!(next_wake(&app), None);
    }

    #[test]
    fn a_finished_task_leaves_no_timer() {
        let mut app = app(Size::new(100, 30));
        run(&mut app, TaskId::Convert, None);
        feed(&mut app, TaskId::Convert, EventKind::Finished);
        assert_eq!(next_wake(&app), None);
    }
}
