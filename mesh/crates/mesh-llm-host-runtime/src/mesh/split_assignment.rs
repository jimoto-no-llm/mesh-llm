//! Operator-facing summary of a split assignment.
//!
//! The TUI has no peer table. This line is the list it can show: one clause
//! per stage, with stage 0 marked as the coordinator. Publishing is deduped so
//! status polls do not append the same sentence to the event log.

use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SplitAssignmentStage {
    pub(crate) stage_index: u32,
    pub(crate) node_id: Option<String>,
    pub(crate) layer_start: u32,
    pub(crate) layer_end: u32,
    pub(crate) state: String,
}

pub(crate) fn format_split_assignment(stages: &[SplitAssignmentStage]) -> Option<String> {
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
    if let Some(summary) = format_split_assignment(&stages) {
        publish_split_assignment(&summary);
    }
}

pub(crate) fn publish_split_assignment(summary: &str) {
    static LAST: Mutex<String> = Mutex::new(String::new());
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    if *last == summary {
        return;
    }
    *last = summary.to_string();
    super::emit_mesh_info(format!("Split stages {summary}"));
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
fn display_node_id(node_id: &str) -> &str {
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
