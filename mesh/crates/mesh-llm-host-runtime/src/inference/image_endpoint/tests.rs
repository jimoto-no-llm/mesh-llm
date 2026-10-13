use super::*;

fn endpoint(model: &str, url: &str) -> ImageEndpoint {
    ImageEndpoint::parse(model, url).expect("valid image endpoint")
}

fn pair(model: &str, url: &str) -> (String, String) {
    (model.to_string(), url.to_string())
}

#[test]
fn accepts_http_base_urls_with_or_without_a_path() {
    for url in [
        "http://127.0.0.1:1234/v1",
        "http://127.0.0.1:1234",
        "http://image-box.lan:8080/api/v1/",
    ] {
        let parsed = endpoint("sd-turbo", url);
        assert_eq!(parsed.model, "sd-turbo");
        assert!(parsed.base_url.starts_with("http://"), "{url}");
    }
    assert_eq!(
        endpoint("stabilityai/sdxl-turbo", "http://127.0.0.1:1234/v1").model,
        "stabilityai/sdxl-turbo"
    );
}

#[test]
fn rejects_urls_the_ingress_cannot_forward_to() {
    for (url, expected) in [
        ("https://127.0.0.1:1234/v1", "http://"),
        ("localhost:1234", "http://"),
        ("127.0.0.1:1234", "not a valid URL"),
        ("not a url", "not a valid URL"),
        ("http://user:secret@127.0.0.1:1234/v1", "credentials"),
        ("http://127.0.0.1:1234/v1?key=value", "query"),
        ("http://127.0.0.1:1234/v1#frag", "query or fragment"),
    ] {
        let error = ImageEndpoint::parse("sd-turbo", url)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{url}: {error}");
        assert!(!error.contains("secret"), "{url}: {error}");
    }
}

#[test]
fn rejects_model_names_that_routing_reads_differently() {
    for (model, expected) in [
        ("", "model name"),
        ("sd turbo", "whitespace"),
        ("sd-turbo#fast", "'#'"),
        ("auto", "automatic routing"),
        ("mesh", "automatic routing"),
    ] {
        let error = ImageEndpoint::parse(model, "http://127.0.0.1:1234/v1")
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{model:?}: {error}");
    }
}

#[test]
fn duplicate_names_are_rejected() {
    let error = parse_image_endpoints(&[
        pair("sd-turbo", "http://127.0.0.1:1234/v1"),
        pair("sd-turbo", "http://127.0.0.1:1235/v1"),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("more than once"), "{error}");
    assert_eq!(
        parse_image_endpoints(&[
            pair("sd-turbo", "http://127.0.0.1:1234/v1"),
            pair("flux", "http://127.0.0.1:1235/v1"),
        ])
        .unwrap()
        .len(),
        2
    );
}

#[test]
fn client_nodes_cannot_host_image_endpoints() {
    let options = crate::RuntimeOptions {
        client: true,
        image_endpoints: vec![pair("sd-turbo", "http://127.0.0.1:1234/v1")],
        ..Default::default()
    };
    let error = image_endpoints_from_options(&options).unwrap_err();
    assert!(error.to_string().contains("mesh-llm serve"), "{error}");
    let serving = crate::RuntimeOptions {
        image_endpoints: vec![pair("sd-turbo", "http://127.0.0.1:1234/v1")],
        ..Default::default()
    };
    assert_eq!(image_endpoints_from_options(&serving).unwrap().len(), 1);
}

/// The probe must hit the same listing a forwarded `GET /v1/models` reaches.
#[test]
fn health_probe_uses_the_forwarded_models_path() {
    for (base, expected) in [
        (
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.1:1234/v1/models",
        ),
        (
            "http://127.0.0.1:1234/v1/",
            "http://127.0.0.1:1234/v1/models",
        ),
        ("http://127.0.0.1:1234/", "http://127.0.0.1:1234/v1/models"),
        (
            "http://127.0.0.1:1234/api",
            "http://127.0.0.1:1234/api/v1/models",
        ),
        (
            "http://127.0.0.1:1234/api/v1",
            "http://127.0.0.1:1234/api/v1/models",
        ),
    ] {
        assert_eq!(models_url(base).unwrap().as_str(), expected, "{base}");
    }
}

#[test]
fn health_advertises_on_first_answer_and_withdraws_after_repeated_failures() {
    let mut health = EndpointHealth::default();
    assert_eq!(health.observe(false), HealthTransition::Unreachable);
    assert_eq!(health.observe(false), HealthTransition::Unchanged);
    assert_eq!(health.observe(true), HealthTransition::Advertise);
    assert_eq!(health.observe(true), HealthTransition::Unchanged);
    // One slow answer does not withdraw the model.
    assert_eq!(health.observe(false), HealthTransition::Unchanged);
    assert_eq!(health.observe(true), HealthTransition::Unchanged);
    assert_eq!(health.observe(false), HealthTransition::Unchanged);
    assert_eq!(health.observe(false), HealthTransition::Withdraw);
    // The withdrawal already reported the outage.
    assert_eq!(health.observe(false), HealthTransition::Unchanged);
    assert_eq!(health.observe(true), HealthTransition::Advertise);
}

#[test]
fn status_messages_name_the_model_and_the_probed_url() {
    let sd_turbo = endpoint("sd-turbo", "http://127.0.0.1:1234/v1");
    let Some(OutputEvent::Warning { message, .. }) =
        status_event(HealthTransition::Unreachable, &sd_turbo)
    else {
        panic!("unreachable endpoints warn");
    };
    assert!(message.contains("'sd-turbo'"), "{message}");
    assert!(
        message.contains("http://127.0.0.1:1234/v1/models"),
        "{message}"
    );
    assert!(matches!(
        status_event(HealthTransition::Advertise, &sd_turbo),
        Some(OutputEvent::Info { .. })
    ));
    assert!(matches!(
        status_event(HealthTransition::Withdraw, &sd_turbo),
        Some(OutputEvent::Warning { .. })
    ));
    assert!(status_event(HealthTransition::Unchanged, &sd_turbo).is_none());
}

#[test]
fn advertised_descriptor_carries_only_the_image_workload() {
    let descriptor = image_generation_descriptor("sd-turbo");
    assert_eq!(descriptor.identity.model_name, "sd-turbo");
    assert!(!descriptor.capabilities_known);
    assert_eq!(
        descriptor.metadata.unwrap(),
        mesh::ServedModelMetadata {
            workload_class: Some(ModelWorkloadClass::ImageGeneration),
            ..Default::default()
        }
    );
}

#[tokio::test]
async fn advertised_endpoint_joins_every_local_serving_view() {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    let sd_turbo = endpoint("sd-turbo", "http://127.0.0.1:1234/v1");
    node.advertise_image_endpoint(&sd_turbo, 9337).await;

    assert_eq!(
        node.image_endpoint_url("sd-turbo").as_deref(),
        Some(sd_turbo.base_url.as_str())
    );
    assert!(node.image_endpoint_url("other").is_none());
    assert_eq!(node.role().await, mesh::NodeRole::Host { http_port: 9337 });
    assert!(
        node.models_being_served()
            .await
            .contains(&"sd-turbo".to_string())
    );
    let descriptors = node.all_served_model_descriptors().await;
    assert_eq!(descriptors, vec![image_generation_descriptor("sd-turbo")]);

    let announcement = node.snapshot_local_announcement_data().await;
    let announcement = node.build_local_announcement(announcement);
    assert_eq!(announcement.serving_models, vec!["sd-turbo".to_string()]);
    assert_eq!(
        announcement.hosted_models,
        Some(vec!["sd-turbo".to_string()])
    );
    assert_eq!(
        announcement.served_model_descriptors,
        vec![image_generation_descriptor("sd-turbo")]
    );

    node.withdraw_image_endpoint("sd-turbo").await;
    assert!(node.image_endpoint_url("sd-turbo").is_none());
    assert_eq!(node.role().await, mesh::NodeRole::Worker);
    assert!(node.all_served_model_descriptors().await.is_empty());
}

/// Native startup replaces the serving lists wholesale; it must not erase an
/// image endpoint that is still answering.
#[tokio::test]
async fn native_serving_resets_do_not_drop_an_advertised_endpoint() {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    node.advertise_image_endpoint(&endpoint("sd-turbo", "http://127.0.0.1:1234/v1"), 9337)
        .await;
    node.set_serving_models(Vec::new()).await;
    node.set_hosted_models(Vec::new()).await;

    let announcement = node.snapshot_local_announcement_data().await;
    let announcement = node.build_local_announcement(announcement);
    assert_eq!(
        announcement.hosted_models,
        Some(vec!["sd-turbo".to_string()])
    );
    assert_eq!(node.image_endpoint_models(), vec!["sd-turbo".to_string()]);
}

/// A native model of the same name keeps its own descriptor; the image
/// endpoint never adds a second, conflicting class for that name.
#[tokio::test]
async fn native_descriptor_wins_over_a_same_named_image_endpoint() {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    let native = mesh::ServedModelDescriptor {
        identity: mesh::ServedModelIdentity {
            model_name: "shared".into(),
            ..Default::default()
        },
        metadata: Some(mesh::ServedModelMetadata {
            workload_class: Some(ModelWorkloadClass::CausalGeneration),
            ..Default::default()
        }),
        ..Default::default()
    };
    node.set_served_model_descriptors(vec![native.clone()])
        .await;
    node.advertise_image_endpoint(&endpoint("shared", "http://127.0.0.1:1234/v1"), 9337)
        .await;
    assert_eq!(node.served_model_descriptors().await, vec![native]);
}

#[tokio::test]
async fn repeated_advertisements_hold_a_single_host_claim() {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    let sd_turbo = endpoint("sd-turbo", "http://127.0.0.1:1234/v1");
    node.advertise_image_endpoint(&sd_turbo, 9337).await;
    node.advertise_image_endpoint(&sd_turbo, 9337).await;
    node.withdraw_image_endpoint("sd-turbo").await;
    assert_eq!(node.role().await, mesh::NodeRole::Worker);
    // Withdrawing an endpoint that is not advertised releases nothing.
    node.claim_host_role(mesh::HostRoleClaim::LocalModel, 9337)
        .await;
    node.withdraw_image_endpoint("sd-turbo").await;
    assert_eq!(node.role().await, mesh::NodeRole::Host { http_port: 9337 });
}
