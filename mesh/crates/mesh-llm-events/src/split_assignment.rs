//! Split assignment output shared by runtime producers and the dashboard.

use crate::OutputEvent;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitAssignmentStage {
    pub stage_index: u32,
    pub node_id: Option<String>,
    pub layer_start: u32,
    pub layer_end: u32,
    pub state: String,
}

pub fn format_split_assignment(stages: &[SplitAssignmentStage]) -> Option<String> {
    if stages.is_empty() {
        return None;
    }
    let mut ordered: Vec<&SplitAssignmentStage> = stages.iter().collect();
    ordered.sort_by_key(|stage| stage.stage_index);
    let summary = ordered
        .into_iter()
        .map(format_split_stage)
        .collect::<Vec<_>>()
        .join("; ");
    Some(summary)
}

pub fn split_coordinator_event(node_id: String, model: String) -> OutputEvent {
    OutputEvent::SplitCoordinator { node_id, model }
}

/// Build both identity and display text from the same stage assignment.
pub fn split_stages_event(stages: &[SplitAssignmentStage]) -> Option<OutputEvent> {
    let summary = format_split_assignment(stages)?;
    let coordinator_id = stages
        .iter()
        .find(|stage| stage.stage_index == 0)
        .and_then(|stage| stage.node_id.clone());
    Some(OutputEvent::SplitStages {
        coordinator_id,
        summary,
    })
}

fn format_split_stage(stage: &SplitAssignmentStage) -> String {
    let node = stage
        .node_id
        .as_deref()
        .map(display_node_id)
        .unwrap_or("unassigned");
    let coordinator = if stage.stage_index == 0 {
        " coordinator"
    } else {
        ""
    };
    format!(
        "stage {} {node} layers {}-{} {}{coordinator}",
        stage.stage_index, stage.layer_start, stage.layer_end, stage.state
    )
}

/// `fmt_short` is the first 10 hex characters of the endpoint id. Stage
/// payloads store the full id, and the status peer list stores the short one.
pub fn display_node_id(node_id: &str) -> &str {
    let short_enough = node_id.len() >= 10
        && node_id
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit());
    if short_enough {
        &node_id[..10]
    } else {
        node_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_lists_stages_and_marks_stage_zero_as_coordinator() {
        let summary = format_split_assignment(&[
            SplitAssignmentStage {
                stage_index: 1,
                node_id: Some("abcdef0123456789".into()),
                layer_start: 21,
                layer_end: 35,
                state: "ready".into(),
            },
            SplitAssignmentStage {
                stage_index: 0,
                node_id: Some("1234abcdffff".into()),
                layer_start: 0,
                layer_end: 20,
                state: "loading".into(),
            },
        ])
        .expect("stages should summarize");

        assert_eq!(
            summary,
            "stage 0 1234abcdff layers 0-20 loading coordinator; stage 1 abcdef0123 layers 21-35 ready"
        );
    }

    #[test]
    fn empty_assignment_is_absent() {
        assert_eq!(format_split_assignment(&[]), None);
    }
}
