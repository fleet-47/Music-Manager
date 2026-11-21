//! What the four plan-making dialogs share: the wait lines, the pending plans and the
//! Continue / Start button (rules 2 to 5 in [`super`]). It owns no form; [`Conversation::refresh`]
//! writes its state into one.

use sdl_form::Form;

/// Where a dialog is in its talk with the task.
#[derive(Debug)]
pub(super) struct Conversation {
    /// What the form says while a plan is made; `None` for a dialog with no plan (Download link and
    /// Review start at once), which then leaves the primary button to the dialog.
    plan_text: Option<&'static str>,
    /// What the form says while it waits for an offer (lists, devices, folders).
    wait: Option<&'static str>,
    asked: usize,
    answered: usize,
    /// The form changed since the last answer was sent (true at the start: nothing is planned).
    stale: bool,
    summary: Option<String>,
    starting: bool,
    headline: Option<String>,
}

impl Conversation {
    /// A dialog that asks for a plan and shows `plan_text` meanwhile; it first waits for an offer
    /// when `wait` is set.
    pub(super) fn planned(plan_text: &'static str, wait: Option<&'static str>) -> Self {
        Self {
            plan_text: Some(plan_text),
            wait,
            asked: 0,
            answered: 0,
            stale: true,
            summary: None,
            starting: false,
            headline: None,
        }
    }

    /// A dialog whose answer starts the run (Download link, Review).
    pub(super) fn direct() -> Self {
        Self {
            plan_text: None,
            ..Self::planned("", None)
        }
    }

    /// Whether the form shows a wait line: an offer or a plan is pending, or the run is starting.
    pub(super) fn is_busy(&self) -> bool {
        self.starting || self.wait.is_some() || self.asked > self.answered
    }

    /// Whether `Go` (or the answer that starts the run) was sent and `Started` has not come yet.
    pub(super) fn is_starting(&self) -> bool {
        self.starting
    }

    /// Whether the summary shown matches the form, so the button reads `Start`.
    pub(super) fn is_planned(&self) -> bool {
        self.summary.is_some()
    }

    /// Whether the form changed since the last answer was sent.
    pub(super) fn is_stale(&self) -> bool {
        self.stale
    }

    /// Whether the dialog waits for an offer.
    pub(super) fn is_waiting(&self) -> bool {
        self.wait.is_some()
    }

    /// Wait for an offer (`Some`), or stop waiting.
    pub(super) fn wait_for(&mut self, wait: Option<&'static str>) {
        self.wait = wait;
        self.forget_headline();
    }

    /// An answer is being sent: a plan is now pending, or the run is starting.
    pub(super) fn ask(&mut self) {
        if self.plan_text.is_some() {
            self.asked = self.asked.saturating_add(1);
            self.stale = false;
        } else {
            self.starting = true;
        }
    }

    /// `Go` is being sent.
    pub(super) fn start(&mut self) {
        self.starting = true;
    }

    /// A field changed: nothing shown is current any more.
    pub(super) fn changed(&mut self) {
        self.stale = true;
        self.summary = None;
    }

    /// A plan arrived. `true` when it is the current one: nothing is pending and the form did not
    /// change since, so it is shown and the button reads `Start`.
    pub(super) fn plan(&mut self, summary: String) -> bool {
        self.answered = self.answered.saturating_add(1).min(self.asked);
        let current = self.answered == self.asked && !self.stale;
        self.summary = current.then_some(summary);
        self.forget_headline();
        current
    }

    /// The task says what it is doing; shown only while the form waits.
    pub(super) fn headline(&mut self, text: &str) {
        if self.is_busy() {
            self.headline = Some(text.to_owned());
        }
    }

    fn forget_headline(&mut self) {
        if !self.is_busy() {
            self.headline = None;
        }
    }

    /// Write the wait line, the summary and (when there is a plan) the button into `form`.
    pub(super) fn refresh(&self, form: &mut Form) {
        let default = if self.starting {
            Some("Starting")
        } else {
            self.wait
                .or_else(|| (self.asked > self.answered).then_some(self.plan_text?))
        };
        form.set_busy(default.map(|text| self.headline.clone().unwrap_or_else(|| text.to_owned())));
        form.set_summary(self.summary.clone());
        if self.plan_text.is_some() {
            form.set_primary(if self.is_planned() {
                "Start"
            } else {
                "Continue"
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_current_only_when_nothing_is_pending_and_nothing_changed() {
        let mut conv = Conversation::planned("Making the plan", None);
        assert!(conv.is_stale() && !conv.is_busy());
        conv.ask();
        conv.ask();
        assert!(conv.is_busy());
        assert!(!conv.plan("first".into()), "one answer is still pending");
        assert!(conv.is_busy() && !conv.is_planned());
        assert!(conv.plan("second".into()));
        assert!(conv.is_planned() && !conv.is_busy());
    }

    #[test]
    fn a_change_while_a_plan_is_pending_makes_it_stale() {
        let mut conv = Conversation::planned("Making the plan", None);
        conv.ask();
        conv.changed();
        assert!(!conv.plan("old".into()));
        assert!(!conv.is_planned() && !conv.is_busy());
    }

    #[test]
    fn a_headline_shows_only_while_busy_and_is_forgotten_after() {
        let mut conv = Conversation::planned("Making the plan", Some("Looking for devices"));
        let mut form = Form::new("F");
        conv.headline("Reading the volumes");
        conv.refresh(&mut form);
        assert_eq!(form.busy(), Some("Reading the volumes"));
        conv.wait_for(None);
        conv.refresh(&mut form);
        assert_eq!(form.busy(), None);
        conv.headline("ignored while idle");
        conv.ask();
        conv.refresh(&mut form);
        assert_eq!(form.busy(), Some("Making the plan"));
    }

    #[test]
    fn the_button_follows_the_plan_and_a_direct_dialog_keeps_its_own() {
        let mut conv = Conversation::planned("Making the plan", None);
        let mut form = Form::new("F");
        conv.refresh(&mut form);
        assert_eq!(form.primary(), "Continue");
        conv.ask();
        assert!(conv.plan("1 file".into()));
        conv.refresh(&mut form);
        assert_eq!((form.primary(), form.summary()), ("Start", Some("1 file")));
        conv.start();
        conv.refresh(&mut form);
        assert_eq!(form.busy(), Some("Starting"));

        let mut direct = Conversation::direct();
        form.set_primary("Download 2 chosen");
        direct.refresh(&mut form);
        assert_eq!(form.primary(), "Download 2 chosen");
        direct.ask();
        direct.refresh(&mut form);
        assert_eq!(form.busy(), Some("Starting"));
    }
}
