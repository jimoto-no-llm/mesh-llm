fn identity_test_claim(
    coordinator: EndpointId,
    term: u64,
) -> crate::inference::skippy::StageControlRequest {
    crate::inference::skippy::StageControlRequest::Claim(
        crate::inference::skippy::StageCoordinatorClaim {
            model_id: "model-a".to_string(),
            package_ref: "gguf:///model.gguf".to_string(),
            manifest_sha256: "direct-gguf:1:model.gguf".to_string(),
            topology_id: "topology-a".to_string(),
            run_id: "run-a".to_string(),
            coordinator_id: coordinator.to_string(),
            coordinator_term: term,
            participant_set_hash: "participants".to_string(),
            topology_hash: "topology".to_string(),
            lease_until_unix_ms: u64::MAX,
        },
    )
}

#[tokio::test]
async fn stage_coordinator_identity_rejects_before_control_side_effects() -> Result<()> {
    use crate::inference::skippy::StageControlRequest;

    let node = Node::new_for_tests(crate::mesh::NodeRole::Worker).await?;
    let remote = make_test_endpoint_id(0x60);
    let other = make_test_endpoint_id(0x70);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *node.stage_control_tx.lock().await = Some(tx);
    let mut load = test_stage_load_request();
    load.coordinator_id = Some(other);
    let mut malformed_claim = identity_test_claim(remote, 1);
    if let StageControlRequest::Claim(claim) = &mut malformed_claim {
        claim.coordinator_id = "invalid".to_string();
    }
    for request in [
        identity_test_claim(other, 1),
        identity_test_claim(other, u64::MAX),
        StageControlRequest::Load(load.clone()),
        StageControlRequest::LoadLocal(load),
        malformed_claim,
    ] {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::select! {
                result = Box::pin(node.authorize_resolve_and_execute_stage_control(remote, "test", request)) => {
                    assert!(result.unwrap_err().to_string().contains("coordinator_id"));
                }
                command = rx.recv() => panic!("identity rejected after control enqueue: {command:?}"),
            }
        }).await?;
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn stage_coordinator_identity_accepts_matching_claims_and_higher_terms() -> Result<()> {
    use crate::inference::skippy::{
        StageControlCommand, StageControlRequest, StageControlResponse, StageCoordinatorClaimAck,
    };

    let node = Node::new_for_tests(crate::mesh::NodeRole::Worker).await?;
    let remote = make_test_endpoint_id(0x60);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *node.stage_control_tx.lock().await = Some(tx);
    let mut fence = skippy_coordinator::ClaimFence::default();
    for (term, remote) in [(1, remote), (2, make_test_endpoint_id(0x70))] {
        let control = async {
            let StageControlCommand::Execute {
                request: StageControlRequest::Claim(claim),
                resp,
            } = rx.recv().await.unwrap()
            else {
                panic!("expected coordinator claim");
            };
            assert_eq!(claim.coordinator_id, remote.to_string());
            assert_eq!(claim.coordinator_term, term);
            assert!(matches!(
                fence.accept_claim(claim.clone(), 1),
                skippy_coordinator::ClaimDecision::Accepted { .. }
            ));
            resp.send(Ok(StageControlResponse::ClaimAccepted(
                StageCoordinatorClaimAck {
                    accepted: true,
                    claim,
                    error: None,
                },
            )))
            .unwrap();
        };
        let request = Box::pin(node.authorize_resolve_and_execute_stage_control(
            remote,
            "claim",
            identity_test_claim(remote, term),
        ));
        let (response, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(request, control)
        })
        .await?;
        assert!(matches!(response?, StageControlResponse::ClaimAccepted(ack) if ack.accepted));
    }
    Ok(())
}

#[tokio::test]
async fn stage_coordinator_identity_preserves_load_sources_and_absent_ids() -> Result<()> {
    use crate::inference::skippy::{
        StageControlCommand, StageControlRequest, StageControlResponse,
    };

    let node = Node::new_for_tests(crate::mesh::NodeRole::Worker).await?;
    let remote = make_test_endpoint_id(0x60);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *node.stage_control_tx.lock().await = Some(tx);
    for coordinator in [Some(remote), None] {
        for local_required in [false, true] {
            let mut load = test_stage_load_request();
            load.coordinator_id = coordinator;
            load.coordinator_term = if coordinator.is_some() { 11 } else { 0 };
            load.projector_path = Some("synthetic-projector.gguf".to_string());
            let expected = load.clone();
            let request = if local_required {
                StageControlRequest::LoadLocal(load)
            } else {
                StageControlRequest::Load(load)
            };
            let control = async {
                let StageControlCommand::ValidateLoad { load, resp } = rx.recv().await.unwrap()
                else {
                    panic!("expected load validation");
                };
                assert_eq!(load.coordinator_id, expected.coordinator_id);
                assert_eq!(load.coordinator_term, expected.coordinator_term);
                assert_eq!(load.model_path, expected.model_path);
                assert_eq!(load.model_id, expected.model_id);
                assert_eq!(load.projector_path, expected.projector_path);
                assert_eq!(load.package_ref, expected.package_ref);
                assert_eq!(load.bind_addr, expected.bind_addr);
                assert_eq!(load.local_source_required, local_required);
                resp.send(Some("synthetic authorization rejection".to_string()))
                    .unwrap();
            };
            let request =
                Box::pin(node.authorize_resolve_and_execute_stage_control(remote, "load", request));
            let (response, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(request, control)
            })
            .await?;
            let StageControlResponse::Ready(ready) = response? else {
                panic!("expected load rejection");
            };
            assert!(!ready.accepted);
            assert!(
                ready
                    .error
                    .unwrap()
                    .contains("synthetic authorization rejection")
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn stage_coordinator_identity_keeps_operator_local_control() -> Result<()> {
    use crate::inference::skippy::{
        StageControlCommand, StageControlRequest, StageControlResponse, StageCoordinatorClaimAck,
    };

    let node = Node::new_for_tests(crate::mesh::NodeRole::Worker).await?;
    let coordinator = make_test_endpoint_id(0x70);
    assert_ne!(node.id(), coordinator);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *node.stage_control_tx.lock().await = Some(tx);
    let control = async {
        let StageControlCommand::Execute {
            request: StageControlRequest::Claim(claim),
            resp,
        } = rx.recv().await.unwrap()
        else {
            panic!("expected local coordinator claim");
        };
        assert_eq!(claim.coordinator_id, coordinator.to_string());
        resp.send(Ok(StageControlResponse::ClaimAccepted(
            StageCoordinatorClaimAck {
                accepted: true,
                claim,
                error: None,
            },
        )))
        .unwrap();
    };
    let request = Box::pin(node.send_local_stage_control(identity_test_claim(coordinator, 1)));
    let (response, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(request, control)
    })
    .await?;
    assert!(matches!(response?, StageControlResponse::ClaimAccepted(ack) if ack.accepted));
    Ok(())
}
