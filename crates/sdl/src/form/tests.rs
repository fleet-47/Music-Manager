use super::*;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn space() -> KeyEvent {
    press(KeyCode::Char(' '))
}

fn options(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn ticks(names: &[&str]) -> Vec<Tick> {
    names.iter().map(|name| Tick::new(*name, false)).collect()
}

/// Lists, a format choice, a toggle and a link: one of each row kind.
fn form() -> Form {
    Form::new(
        "Test",
        vec![
            Row::ticks("lists", "Lists", ticks(&["A", "B", "C"])).needs("Tick a list"),
            Row::choice("format", "Format", options(&["One", "Two", "Three"]), 0),
            Row::toggle("remember", "Remember", false),
            Row::text("link", "Link", "").needs("Paste a link"),
        ],
    )
}

#[test]
fn up_and_down_walk_through_a_tick_list_before_leaving_it() {
    let mut form = form();
    assert_eq!(form.focused(), Some("lists"));
    form.handle_key(press(KeyCode::Down));
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("lists"));
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("format"));
    // Coming back up lands on the list's last entry, not its first.
    form.handle_key(press(KeyCode::Up));
    assert_eq!(form.focused(), Some("lists"));
    form.handle_key(space());
    assert_eq!(form.ticked("lists"), vec![2]);
}

#[test]
fn focus_stops_at_the_first_and_last_row() {
    let mut form = form();
    form.handle_key(press(KeyCode::Up));
    assert_eq!(form.focused(), Some("lists"));
    for _ in 0..10 {
        form.handle_key(press(KeyCode::Down));
    }
    assert_eq!(form.focused(), Some("link"));
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("link"));
}

#[test]
fn left_and_right_cycle_a_choice_and_wrap() {
    let mut form = form();
    form.focus_on("format");
    assert_eq!(
        form.handle_key(press(KeyCode::Right)),
        Step::Changed("format")
    );
    assert_eq!(form.choice("format"), 1);
    form.handle_key(press(KeyCode::Right));
    form.handle_key(press(KeyCode::Right));
    assert_eq!(form.choice("format"), 0);
    form.handle_key(press(KeyCode::Left));
    assert_eq!(form.choice("format"), 2);
    // Space is "change this" on a choice too.
    form.handle_key(space());
    assert_eq!(form.choice("format"), 0);
}

#[test]
fn a_choice_with_one_option_does_not_report_a_change() {
    let mut form = Form::new("T", vec![Row::choice("only", "Only", options(&["x"]), 0)]);
    assert_eq!(form.handle_key(press(KeyCode::Right)), Step::Idle);
}

#[test]
fn space_ticks_and_unticks_the_entry_under_the_cursor() {
    let mut form = form();
    assert_eq!(form.handle_key(space()), Step::Changed("lists"));
    form.handle_key(press(KeyCode::Down));
    form.handle_key(space());
    assert_eq!(form.ticked("lists"), vec![0, 1]);
    form.handle_key(space());
    assert_eq!(form.ticked("lists"), vec![0]);
    // Left and Right do nothing in a tick list.
    assert_eq!(form.handle_key(press(KeyCode::Right)), Step::Idle);
}

#[test]
fn a_toggle_flips_with_space_and_with_left_and_right() {
    let mut form = form();
    form.focus_on("remember");
    assert!(!form.toggle("remember"));
    form.handle_key(space());
    assert!(form.toggle("remember"));
    form.handle_key(press(KeyCode::Left));
    assert!(!form.toggle("remember"));
    assert_eq!(
        form.handle_key(press(KeyCode::Right)),
        Step::Changed("remember")
    );
}

#[test]
fn a_text_row_takes_typing_including_keys_the_form_uses_elsewhere() {
    let mut form = form();
    form.focus_on("link");
    for c in "q a?".chars() {
        assert_eq!(
            form.handle_key(press(KeyCode::Char(c))),
            Step::Changed("link")
        );
    }
    assert_eq!(form.text("link"), "q a?");
    form.handle_key(ctrl('u'));
    assert_eq!(form.text("link"), "");
}

#[test]
fn a_paste_drops_newlines_and_never_submits() {
    let mut form = form();
    form.focus_on("link");
    let step = form.paste("https://a.example/x\nhttps://b.example/y\r\n");
    assert_eq!(step, Step::Changed("link"));
    assert_eq!(form.text("link"), "https://a.example/xhttps://b.example/y");
    // A paste on a row that takes no text changes nothing.
    form.focus_on("format");
    assert_eq!(form.paste("abc"), Step::Idle);
}

#[test]
fn hidden_rows_are_skipped_and_do_not_count_toward_completeness() {
    let mut form = form();
    form.set_visible("format", false);
    form.set_visible("link", false);
    form.handle_key(press(KeyCode::Down));
    form.handle_key(press(KeyCode::Down));
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("remember"));
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("remember"));
    // The text row is hidden, so its need no longer holds the form back.
    form.focus_on("lists");
    form.handle_key(space());
    assert!(form.is_complete());
    form.set_visible("link", true);
    assert_eq!(form.missing().as_deref(), Some("Paste a link"));
}

#[test]
fn hiding_the_focused_row_moves_focus_to_a_visible_one() {
    let mut form = form();
    form.focus_on("link");
    form.set_visible("link", false);
    assert_eq!(form.focused(), Some("lists"));
    assert!(!form.is_shown("link"));
}

#[test]
fn enter_continues_only_when_complete_and_otherwise_jumps_to_what_is_missing() {
    let mut form = form();
    form.focus_on("remember");
    assert_eq!(form.handle_key(press(KeyCode::Enter)), Step::Idle);
    assert_eq!(form.focused(), Some("lists"));
    assert_eq!(form.missing().as_deref(), Some("Tick a list"));
    form.handle_key(space());
    assert_eq!(form.handle_key(press(KeyCode::Enter)), Step::Idle);
    assert_eq!(form.focused(), Some("link"));
    form.paste("https://example.org/");
    assert_eq!(form.handle_key(press(KeyCode::Enter)), Step::Submit);
}

#[test]
fn blank_text_is_incomplete() {
    let mut form = form();
    form.focus_on("link");
    form.paste("   ");
    form.focus_on("lists");
    form.handle_key(space());
    assert_eq!(form.missing().as_deref(), Some("Paste a link"));
}

#[test]
fn a_choice_with_no_options_and_an_empty_or_loading_list_are_incomplete() {
    let mut form = Form::new(
        "T",
        vec![
            Row::choice("device", "Device", Vec::new(), 0)
                .needs("Plug in a device")
                .when_empty("No device found."),
            Row::ticks("folders", "Folders", Vec::new()).when_empty("No folders here."),
        ],
    );
    assert_eq!(form.missing().as_deref(), Some("Plug in a device"));
    form.set_options("device", options(&["Card"]), 0);
    assert_eq!(form.missing().as_deref(), Some("No folders here."));
    form.set_ticks("folders", ticks(&["Music"]));
    assert!(form.is_complete());
    form.set_loading("folders", true);
    assert_eq!(form.missing().as_deref(), Some("Wait for the list"));
    form.set_ticks("folders", ticks(&["Music"]));
    assert!(form.is_complete());
}

#[test]
fn hints_offer_enter_only_when_complete_and_say_what_is_missing_otherwise() {
    let mut form = form();
    let enter = crate::keys::FORM_CONTINUE;
    let incomplete = form.hints(Some(enter));
    assert!(!incomplete.contains(&enter.hint()));
    assert_eq!(
        incomplete.last(),
        Some(&Hint::Missing("Tick a list".into()))
    );
    assert!(incomplete.contains(&crate::keys::CANCEL.hint()));
    form.handle_key(space());
    form.focus_on("link");
    form.paste("x");
    let complete = form.hints(Some(enter));
    assert!(complete.contains(&enter.hint()));
    assert!(!complete.iter().any(|hint| matches!(hint, Hint::Missing(_))));
    // At a step where Enter does nothing, it is not offered and nothing is "missing".
    let none = form.hints(None);
    assert!(!none.contains(&enter.hint()));
}

#[test]
fn hints_follow_the_focused_row() {
    let mut form = form();
    assert!(form.hints(None).contains(&crate::keys::FORM_TICK.hint()));
    form.focus_on("format");
    assert!(form.hints(None).contains(&crate::keys::FORM_CHANGE.hint()));
    form.focus_on("remember");
    assert!(form.hints(None).contains(&crate::keys::FORM_SWITCH.hint()));
    form.focus_on("link");
    let hints = form.hints(None);
    assert!(!hints.contains(&crate::keys::FORM_CHANGE.hint()));
    assert!(hints.contains(&crate::keys::FORM_MOVE.hint()));
}

#[test]
fn a_locked_form_answers_only_enter_and_escape() {
    let mut form = form();
    form.set_locked(true);
    assert_eq!(form.handle_key(space()), Step::Idle);
    assert_eq!(form.ticked("lists"), Vec::<usize>::new());
    form.handle_key(press(KeyCode::Down));
    assert_eq!(form.focused(), Some("lists"));
    assert_eq!(form.paste("x"), Step::Idle);
    assert_eq!(form.handle_key(press(KeyCode::Esc)), Step::Cancel);
    assert!(form.is_locked());
    assert!(!form.hints(None).contains(&crate::keys::FORM_TICK.hint()));
}

#[test]
fn escape_cancels() {
    let mut form = form();
    assert_eq!(form.handle_key(press(KeyCode::Esc)), Step::Cancel);
}

#[test]
fn r_on_a_refreshable_choice_asks_to_look_again() {
    let mut form = Form::new(
        "T",
        vec![
            Row::choice("device", "Device", Vec::new(), 0).refreshable(),
            Row::text("t", "Text", ""),
        ],
    );
    assert_eq!(
        form.handle_key(press(KeyCode::Char('r'))),
        Step::Refresh("device")
    );
    assert!(form.hints(None).contains(&crate::keys::REFRESH.hint()));
    // The same key is only text in a text row.
    form.focus_on("t");
    assert_eq!(
        form.handle_key(press(KeyCode::Char('r'))),
        Step::Changed("t")
    );
}

#[test]
fn replacing_options_and_ticks_keeps_positions_in_range() {
    let mut form = form();
    form.focus_on("format");
    form.set_options("format", options(&["Only"]), 5);
    assert_eq!(form.choice("format"), 0);
    form.set_choice("format", 9);
    assert_eq!(form.choice("format"), 0);
    form.set_ticks("lists", vec![Tick::new("X", true).detail("9")]);
    assert_eq!(form.ticked("lists"), vec![0]);
    assert_eq!(
        form.ticks("lists")
            .first()
            .and_then(|t| t.detail.as_deref()),
        Some("9")
    );
}

#[test]
fn unknown_rows_read_as_empty() {
    let form = form();
    assert_eq!(form.text("nope"), "");
    assert_eq!(form.choice("nope"), 0);
    assert!(!form.toggle("nope"));
    assert_eq!(form.ticks("nope"), []);
    assert_eq!(form.title(), "Test");
}

#[test]
fn the_summary_is_kept_until_replaced() {
    let mut form = form();
    assert_eq!(form.summary(), &Summary::None);
    form.set_summary(Summary::Warn("no room".into()));
    assert_eq!(form.summary(), &Summary::Warn("no room".into()));
}
