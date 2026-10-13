//! Elected split coordinator, visible before stage assignment exists.
//!
//! `runtime.stages` stays empty until the coordinator plans the split, so the
//! standby nodes from that election cannot be identified from stage 0.

use crate::mesh::SplitCoordinatorObservation;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct SplitCoordinatorPayload {
    /// Full endpoint id. Stage rows use this same form.
    pub(crate) node_id: String,
    /// Form used by `/api/status` `node_id` and `peers[].id`.
    pub(crate) short_node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hostname: Option<String>,
    pub(crate) elected_at_unix_ms: u64,
    pub(crate) is_self: bool,
}

pub(crate) fn split_coordinator_payload(
    observation: &SplitCoordinatorObservation,
    self_short_node_id: &str,
    self_hostname: Option<&str>,
    peers: &[CoordinatorPeerHostname<'_>],
) -> SplitCoordinatorPayload {
    let is_self = observation.short_node_id == self_short_node_id;
    let hostname = if is_self {
        self_hostname.map(str::to_string)
    } else {
        peers.iter().find_map(|peer| {
            (peer.short_node_id == observation.short_node_id).then(|| peer.hostname.to_string())
        })
    };
    SplitCoordinatorPayload {
        node_id: observation.node_id.clone(),
        short_node_id: observation.short_node_id.clone(),
        hostname,
        elected_at_unix_ms: observation.elected_at_unix_ms,
        is_self,
    }
}

pub(crate) struct CoordinatorPeerHostname<'a> {
    pub(crate) short_node_id: &'a str,
    pub(crate) hostname: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> SplitCoordinatorObservation {
        SplitCoordinatorObservation {
            node_id: "abcdef0123456789abcdef0123456789".into(),
            short_node_id: "abcdef0123".into(),
            elected_at_unix_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn remote_coordinator_uses_the_peer_hostname() {
        let payload = split_coordinator_payload(
            &observation(),
            "1111111111",
            Some("self.local"),
            &[CoordinatorPeerHostname {
                short_node_id: "abcdef0123",
                hostname: "big.local",
            }],
        );

        assert_eq!(payload.hostname.as_deref(), Some("big.local"));
        assert!(!payload.is_self);
        assert_eq!(payload.elected_at_unix_ms, 1_700_000_000_000);
    }

    #[test]
    fn self_coordinator_uses_the_local_hostname() {
        let payload =
            split_coordinator_payload(&observation(), "abcdef0123", Some("self.local"), &[]);

        assert!(payload.is_self);
        assert_eq!(payload.hostname.as_deref(), Some("self.local"));
    }
}
