//! Publish split assignment events, deduplicated across status polls.

use mesh_llm_events::split_assignment::{SplitAssignmentStage, split_stages_event};
use mesh_llm_events::{OutputEvent, emit_event};
use std::sync::Mutex;

pub(crate) fn publish_observed_stages(statuses: &[super::StageRuntimeStatus]) {
    let stages = statuses
        .iter()
        .map(|status| SplitAssignmentStage {
            stage_index: status.stage_index,
            node_id: status.node_id.as_ref().map(ToString::to_string),
            layer_start: status.layer_start,
            layer_end: status.layer_end,
            state: crate::api::status::runtime_stage_state_label(status.state).to_string(),
        })
        .collect::<Vec<_>>();
    if let Some(event) = split_stages_event(&stages) {
        publish_split_assignment(event);
    }
}

static LAST: Mutex<Option<OutputEvent>> = Mutex::new(None);

fn publish_split_assignment(event: OutputEvent) {
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref() == Some(&event) {
        return;
    }
    *last = Some(event.clone());
    let _ = emit_event(event);
}

/// Clear the pinned assignment and forget the last one, so the next split is
/// published again even if it looks the same.
pub(crate) fn publish_split_assignment_cleared() {
    *LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let _ = emit_event(OutputEvent::SplitAssignmentCleared);
}
