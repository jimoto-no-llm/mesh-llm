use crate::inference::skippy::StageControlRequest;
use anyhow::{Context, Result};
use iroh::EndpointId;

pub(super) fn validate_coordinator_identity(
    remote: EndpointId,
    request: &StageControlRequest,
) -> Result<()> {
    let coordinator_id = match request {
        StageControlRequest::Claim(claim) => Some(
            claim
                .coordinator_id
                .parse::<EndpointId>()
                .context("invalid stage claim coordinator_id")?,
        ),
        StageControlRequest::Load(load) | StageControlRequest::LoadLocal(load) => {
            load.coordinator_id
        }
        _ => None,
    };
    anyhow::ensure!(
        coordinator_id.is_none_or(|coordinator| coordinator == remote),
        "stage coordinator_id does not match QUIC peer identity"
    );
    Ok(())
}
