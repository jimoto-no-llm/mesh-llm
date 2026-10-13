//! Mesh membership as `PeerJoined` / `PeerLeft` output events.
//!
//! The node signals admitted-peer changes on `peer_change_rx`. Diffing the
//! admitted peers on each signal reports every path that adds or removes a
//! peer, including removals that never emit a plugin `PeerDown` event, so the
//! console and dashboard peer list follow the actual membership.

use std::collections::BTreeMap;

use mesh_llm_events::{OutputEvent, emit_event};

use crate::mesh;

pub(super) fn spawn_peer_membership_output(node: mesh::Node) {
    tokio::spawn(async move {
        let mut peer_rx = node.peer_change_rx.clone();
        let mut known = BTreeMap::new();
        loop {
            for event in membership_events(&mut known, node.admitted_peer_labels().await) {
                let _ = emit_event(event);
            }
            if peer_rx.changed().await.is_err() {
                break;
            }
        }
    });
}

/// Events that move `known` to `current`, and the new `known` in place.
fn membership_events(
    known: &mut BTreeMap<String, Option<String>>,
    current: Vec<(String, Option<String>)>,
) -> Vec<OutputEvent> {
    let current: BTreeMap<String, Option<String>> = current.into_iter().collect();
    let left = known
        .keys()
        .filter(|peer_id| !current.contains_key(*peer_id))
        .map(|peer_id| OutputEvent::PeerLeft {
            peer_id: peer_id.clone(),
            reason: None,
        });
    let joined = current
        .iter()
        .filter(|(peer_id, _)| !known.contains_key(*peer_id))
        .map(|(peer_id, label)| OutputEvent::PeerJoined {
            peer_id: peer_id.clone(),
            label: label.clone(),
        });
    let events = left.chain(joined).collect();
    *known = current;
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, label: Option<&str>) -> (String, Option<String>) {
        (id.to_string(), label.map(str::to_string))
    }

    #[test]
    fn first_sighting_joins_and_disappearance_leaves() {
        let mut known = BTreeMap::new();

        let joined = membership_events(&mut known, vec![peer("a", Some("alpha")), peer("b", None)]);
        assert_eq!(
            joined,
            vec![
                OutputEvent::PeerJoined {
                    peer_id: "a".to_string(),
                    label: Some("alpha".to_string()),
                },
                OutputEvent::PeerJoined {
                    peer_id: "b".to_string(),
                    label: None,
                },
            ]
        );

        let left = membership_events(&mut known, vec![peer("b", None)]);
        assert_eq!(
            left,
            vec![OutputEvent::PeerLeft {
                peer_id: "a".to_string(),
                reason: None,
            }]
        );
        assert_eq!(known.len(), 1);
    }

    #[test]
    fn unchanged_or_relabelled_membership_emits_nothing() {
        let mut known = BTreeMap::new();
        membership_events(&mut known, vec![peer("a", None)]);

        assert!(membership_events(&mut known, vec![peer("a", None)]).is_empty());
        assert!(membership_events(&mut known, vec![peer("a", Some("alpha"))]).is_empty());
    }

    #[test]
    fn a_peer_that_returns_joins_again() {
        let mut known = BTreeMap::new();
        membership_events(&mut known, vec![peer("a", None)]);
        membership_events(&mut known, Vec::new());

        assert_eq!(
            membership_events(&mut known, vec![peer("a", None)]),
            vec![OutputEvent::PeerJoined {
                peer_id: "a".to_string(),
                label: None,
            }]
        );
    }
}
