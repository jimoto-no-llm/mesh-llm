//! Restore the pinned assignment from structured runtime output.

use super::OutputEvent;
use super::state::DashboardState;
use mesh_llm_events::split_assignment::display_node_id;

impl DashboardState {
    pub(super) fn note_split_assignment(&mut self, event: &OutputEvent) {
        match event {
            OutputEvent::Startup { .. } | OutputEvent::SplitAssignmentCleared => {
                self.split_assignment = None;
                self.split_coordinator_id = None;
            }
            OutputEvent::SplitCoordinator { node_id, .. } => {
                if self.split_coordinator_id.as_ref() == Some(node_id) {
                    return;
                }
                self.split_coordinator_id = Some(node_id.clone());
                self.split_assignment = Some(format!("coordinator {}", display_node_id(node_id)));
            }
            OutputEvent::SplitStages {
                coordinator_id,
                summary,
            } => {
                self.split_coordinator_id = coordinator_id.clone();
                self.split_assignment = Some(summary.clone());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::dashboard::DashboardAction;
    use crate::output::rendering::split_assignment_lines;
    use mesh_llm_events::split_assignment::{
        SplitAssignmentStage, split_coordinator_event, split_stages_event,
    };

    #[test]
    fn cleared_event_removes_the_pinned_assignment() {
        let mut state = DashboardState::default();
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            "abcdef0123456789".into(),
            "demo".into(),
        )));
        assert!(state.split_assignment.is_some());

        state.reduce(DashboardAction::OutputEvent(
            OutputEvent::SplitAssignmentCleared,
        ));
        assert_eq!(state.split_assignment, None);
        assert_eq!(state.split_coordinator_id, None);
    }

    #[test]
    fn producer_events_preserve_stages_and_follow_the_exact_coordinator() {
        let coordinator = "abcdef0123456789";
        let worker = "1234abcdffff9876";
        let mut state = DashboardState::default();
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            coordinator.into(),
            "demo".into(),
        )));
        assert_eq!(
            state.split_assignment.as_deref(),
            Some("coordinator abcdef0123")
        );

        // Deliberately unordered: the producer chooses stage zero, not the first row.
        let event = split_stages_event(&[
            SplitAssignmentStage {
                stage_index: 1,
                node_id: Some(worker.into()),
                layer_start: 21,
                layer_end: 35,
                state: "loading".into(),
            },
            SplitAssignmentStage {
                stage_index: 0,
                node_id: Some(coordinator.into()),
                layer_start: 0,
                layer_end: 20,
                state: "ready".into(),
            },
        ])
        .expect("nonempty stages");
        assert_eq!(event.event_name(), "split_stages");
        state.reduce(DashboardAction::OutputEvent(event));
        let stages = "stage 0 abcdef0123 layers 0-20 ready coordinator; stage 1 1234abcdff layers 21-35 loading";
        assert_eq!(state.split_assignment.as_deref(), Some(stages));
        assert_eq!(state.split_coordinator_id.as_deref(), Some(coordinator));
        assert_eq!(split_assignment_lines(stages).len(), 2);

        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            coordinator.into(),
            "demo".into(),
        )));
        assert_eq!(state.split_assignment.as_deref(), Some(stages));

        // A worker occurs in the display summary but is a different coordinator.
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            worker.into(),
            "demo".into(),
        )));
        assert_eq!(
            state.split_assignment.as_deref(),
            Some("coordinator 1234abcdff")
        );
        assert_eq!(state.split_coordinator_id.as_deref(), Some(worker));

        // Display wording need not contain either identity or stage syntax.
        state.reduce(DashboardAction::OutputEvent(OutputEvent::SplitStages {
            coordinator_id: Some(worker.into()),
            summary: "Workers assigned; coordinator ready".into(),
        }));
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            worker.into(),
            "demo".into(),
        )));
        assert_eq!(
            state.split_assignment.as_deref(),
            Some("Workers assigned; coordinator ready")
        );

        // Identical display prefixes must not hide a change of full identity.
        let next = "1234abcdff000000";
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            next.into(),
            "demo".into(),
        )));
        assert_eq!(state.split_coordinator_id.as_deref(), Some(next));
        assert_eq!(
            state.split_assignment.as_deref(),
            Some("coordinator 1234abcdff")
        );

        state.reduce(DashboardAction::OutputEvent(OutputEvent::Startup {
            version: "0.78.0".into(),
            message: None,
        }));
        assert_eq!(state.split_assignment, None);
        assert_eq!(state.split_coordinator_id, None);
    }

    #[test]
    fn info_wording_cannot_change_the_pinned_assignment() {
        let mut state = DashboardState::default();
        state.reduce(DashboardAction::OutputEvent(split_coordinator_event(
            "abcdef0123456789".into(),
            "demo".into(),
        )));
        for message in [
            "Split stages misleading",
            "Split runtime coordinator is wrong; waiting",
        ] {
            state.reduce(DashboardAction::OutputEvent(OutputEvent::Info {
                message: message.into(),
                context: None,
            }));
        }
        assert_eq!(
            state.split_assignment.as_deref(),
            Some("coordinator abcdef0123")
        );
        assert_eq!(
            state.split_coordinator_id.as_deref(),
            Some("abcdef0123456789")
        );
        assert_eq!(split_stages_event(&[]), None);
    }
}
