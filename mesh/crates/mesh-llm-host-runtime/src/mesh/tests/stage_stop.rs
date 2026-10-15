async fn send_test_stage_stop(
    node: &Node,
    remote: bool,
    stop: crate::inference::skippy::StageStopRequest,
) -> Result<crate::inference::skippy::StageControlResponse> {
    let request = crate::inference::skippy::StageControlRequest::Stop(stop);
    if remote {
        Box::pin(node.authorize_resolve_and_execute_stage_control(
            make_test_endpoint_id(0x92),
            "stop",
            request,
        ))
        .await
    } else {
        Box::pin(node.send_local_stage_control(request)).await
    }
}

async fn respond_to_test_stage_stop(
    node: &Node,
    remote: bool,
    stop: crate::inference::skippy::StageStopRequest,
    response: Result<crate::inference::skippy::StageControlResponse>,
) -> Result<crate::inference::skippy::StageControlResponse> {
    let key = stage_runtime_status_key(&stop.topology_id, &stop.run_id, &stop.stage_id);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *node.stage_control_tx.lock().await = Some(tx);
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(send_test_stage_stop(node, remote, stop), async {
            let crate::inference::skippy::StageControlCommand::Execute { request, resp, .. } =
                rx.recv().await.expect("stop command")
            else {
                panic!("unexpected load validation");
            };
            assert!(matches!(
                request,
                crate::inference::skippy::StageControlRequest::Stop(_)
            ));
            assert!(node.stage_transport_bridge_is_running(&key).await);
            resp.send(response).unwrap();
        })
    })
    .await?;
    result
}

#[tokio::test]
async fn stage_stop_preserves_bridge_until_accepted() -> Result<()> {
    use crate::inference::skippy::{
        StageControlResponse, StageReadyResponse, StageRuntimeState, StageStopRequest,
    };

    for remote in [false, true] {
        let node = Node::new_for_tests(NodeRole::Worker).await?;
        let load = test_stage_load_request();
        let key = stage_runtime_status_key(&load.topology_id, &load.run_id, &load.stage_id);
        node.ensure_stage_transport_bridge(
            make_test_endpoint_id(0x93),
            &load.topology_id,
            &load.run_id,
            &load.stage_id,
        )
        .await?;
        let other_key = stage_runtime_status_key("other-topology", "other-run", "other-stage");
        let other_owner = node
            .reserve_stage_transport_bridge_for_test(other_key.clone())
            .await;

        for (term, generation, error) in [
            (10, 7, Some("stale coordinator term 10 < 11")),
            (11, 6, Some("stale shutdown generation")),
            (11, 7, None),
        ] {
            let accepted = error.is_none();
            let state = if accepted {
                StageRuntimeState::Stopped
            } else {
                StageRuntimeState::Ready
            };
            let response = StageControlResponse::Ready(StageReadyResponse {
                accepted,
                status: stage_status_from_load(&load, state),
                error: error.map(str::to_string),
            });
            let result = respond_to_test_stage_stop(
                &node,
                remote,
                StageStopRequest {
                    topology_id: load.topology_id.clone(),
                    run_id: load.run_id.clone(),
                    stage_id: load.stage_id.clone(),
                    coordinator_term: term,
                    shutdown_generation: generation,
                },
                Ok(response),
            )
            .await?;
            let StageControlResponse::Ready(ready) = result else {
                panic!("expected stop response");
            };
            assert_eq!(ready.accepted, accepted);
            assert_eq!(node.stage_transport_bridge_exists(&key).await, !accepted);
            assert!(
                node.stage_transport_bridge_owner_matches(&other_key, &other_owner)
                    .await
            );
        }
        node.clear_stage_transport_bridge_for_test(&other_key).await;
    }
    Ok(())
}

#[tokio::test]
async fn stage_stop_failures_preserve_transport_bridge() -> Result<()> {
    use crate::inference::skippy::{StageControlResponse, StageStopRequest};

    for remote in [false, true] {
        let node = Node::new_for_tests(NodeRole::Worker).await?;
        let stop = StageStopRequest {
            topology_id: "topology-error".to_string(),
            run_id: "run-error".to_string(),
            stage_id: "stage-error".to_string(),
            coordinator_term: 11,
            shutdown_generation: 7,
        };
        let key = stage_runtime_status_key(&stop.topology_id, &stop.run_id, &stop.stage_id);
        node.ensure_stage_transport_bridge(
            make_test_endpoint_id(0x94),
            &stop.topology_id,
            &stop.run_id,
            &stop.stage_id,
        )
        .await?;
        let error = respond_to_test_stage_stop(
            &node,
            remote,
            stop.clone(),
            Err(anyhow::anyhow!("synthetic stop failure")),
        )
        .await
        .expect_err("failed stop");
        assert!(error.to_string().contains("synthetic stop failure"));
        assert!(node.stage_transport_bridge_is_running(&key).await);

        *node.stage_control_tx.lock().await = None;
        match send_test_stage_stop(&node, remote, stop).await {
            Ok(StageControlResponse::Ready(ready)) => assert!(!ready.accepted),
            Err(error) => assert!(error.to_string().contains("not available")),
            _ => panic!("unexpected unavailable response"),
        }
        assert!(node.stage_transport_bridge_is_running(&key).await);
        node.clear_stage_transport_bridge_for_test(&key).await;
    }
    Ok(())
}

#[tokio::test]
async fn stage_stop_clears_split_coordinator_when_no_bridges_remain() -> Result<()> {
    let node = Node::new_for_tests(NodeRole::Worker).await?;
    let coordinator = make_test_endpoint_id(0x42);
    node.note_split_coordinator(coordinator).await;
    assert!(node.split_coordinator().await.is_some());

    node
        .ensure_stage_transport_bridge(make_test_endpoint_id(0x93), "topo", "run", "s0")
        .await?;
    node
        .ensure_stage_transport_bridge(make_test_endpoint_id(0x93), "topo", "run", "s1")
        .await?;
    node.stop_stage_transport_bridge("topo", "run", "s0").await;
    assert!(
        node.split_coordinator().await.is_some(),
        "coordinator must survive while a stage remains"
    );
    node.stop_stage_transport_bridge("topo", "run", "s1").await;
    assert!(
        node.split_coordinator().await.is_none(),
        "coordinator must clear once the last bridge stops"
    );
    Ok(())
}
