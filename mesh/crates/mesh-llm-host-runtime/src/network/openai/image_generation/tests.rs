use super::*;
use crate::inference::election;
use crate::inference::image_endpoint::ImageEndpoint;
use crate::network::affinity::AffinityRouter;
use crate::network::openai::ingress::api_proxy;
use crate::network::tunnel::Manager as TunnelManager;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};

const MODEL: &str = "sd-turbo";
const IMAGE_RESPONSE: &str =
    r#"{"created":1767225600,"output_format":"png","data":[{"b64_json":"iVBORw0KGgo="}]}"#;

#[test]
fn inline_images_and_bounded_counts_are_accepted() {
    for body in [
        json!({"prompt": "a lighthouse"}),
        json!({"prompt": "a lighthouse", "response_format": "b64_json", "n": 1}),
        json!({"prompt": "a lighthouse", "response_format": null, "n": null}),
        json!({"prompt": "a lighthouse", "n": MAX_IMAGES_PER_REQUEST}),
    ] {
        assert_eq!(invalid_parameter(&body), None, "{body}");
    }
}

#[test]
fn url_responses_are_rejected_because_peers_cannot_reach_the_server() {
    for format in [json!("url"), json!("png"), json!(1)] {
        let invalid = invalid_parameter(&json!({"prompt": "x", "response_format": format}))
            .expect("rejected response_format");
        assert_eq!(invalid.param, Some("response_format"));
        let body = invalid.error_body();
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["param"], "response_format");
    }
}

#[test]
fn image_counts_outside_the_openai_range_are_rejected() {
    for n in [json!(0), json!(11), json!(-1), json!(2.5), json!("2")] {
        let invalid =
            invalid_parameter(&json!({"prompt": "x", "n": n})).expect("rejected image count");
        assert_eq!(invalid.param, Some("n"), "{n}");
    }
}

/// Answer one request with `IMAGE_RESPONSE` and hand back what was received.
async fn spawn_image_server() -> (
    String,
    oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<()>,
) {
    spawn_server(|_| IMAGE_RESPONSE.to_string()).await
}

/// Like LocalAI: inline images only when the request asks for `b64_json`,
/// otherwise a URL on the server itself.
async fn spawn_url_default_server() -> (
    String,
    oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<()>,
) {
    spawn_server(|body| {
        if body["response_format"] == "b64_json" {
            IMAGE_RESPONSE.to_string()
        } else {
            r#"{"created":1767225600,"data":[{"url":"http://127.0.0.1:8080/generated-images/b64.png"}]}"#
                .to_string()
        }
    })
    .await
}

async fn spawn_server(
    respond: fn(&Value) -> String,
) -> (
    String,
    oneshot::Receiver<Vec<u8>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (request_tx, request_rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let request = read_http_request(&mut stream).await;
        let body = respond(&forwarded_json(&request));
        let _ = request_tx.send(request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
    (base_url, request_rx, handle)
}

/// JSON body of a captured request, read through its declared Content-Length.
fn forwarded_json(raw: &[u8]) -> Value {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw.len(), |end| end + 4);
    serde_json::from_slice(&raw[header_end..]).unwrap_or(Value::Null)
}

/// A local URL that refuses connections, like a stopped image server.
async fn refusing_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/v1")
}

async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = stream.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            return raw;
        }
        raw.extend_from_slice(&chunk[..read]);
        let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&raw[..header_end]).to_ascii_lowercase();
        let body_len = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if raw.len() >= header_end + 4 + body_len {
            return raw;
        }
    }
}

async fn spawn_ingress(node: mesh::Node) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (_targets_tx, targets_rx) = watch::channel(election::ModelTargets::default());
    let handle = tokio::spawn(api_proxy(
        node,
        addr.port(),
        targets_rx,
        Some(listener.into()),
        false,
        AffinityRouter::new(),
    ));
    (addr, handle)
}

async fn post(addr: SocketAddr, path: &str, extra_headers: &str, body: &Value) -> String {
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut response),
    )
    .await
    .expect("ingress must answer")
    .unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

async fn node_with_image_endpoint(base_url: &str) -> mesh::Node {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    node.advertise_image_endpoint(&ImageEndpoint::parse(MODEL, base_url).unwrap(), 9337)
        .await;
    node
}

fn generation_body(model: &str) -> Value {
    json!({"model": model, "prompt": "a lighthouse at dusk", "size": "512x512"})
}

#[tokio::test]
async fn image_request_is_forwarded_to_the_local_image_endpoint() {
    let (base_url, server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body(MODEL)).await;

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with(IMAGE_RESPONSE), "{response}");
    let forwarded = String::from_utf8(server_request.await.unwrap()).unwrap();
    assert!(
        forwarded.starts_with("POST /v1/images/generations HTTP/1.1\r\n"),
        "{forwarded}"
    );
    assert!(
        forwarded.contains(r#""prompt":"a lighthouse at dusk""#),
        "{forwarded}"
    );
    ingress.abort();
    server.abort();
}

#[tokio::test]
async fn models_list_marks_the_image_workload() {
    let node = node_with_image_endpoint("http://127.0.0.1:9/v1").await;
    let (addr, ingress) = spawn_ingress(node).await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response).into_owned();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let body = &response[response.find('{').expect("JSON body")..];
    let models: Value = serde_json::from_str(body).expect("models list JSON");
    let entry = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == MODEL)
        .expect("image model listed");
    assert_eq!(entry["metadata"]["workload_class"], "image_generation");
    ingress.abort();
}

#[tokio::test]
async fn automatic_model_selection_picks_the_image_endpoint() {
    let (base_url, server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body("auto")).await;

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(server_request.await.is_ok());
    ingress.abort();
    server.abort();
}

#[tokio::test]
async fn other_endpoints_for_an_image_model_never_reach_its_server() {
    let (base_url, mut server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let (addr, ingress) = spawn_ingress(node).await;

    let chat = post(
        addr,
        "/v1/chat/completions",
        "",
        &json!({"model": MODEL, "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert!(chat.starts_with("HTTP/1.1 422"), "{chat}");

    let messages = post(
        addr,
        "/v1/messages",
        "",
        &json!({
            "model": MODEL,
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}],
        }),
    )
    .await;
    assert!(messages.starts_with("HTTP/1.1 422"), "{messages}");
    assert!(
        messages.contains("only serves image generation"),
        "{messages}"
    );

    assert!(
        server_request.try_recv().is_err(),
        "the image server must not see non-image requests"
    );
    ingress.abort();
    server.abort();
}

/// Servers such as LocalAI return a URL unless asked for `b64_json`; the mesh
/// asks for inline images whenever the client did not choose a format.
#[tokio::test]
async fn unset_response_format_is_forwarded_as_inline_images() {
    for response_format in [None, Some(Value::Null), Some(json!("b64_json"))] {
        let (base_url, server_request, server) = spawn_url_default_server().await;
        let node = node_with_image_endpoint(&base_url).await;
        let (addr, ingress) = spawn_ingress(node).await;
        let mut body = generation_body(MODEL);
        if let Some(response_format) = response_format.clone() {
            body["response_format"] = response_format;
        }

        let response = post(addr, "/v1/images/generations", "", &body).await;

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.contains(r#""b64_json""#),
            "{response_format:?}: {response}"
        );
        assert!(
            !response.contains(r#""url""#),
            "{response_format:?}: {response}"
        );
        let forwarded = forwarded_json(&server_request.await.unwrap());
        assert_eq!(forwarded["response_format"], "b64_json", "{forwarded}");
        assert_eq!(forwarded["prompt"], "a lighthouse at dusk", "{forwarded}");
        ingress.abort();
        server.abort();
    }
}

/// A body the mesh cannot rewrite must not reach a server that defaults to URLs.
#[tokio::test]
async fn non_json_image_requests_are_rejected_before_the_server() {
    let (base_url, mut server_request, server) = spawn_url_default_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let (addr, ingress) = spawn_ingress(node).await;
    let body = generation_body(MODEL).to_string();
    let request = format!(
        "POST /v1/images/generations HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response).into_owned();

    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    assert!(response.contains("application/json"), "{response}");
    assert!(server_request.try_recv().is_err());
    ingress.abort();
    server.abort();
}

#[tokio::test]
async fn local_server_failure_without_a_peer_answers_503() {
    let node = node_with_image_endpoint(&refusing_url().await).await;
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body(MODEL)).await;

    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(
        response.contains("image endpoint for model 'sd-turbo' failed"),
        "{response}"
    );
    ingress.abort();
}

#[tokio::test]
async fn url_response_format_is_rejected_before_the_server() {
    let (base_url, mut server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let (addr, ingress) = spawn_ingress(node).await;

    let mut body = generation_body(MODEL);
    body["response_format"] = json!("url");
    let response = post(addr, "/v1/images/generations", "", &body).await;

    assert!(response.starts_with("HTTP/1.1 400"), "{response}");
    assert!(
        response.contains(r#""param":"response_format""#),
        "{response}"
    );
    assert!(server_request.try_recv().is_err());
    ingress.abort();
    server.abort();
}

#[tokio::test]
async fn image_request_without_an_image_model_reports_the_missing_workload() {
    let node = mesh::Node::new_for_tests(mesh::NodeRole::Worker)
        .await
        .unwrap();
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body(MODEL)).await;

    assert!(response.starts_with("HTTP/1.1 422"), "{response}");
    assert!(response.contains("ImageGeneration"), "{response}");
    ingress.abort();
}

#[tokio::test]
async fn excluding_this_node_skips_its_image_endpoint() {
    let (base_url, mut server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let exclude = format!("x-mesh-exclude: {}\r\n", hex::encode(node.id().as_bytes()));
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(
        addr,
        "/v1/images/generations",
        &exclude,
        &generation_body(MODEL),
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    assert!(server_request.try_recv().is_err());
    ingress.abort();
    server.abort();
}

#[tokio::test]
async fn targeting_another_peer_skips_this_nodes_image_endpoint() {
    let (base_url, mut server_request, server) = spawn_image_server().await;
    let node = node_with_image_endpoint(&base_url).await;
    let other_peer = iroh::SecretKey::generate().public();
    let target = format!("x-mesh-target: {}\r\n", hex::encode(other_peer.as_bytes()));
    let (addr, ingress) = spawn_ingress(node).await;

    let response = post(
        addr,
        "/v1/images/generations",
        &target,
        &generation_body(MODEL),
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    assert!(server_request.try_recv().is_err());
    ingress.abort();
    server.abort();
}

/// Same fixture as the tunnel lifecycle tests: loopback QUIC without relays
/// and an isolated payment configuration.
async fn start_tunnel_node() -> (mesh::Node, mesh::TunnelChannels, tempfile::TempDir) {
    let relay_urls = Vec::new();
    let relay_auths = HashMap::new();
    let (node, channels) = mesh::Node::start(
        // Clients get an ephemeral identity; `advertise` promotes serving nodes.
        mesh::NodeRole::Client,
        mesh::RelayConfig {
            urls: &relay_urls,
            auths: &relay_auths,
            policy: mesh::RelayPolicy::Disabled,
        },
        mesh::QuicBindSelection {
            ip: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            port: Some(0),
        },
        Some(0.0),
        false,
        None,
        None,
        crate::MeshRequirements::unrestricted(),
    )
    .await
    .expect("start tunnel test node");
    let directory = tempfile::tempdir().expect("isolated payment config");
    *node.config_state.lock().await =
        crate::runtime::config_state::ConfigState::load(&directory.path().join("config.toml"))
            .expect("isolated config state");
    (node, channels, directory)
}

async fn advertise(node: &mesh::Node, base_url: &str) {
    *node.role.lock().await = mesh::NodeRole::Worker;
    node.advertise_image_endpoint(&ImageEndpoint::parse(MODEL, base_url).unwrap(), 9337)
        .await;
}

/// Two loopback nodes that have exchanged gossip: `peer` serves `MODEL` from
/// `peer_url` and accepts mesh HTTP tunnels; `entry` may serve it as well.
struct ImageMesh {
    entry: mesh::Node,
    peer: mesh::Node,
    _tunnels: TunnelManager,
    _targets: watch::Sender<election::ModelTargets>,
    _entry_channels: mesh::TunnelChannels,
    _configs: [tempfile::TempDir; 2],
}

async fn image_mesh(peer_url: &str, entry_url: Option<&str>) -> ImageMesh {
    let (peer, peer_channels, peer_config) = start_tunnel_node().await;
    let (entry, entry_channels, entry_config) = start_tunnel_node().await;
    let tunnels = TunnelManager::start(
        peer.clone(),
        peer_channels.rpc,
        peer_channels.http,
        peer_channels.stage,
    )
    .await
    .expect("start peer tunnel manager");
    let (targets, targets_rx) = watch::channel(election::ModelTargets::default());
    tunnels.set_http_ingress(targets_rx, AffinityRouter::new());
    advertise(&peer, peer_url).await;
    if let Some(entry_url) = entry_url {
        advertise(&entry, entry_url).await;
    }
    peer.start_accepting();
    entry.start_accepting();
    entry
        .connect_to_peer(peer.endpoint_addr_for_advertisement())
        .await
        .expect("connect test nodes");
    assert_eq!(entry.hosts_for_model(MODEL).await, vec![peer.id()]);
    ImageMesh {
        entry,
        peer,
        _tunnels: tunnels,
        _targets: targets,
        _entry_channels: entry_channels,
        _configs: [peer_config, entry_config],
    }
}

/// A node without an image model routes the request over the mesh to the
/// peer that advertises it, and that peer's image endpoint answers.
#[tokio::test]
async fn image_request_reaches_the_peer_that_advertises_the_model() {
    let (base_url, server_request, server) = spawn_image_server().await;
    let mesh = image_mesh(&base_url, None).await;
    let (addr, ingress) = spawn_ingress(mesh.entry.clone()).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body(MODEL)).await;

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains(r#""b64_json""#), "{response}");
    let forwarded = String::from_utf8(server_request.await.unwrap()).unwrap();
    assert!(
        forwarded.starts_with("POST /v1/images/generations HTTP/1.1\r\n"),
        "{forwarded}"
    );
    ingress.abort();
    server.abort();
}

/// A stopped local server must not hide a healthy peer while the health
/// check has not yet withdrawn the local advertisement.
#[tokio::test]
async fn local_server_failure_falls_back_to_a_peer_with_the_model() {
    let (peer_url, peer_request, peer_server) = spawn_url_default_server().await;
    let mesh = image_mesh(&peer_url, Some(&refusing_url().await)).await;
    let (addr, ingress) = spawn_ingress(mesh.entry.clone()).await;

    let response = post(addr, "/v1/images/generations", "", &generation_body(MODEL)).await;

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains(r#""b64_json""#), "{response}");
    let forwarded = forwarded_json(&peer_request.await.unwrap());
    assert_eq!(forwarded["response_format"], "b64_json", "{forwarded}");
    ingress.abort();
    peer_server.abort();
}

/// `x-mesh-target` naming this node pins the request even when its server fails.
#[tokio::test]
async fn pinned_local_server_failure_does_not_fall_back() {
    let (peer_url, mut peer_request, peer_server) = spawn_image_server().await;
    let mesh = image_mesh(&peer_url, Some(&refusing_url().await)).await;
    let target = format!(
        "x-mesh-target: {}\r\n",
        hex::encode(mesh.entry.id().as_bytes())
    );
    let (addr, ingress) = spawn_ingress(mesh.entry.clone()).await;

    let response = post(
        addr,
        "/v1/images/generations",
        &target,
        &generation_body(MODEL),
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(peer_request.try_recv().is_err());
    ingress.abort();
    peer_server.abort();
}

/// A node reached over the mesh answers for itself: when its own server is
/// down it must not send the request back out, or two nodes with stopped
/// servers would bounce it between them.
#[tokio::test]
async fn request_from_a_peer_is_not_forwarded_again() {
    let (entry_url, mut entry_request, entry_server) = spawn_image_server().await;
    let mesh = image_mesh(&refusing_url().await, Some(&entry_url)).await;
    assert_eq!(
        mesh.peer.hosts_for_model(MODEL).await,
        vec![mesh.entry.id()]
    );

    let body = generation_body(MODEL).to_string();
    let request = format!(
        "POST /v1/images/generations HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (mut send, mut recv) = mesh
        .entry
        .open_http_tunnel(mesh.peer.id())
        .await
        .expect("open HTTP tunnel");
    send.write_all(request.as_bytes())
        .await
        .expect("write tunneled request");
    send.finish().expect("finish tunneled request");
    let wire = recv
        .read_to_end(1024 * 1024)
        .await
        .expect("read tunneled response");
    let response = String::from_utf8_lossy(&wire).into_owned();

    assert!(response.starts_with("HTTP/1.1 503"), "{response}");
    assert!(
        entry_request.try_recv().is_err(),
        "the peer must not route the request back to the entry node"
    );
    entry_server.abort();
}

#[test]
fn only_the_entry_node_tries_peers_for_an_unpinned_request() {
    fn scope(
        target: Option<iroh::EndpointId>,
        excluded: &[iroh::EndpointId],
        from_peer: bool,
    ) -> ImageRouteScope<'_> {
        ImageRouteScope {
            target,
            excluded,
            from_peer,
        }
    }
    let node_id = iroh::SecretKey::generate().public();
    let other = iroh::SecretKey::generate().public();
    assert!(scope(None, &[], false).may_try_peers());
    assert!(!scope(None, &[], true).may_try_peers());
    assert!(!scope(Some(node_id), &[], false).may_try_peers());
    assert!(scope(Some(node_id), &[], false).allows(node_id));
    assert!(!scope(Some(other), &[], false).allows(node_id));
    assert!(!scope(None, &[node_id], false).allows(node_id));
}
