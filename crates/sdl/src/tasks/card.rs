//! Sync card (F4): mount, plan, wait for go, copy. After a copy, cleanup and unmount are
//! `Runner::unmount_card`, once the owner confirms; leaving the plan unmounts here.

use sdl_core::{CardSummary, EventKind};
use tokio::sync::mpsc::UnboundedReceiver;

use super::progress::wait_for_go;
use super::runner::{Flow, TaskContext};
use super::{Command, TaskError};

/// Run the card flow, waiting on `commands` for go.
pub(super) async fn run(
    context: &TaskContext,
    mut commands: UnboundedReceiver<Command>,
) -> Result<Flow, TaskError> {
    let events = &context.events;
    events.send(EventKind::Headline("Mounting the card".to_owned()));
    let card = sdl_library::mount_card(&context.config.card_uuid).await?;
    context.set_card(Some(card.clone()));
    events.send(EventKind::Headline(
        "Checking what is on the card".to_owned(),
    ));
    let plan = sdl_library::plan_copy(&context.config.music_root, &card).await?;
    events.send(EventKind::CardSummary(CardSummary {
        to_copy: plan.to_copy.len(),
        already: plan.already,
    }));
    if !wait_for_go(&mut commands).await {
        // The owner left the plan: give the card back. The dot-folders go first, as before any
        // unmount (AGENTS.md).
        events.send(EventKind::Headline("Unmounting the card".to_owned()));
        sdl_library::finish_card(&card, &context.config.card_uuid).await?;
        context.set_card(None);
        return Ok(Flow::Cancelled);
    }
    events.send(EventKind::Headline("Copying files".to_owned()));
    sdl_library::copy_files(&plan, events).await?;
    let copied = plan.to_copy.len();
    let noun = if copied == 1 { "file" } else { "files" };
    Ok(Flow::Done {
        line: format!("{copied} {noun} copied"),
    })
}
