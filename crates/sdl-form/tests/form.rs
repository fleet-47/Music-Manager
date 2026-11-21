//! The form through its interface only: keys, pastes and mouse events in; values, reactions and
//! the drawn buffer out. Every mouse action has its keyboard twin in the same test.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use sdl_form::{
    Choice, Field, Focus, Form, FormStyle, HitMap, LIST_ROWS, Reaction, Target, Value, render, size,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn type_text(form: &mut Form, text: &str) {
    for c in text.chars() {
        let _ = form.key(press(KeyCode::Char(c)));
    }
}

fn mouse(kind: MouseEventKind, at: Rect) -> MouseEvent {
    MouseEvent {
        kind,
        column: at.x,
        row: at.y,
        modifiers: KeyModifiers::NONE,
    }
}

fn choices(n: usize) -> Vec<Choice> {
    (0..n)
        .map(|i| Choice::new(format!("{i}"), format!("Item {i}")))
        .collect()
}

/// A drawn form: its text, line by line, and the hit map of what was drawn.
struct Drawn {
    lines: Vec<String>,
    hits: HitMap<Target>,
}

impl Drawn {
    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn shows(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }

    fn area(&self, target: &Target) -> Result<Rect, String> {
        self.hits
            .area_of(target)
            .ok_or_else(|| format!("{target:?} was not drawn"))
    }

    fn click(&self, form: &mut Form, target: &Target) -> Result<Reaction, String> {
        let at = self.area(target)?;
        Ok(form.mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), at),
            &self.hits,
        ))
    }

    fn wheel(&self, form: &mut Form, target: &Target, down: bool) -> Result<Reaction, String> {
        let at = self.area(target)?;
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        Ok(form.mouse(mouse(kind, at), &self.hits))
    }
}

/// Whether row `Item {i}` is drawn (not `Item {i}0`: a scrollbar may follow the label).
fn shows_item(drawn: &Drawn, i: usize) -> bool {
    let label = format!("Item {i}");
    drawn.lines.iter().any(|line| {
        line.match_indices(&label).any(|(at, _)| {
            !line
                .get(at.saturating_add(label.len())..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        })
    })
}

/// Draw the form in a popup of its own size, as the screen does.
fn draw(form: &Form) -> Result<Drawn, Box<dyn std::error::Error>> {
    let wanted = size(form, 100);
    let mut hits = HitMap::new();
    let mut terminal = Terminal::new(TestBackend::new(wanted.width, wanted.height))?;
    terminal.draw(|frame| {
        let area = frame.area();
        hits = render(form, area, frame.buffer_mut(), &FormStyle::default());
    })?;
    let buffer = terminal.backend().buffer().clone();
    let width = usize::from(buffer.area.width);
    let cells: Vec<&str> = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    let lines = cells
        .chunks(width.max(1))
        .map(|row| row.concat().trim_end().to_owned())
        .collect();
    Ok(Drawn { lines, hits })
}

fn http_only(value: &str) -> Result<(), String> {
    if value.starts_with("http") {
        Ok(())
    } else {
        Err("That is not a web link".to_owned())
    }
}

#[test]
fn a_text_field_shows_its_message_until_the_value_is_acceptable() -> TestResult {
    let mut form = Form::new("Download link").field(
        Field::text("link", "Link")
            .placeholder("Paste a link")
            .validate(http_only)
            .needs("Paste a link"),
    );
    assert!(draw(&form)?.shows("Paste a link"));
    type_text(&mut form, "abc");
    let drawn = draw(&form)?;
    assert!(drawn.shows("That is not a web link"));
    assert!(!form.can_submit());
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
    for _ in 0..3 {
        let _ = form.key(press(KeyCode::Backspace));
    }
    assert_eq!(form.paste("https://a.b/c\r\n"), Reaction::Changed("link"));
    assert!(!draw(&form)?.shows("That is not a web link"));
    assert!(form.can_submit());
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Submit);
    Ok(())
}

#[test]
fn clicking_a_field_focuses_it() -> TestResult {
    let mut form = Form::new("F")
        .field(Field::text("a", "A"))
        .field(Field::text("b", "B"));
    assert_eq!(form.focus(), &Focus::Field("a"));
    draw(&form)?.click(&mut form, &Target::Field("b"))?;
    assert_eq!(form.focus(), &Focus::Field("b"));
    // Keyboard twin.
    let _ = form.key(press(KeyCode::BackTab));
    assert_eq!(form.focus(), &Focus::Field("a"));
    Ok(())
}

#[test]
fn a_dropdown_shows_its_value_opens_to_every_option_and_picks() -> TestResult {
    let build = || {
        Form::new("Convert").field(
            Field::dropdown("fmt", "Format")
                .options(choices(4))
                .when_empty("Nothing to pick"),
        )
    };
    let mut form = build();
    let closed = draw(&form)?;
    assert!(!closed.shows("Item 3"));

    // Mouse: a click opens it, a click on a row picks.
    closed.click(&mut form, &Target::Field("fmt"))?;
    let open = draw(&form)?;
    for i in 0..4 {
        assert!(open.shows(&format!("Item {i}")), "Item {i} missing");
    }
    let picked = open.click(
        &mut form,
        &Target::Row {
            field: "fmt",
            index: 2,
        },
    )?;
    assert_eq!(picked, Reaction::Changed("fmt"));
    assert_eq!(form.value("fmt"), Some(Value::Choice(Some("2".into()))));
    let after = draw(&form)?;
    assert!(after.shows("Item 2") && !after.shows("Item 3"));

    // Keyboard twin: Enter opens, Down moves, Enter picks; Esc closes without picking.
    let mut form = build();
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
    assert!(draw(&form)?.shows("Item 3"));
    let _ = form.key(press(KeyCode::Down));
    let _ = form.key(press(KeyCode::Down));
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Changed("fmt"));
    assert_eq!(form.value("fmt"), Some(Value::Choice(Some("2".into()))));
    let _ = form.key(press(KeyCode::Char(' ')));
    assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Handled);
    assert_eq!(form.value("fmt"), Some(Value::Choice(Some("2".into()))));
    Ok(())
}

#[test]
fn an_empty_dropdown_or_radio_says_why() -> TestResult {
    let form = Form::new("F").field(
        Field::radio("device", "Device")
            .when_empty("No device found. Plug one in.")
            .needs("Choose a device"),
    );
    let drawn = draw(&form)?;
    assert!(drawn.shows("No device found. Plug one in."));
    // Nothing can be chosen yet, so the missing line points at the field rather than asking.
    assert_eq!(form.missing(), vec!["No device to choose yet".to_owned()]);
    assert!(drawn.shows("No device to choose yet"));
    Ok(())
}

#[test]
fn a_radio_group_shows_every_option_and_chooses_by_click_or_arrow() -> TestResult {
    let mut form = Form::new("F").field(Field::radio("dir", "Direction").options(choices(3)));
    let drawn = draw(&form)?;
    for i in 0..3 {
        assert!(drawn.shows(&format!("Item {i}")));
    }
    drawn.click(
        &mut form,
        &Target::Row {
            field: "dir",
            index: 1,
        },
    )?;
    assert_eq!(form.value("dir"), Some(Value::Choice(Some("1".into()))));
    let _ = form.key(press(KeyCode::Down));
    assert_eq!(form.value("dir"), Some(Value::Choice(Some("2".into()))));
    Ok(())
}

#[test]
fn a_tick_list_counts_ticks_has_a_fixed_height_and_scrolls() -> TestResult {
    let short = Form::new("F").field(Field::ticks("lists", "Lists").options(choices(10)));
    let long = Form::new("F").field(Field::ticks("lists", "Lists").options(choices(34)));
    assert_eq!(size(&short, 100).height, size(&long, 100).height);

    let mut form = long;
    let drawn = draw(&form)?;
    assert!(drawn.shows("0 of 34 ticked"));
    let shown = (0..34).filter(|i| shows_item(&drawn, *i)).count();
    assert_eq!(shown, usize::from(LIST_ROWS), "{}", drawn.text());

    // Mouse: a click ticks, the wheel scrolls the list under it.
    let ticked = drawn.click(
        &mut form,
        &Target::Row {
            field: "lists",
            index: 1,
        },
    )?;
    assert_eq!(ticked, Reaction::Changed("lists"));
    assert!(draw(&form)?.shows("1 of 34 ticked"));
    let mut seen_last = false;
    for top in 0..40 {
        let drawn = draw(&form)?;
        seen_last = shows_item(&drawn, 33);
        if seen_last {
            break;
        }
        drawn.wheel(
            &mut form,
            &Target::Row {
                field: "lists",
                index: top,
            },
            true,
        )?;
    }
    assert!(seen_last, "the wheel never reached the last item");

    // Keyboard twin: Down walks the list (scrolling it), Space ticks.
    let mut form = Form::new("F").field(Field::ticks("lists", "Lists").options(choices(34)));
    for _ in 0..33 {
        let _ = form.key(press(KeyCode::Down));
    }
    assert!(shows_item(&draw(&form)?, 33));
    assert_eq!(
        form.key(press(KeyCode::Char(' '))),
        Reaction::Changed("lists")
    );
    assert_eq!(form.value("lists"), Some(Value::Ticked(vec!["33".into()])));
    Ok(())
}

#[test]
fn the_filter_ranks_matches_and_escape_clears_it_before_cancelling() -> TestResult {
    let options = vec![
        Choice::new("a", "Road trip"),
        Choice::new("b", "Liked Songs"),
        Choice::new("c", "Sleep"),
        Choice::new("d", "Likeable"),
    ];
    let mut form = Form::new("F").field(Field::ticks("lists", "Lists").options(options));
    type_text(&mut form, "liked");
    let drawn = draw(&form)?;
    assert!(drawn.shows("Liked Songs") && !drawn.shows("Sleep") && !drawn.shows("Road trip"));
    let first = drawn
        .lines
        .iter()
        .position(|l| l.contains("Liked Songs"))
        .ok_or("no match row")?;
    let other = drawn.lines.iter().position(|l| l.contains("Likeable"));
    assert!(other.is_none_or(|other| first < other), "best match first");
    // Ticking works on the filtered row and the count follows.
    let _ = form.key(press(KeyCode::Char(' ')));
    assert_eq!(form.value("lists"), Some(Value::Ticked(vec!["b".into()])));
    // Esc clears the filter, the next Esc cancels.
    assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Handled);
    let drawn = draw(&form)?;
    assert!(drawn.shows("Sleep") && drawn.shows("1 of 4 ticked"));
    assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Cancel);
    // The filter box takes a paste too.
    assert_eq!(form.paste("sleep"), Reaction::Handled);
    assert!(!draw(&form)?.shows("Road trip"));
    Ok(())
}

#[test]
fn a_toggle_switches_by_click_or_space() -> TestResult {
    let mut form = Form::new("F").field(Field::toggle("cover", "Keep cover art"));
    assert_eq!(form.value("cover"), Some(Value::Flag(false)));
    let reaction = draw(&form)?.click(&mut form, &Target::Field("cover"))?;
    assert_eq!(reaction, Reaction::Changed("cover"));
    assert_eq!(form.value("cover"), Some(Value::Flag(true)));
    assert_eq!(
        form.key(press(KeyCode::Char(' '))),
        Reaction::Changed("cover")
    );
    assert_eq!(form.value("cover"), Some(Value::Flag(false)));
    Ok(())
}

#[test]
fn a_folder_field_browses_by_click_or_key_and_shows_folders_only() -> TestResult {
    let dir = std::env::temp_dir().join(format!("sdl-form-browse-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("albums"))?;
    std::fs::write(dir.join("notes.txt"), "x")?;
    let build = || {
        let mut form = Form::new("Convert").field(Field::folder("dest", "Destination"));
        form.set("dest", Value::Text(dir.display().to_string()));
        form
    };

    let mut form = build();
    let drawn = draw(&form)?;
    drawn.click(&mut form, &Target::Browse("dest"))?;
    assert!(form.explorer().is_some());
    let open = draw(&form)?;
    assert!(open.shows("albums") && !open.shows("notes.txt"));
    // Back closes it without a change.
    assert_eq!(
        open.click(&mut form, &Target::BrowserBack)?,
        Reaction::Handled
    );
    assert!(form.explorer().is_none());
    assert_eq!(
        form.value("dest"),
        Some(Value::Text(dir.display().to_string()))
    );

    // Clicking a folder twice goes into it; Choose takes the folder shown.
    let drawn = draw(&form)?;
    drawn.click(&mut form, &Target::Browse("dest"))?;
    let open = draw(&form)?;
    let row = (0..8)
        .map(Target::BrowserRow)
        .find(|t| {
            open.hits
                .area_of(t)
                .and_then(|a| open.lines.get(usize::from(a.y)))
                .is_some_and(|l| l.contains("albums"))
        })
        .ok_or("albums row not drawn")?;
    open.click(&mut form, &row)?;
    draw(&form)?.click(&mut form, &row)?;
    let chosen = draw(&form)?.click(&mut form, &Target::BrowserChoose)?;
    assert_eq!(chosen, Reaction::Changed("dest"));
    assert_eq!(
        form.value("dest"),
        Some(Value::Text(dir.join("albums").display().to_string()))
    );

    // Keyboard twin: Tab to Browse, Enter opens, Esc closes.
    let mut form = build();
    let _ = form.key(press(KeyCode::Tab));
    assert_eq!(form.focus(), &Focus::Browse("dest"));
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Handled);
    assert!(form.explorer().is_some());
    assert_eq!(form.key(press(KeyCode::Esc)), Reaction::Handled);
    assert!(form.explorer().is_none());
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn buttons_press_by_click_or_tab_and_enter() -> TestResult {
    let mut form = Form::new("F").field(Field::text("a", "A").needs("Fill A"));
    let drawn = draw(&form)?;
    assert!(drawn.shows("Continue") && drawn.shows("Cancel") && drawn.shows("Fill A"));
    // Continue does nothing while something is missing; Cancel always works.
    assert_eq!(drawn.click(&mut form, &Target::Primary)?, Reaction::Handled);
    assert_eq!(drawn.click(&mut form, &Target::Cancel)?, Reaction::Cancel);
    form.set_focus(Focus::Field("a"));
    type_text(&mut form, "x");
    let drawn = draw(&form)?;
    assert!(!drawn.shows("Fill A"));
    assert_eq!(drawn.click(&mut form, &Target::Primary)?, Reaction::Submit);
    // Keyboard twin.
    form.set_focus(Focus::Field("a"));
    let _ = form.key(press(KeyCode::Tab));
    assert_eq!(form.focus(), &Focus::Primary);
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Submit);
    let _ = form.key(press(KeyCode::Tab));
    assert_eq!(form.focus(), &Focus::Cancel);
    assert_eq!(form.key(press(KeyCode::Enter)), Reaction::Cancel);
    Ok(())
}

#[test]
fn the_primary_label_changes_to_start() -> TestResult {
    let mut form = Form::new("F");
    form.set_primary("Start");
    let drawn = draw(&form)?;
    assert!(drawn.shows("Start") && !drawn.shows("Continue"));
    Ok(())
}

#[test]
fn tab_and_shift_tab_skip_hidden_fields() {
    let mut form = Form::new("F")
        .field(Field::toggle("a", "A"))
        .field(Field::toggle("b", "B").hidden())
        .field(Field::toggle("c", "C"));
    let _ = form.key(press(KeyCode::Tab));
    assert_eq!(form.focus(), &Focus::Field("c"));
    let _ = form.key(press(KeyCode::BackTab));
    assert_eq!(form.focus(), &Focus::Field("a"));
    form.set_visible("b", true);
    let _ = form.key(press(KeyCode::Tab));
    assert_eq!(form.focus(), &Focus::Field("b"));
    form.set_visible("b", false);
    assert_eq!(form.focus(), &Focus::Field("c"));
}

#[test]
fn options_replaced_while_open_keep_focus_openness_and_ticks() -> TestResult {
    let mut form = Form::new("F")
        .field(Field::ticks("lists", "Lists").options(choices(5)))
        .field(Field::dropdown("dev", "Device").options(choices(3)));
    let _ = form.key(press(KeyCode::Char(' ')));
    let _ = form.key(press(KeyCode::Down));
    let _ = form.key(press(KeyCode::Char(' ')));
    assert_eq!(
        form.value("lists"),
        Some(Value::Ticked(vec!["0".into(), "1".into()]))
    );
    // A list arrives with more entries while the user is on it.
    form.set_options("lists", choices(8));
    assert_eq!(form.focus(), &Focus::Field("lists"));
    assert_eq!(
        form.value("lists"),
        Some(Value::Ticked(vec!["0".into(), "1".into()]))
    );
    assert!(draw(&form)?.shows("2 of 8 ticked"));

    // A dropdown open with a choice made: new options keep it open, focused and chosen.
    form.set_focus(Focus::Field("dev"));
    let _ = form.key(press(KeyCode::Enter));
    let _ = form.key(press(KeyCode::Down));
    let _ = form.key(press(KeyCode::Enter));
    assert_eq!(form.value("dev"), Some(Value::Choice(Some("1".into()))));
    let _ = form.key(press(KeyCode::Enter));
    form.set_options("dev", choices(5));
    assert_eq!(form.focus(), &Focus::Field("dev"));
    let open = draw(&form)?;
    assert!(open.shows("Item 4"), "still open, with the new options");
    assert_eq!(form.value("dev"), Some(Value::Choice(Some("1".into()))));
    // When the chosen option goes away, nothing stale stays chosen.
    form.set_options("dev", vec![Choice::new("9", "Item 9")]);
    assert_eq!(form.value("dev"), Some(Value::Choice(None)));
    Ok(())
}

#[test]
fn the_summary_and_what_is_missing_are_drawn_and_a_block_stops_continue() -> TestResult {
    let mut form = Form::new("Convert")
        .field(Field::text("a", "A").needs("Fill A"))
        .field(Field::ticks("t", "What").needs("Tick at least one"));
    form.set_summary(Some("120 files to convert".into()));
    let drawn = draw(&form)?;
    assert!(drawn.shows("120 files to convert"));
    assert!(drawn.shows("Fill A") && drawn.shows("Tick at least one"));
    assert_eq!(form.missing().len(), 2);

    type_text(&mut form, "x");
    form.set_blocked(Some("Not enough room on the device".into()));
    let drawn = draw(&form)?;
    assert!(drawn.shows("Not enough room on the device"));
    assert!(!form.can_submit());
    form.set_busy(Some("Listing songs".into()));
    assert!(draw(&form)?.shows("Listing songs"));
    Ok(())
}

#[test]
fn a_form_wider_than_the_room_still_draws_and_records_its_buttons() -> TestResult {
    let form = Form::new("F").field(Field::text("a", "A"));
    let area = Rect::new(0, 0, 30, 12);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))?;
    let mut hits = HitMap::new();
    terminal.draw(|frame| {
        hits = render(
            &form,
            frame.area(),
            frame.buffer_mut(),
            &FormStyle::default(),
        );
    })?;
    assert!(hits.area_of(&Target::Primary).is_some());
    assert!(hits.area_of(&Target::Cancel).is_some());
    Ok(())
}

#[test]
fn an_open_dropdown_scrolls_under_the_wheel_and_closes_when_clicking_elsewhere() -> TestResult {
    let build = || {
        Form::new("F")
            .field(Field::dropdown("fmt", "Format").options(choices(12)))
            .field(Field::toggle("cover", "Cover"))
    };
    // Mouse: open, wheel down past the visible rows, the last one comes into view.
    let mut form = build();
    draw(&form)?.click(&mut form, &Target::Field("fmt"))?;
    let open = draw(&form)?;
    assert!(!shows_item(&open, 11));
    for _ in 0..11 {
        let now = draw(&form)?;
        let row = now
            .hits
            .area_of(&Target::Row {
                field: "fmt",
                index: 0,
            })
            .or_else(|| now.hits.area_of(&Target::Field("fmt")))
            .ok_or("dropdown not drawn")?;
        let _ = form.mouse(mouse(MouseEventKind::ScrollDown, row), &now.hits);
    }
    let scrolled = draw(&form)?;
    assert!(shows_item(&scrolled, 11));
    assert_eq!(form.focus(), &Focus::Field("fmt"));
    // Keyboard twin: Down moves the same highlight.
    let mut keys = build();
    let _ = keys.key(press(KeyCode::Enter));
    for _ in 0..11 {
        let _ = keys.key(press(KeyCode::Down));
    }
    assert!(shows_item(&draw(&keys)?, 11));

    // A click on another field closes the open list.
    scrolled.click(&mut form, &Target::Field("cover"))?;
    assert!(!shows_item(&draw(&form)?, 11));
    Ok(())
}

#[test]
fn a_tick_kept_for_a_list_that_went_away_is_not_counted() -> TestResult {
    let mut form = Form::new("F").field(
        Field::ticks("lists", "Lists")
            .options(choices(3))
            .needs("Tick at least one"),
    );
    form.set("lists", Value::Ticked(vec!["gone".into()]));
    let drawn = draw(&form)?;
    assert!(drawn.shows("0 of 3 ticked"));
    assert_eq!(form.missing(), vec!["Tick at least one".to_owned()]);
    assert!(!form.can_submit());
    Ok(())
}

#[test]
fn the_browser_shows_the_end_of_a_long_path() -> TestResult {
    let dir = std::env::temp_dir()
        .join(format!("sdl-form-long-{}", std::process::id()))
        .join("a-folder-name-long-enough-to-push-the-path-past-the-popup")
        .join("the-folder-shown");
    std::fs::create_dir_all(&dir)?;
    let mut form = Form::new("F").field(Field::folder("dest", "Destination"));
    form.set("dest", Value::Text(dir.display().to_string()));
    let _ = form.key(press(KeyCode::Tab));
    let _ = form.key(press(KeyCode::Enter));
    let drawn = draw(&form)?;
    let head = drawn.lines.first().ok_or("nothing drawn")?;
    assert!(head.ends_with("the-folder-shown"), "{head:?}");
    if let Some(root) = dir.ancestors().nth(2) {
        std::fs::remove_dir_all(root)?;
    }
    Ok(())
}
