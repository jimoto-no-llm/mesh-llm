use super::*;
use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, BufReader};

fn callers() -> Vec<(Option<SocketAddr>, &'static str, bool)> {
    let local = Some("127.0.0.1:40123".parse().unwrap());
    vec![
        (local, "Host: localhost:3131\r\n", true),
        (
            local,
            "Host: 127.0.0.1:3131\r\nOrigin: http://localhost:3131\r\n",
            true,
        ),
        (
            Some("[::1]:40123".parse().unwrap()),
            "Host: [::1]:3131\r\nOrigin: http://[::1]:3131\r\n",
            true,
        ),
        (local, "Host: attacker.example\r\n", false),
        (
            local,
            "Host: localhost\r\nOrigin: https://attacker.example\r\n",
            false,
        ),
        (local, "Host: localhost\r\nOrigin: null\r\n", false),
        (local, "Host: localhost\r\nOrigin: invalid\r\n", false),
        (
            Some("192.0.2.10:40123".parse().unwrap()),
            "Host: localhost\r\nOrigin: http://localhost\r\n",
            false,
        ),
        (None, "Host: localhost\r\n", false),
    ]
}

async fn dispatch(
    state: MeshApi,
    source: Option<SocketAddr>,
    path: &'static str,
    headers: &str,
) -> (
    BufReader<TcpStream>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut server, _) = listener.accept().await.unwrap();
    let request = format!("GET {path} HTTP/1.1\r\n{headers}\r\n");
    let task = tokio::spawn(async move {
        Box::pin(crate::api::server::dispatch_management_request(
            &mut server,
            &state,
            source,
            "GET",
            path,
            path,
            "",
            &request,
            request.as_bytes(),
        ))
        .await
    });
    (BufReader::new(client), task)
}

async fn next_snapshot(reader: &mut BufReader<TcpStream>) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
            if let Some(json) = line.strip_prefix("data: ") {
                return serde_json::from_str(json).unwrap();
            }
        }
    })
    .await
    .expect("status event")
}

fn assert_snapshot(snapshot: &serde_json::Value, token: &str, trusted: bool, ready: bool) {
    assert_eq!(snapshot["token"], if trusted { token } else { "" });
    if !trusted {
        assert!(!snapshot.to_string().contains(token));
    }
    assert_eq!(snapshot["version"], crate::BUILD_VERSION);
    assert_eq!(snapshot["llama_ready"], ready);
    assert_eq!(snapshot["is_host"], ready);
    assert!(snapshot["peers"].is_array());
    assert!(snapshot["models"].is_array());
    assert!(snapshot["runtime"].is_object());
}

#[tokio::test]
#[serial]
async fn status_token_visibility_follows_caller_trust() {
    let home = tempfile::tempdir().unwrap();
    let _home = HomeEnvGuard::set(home.path());
    let state = build_test_mesh_api().await;
    let token = state.status().await.token;
    assert!(!token.is_empty());
    for (source, headers, trusted) in callers() {
        let (mut reader, task) = dispatch(state.clone(), source, "/api/status", headers).await;
        let mut response = String::new();
        reader.read_to_string(&mut response).await.unwrap();
        task.await.unwrap().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert_snapshot(&json_body(&response), &token, trusted, false);
    }
    assert_eq!(state.status().await.token, token);
}

#[tokio::test]
#[serial]
async fn initial_status_events_follow_caller_trust() {
    let home = tempfile::tempdir().unwrap();
    let _home = HomeEnvGuard::set(home.path());
    let state = build_test_mesh_api().await;
    let token = state.status().await.token;
    assert!(!token.is_empty());
    for (source, headers, trusted) in callers() {
        let (mut reader, task) = dispatch(state.clone(), source, "/api/events", headers).await;
        let mut head = String::new();
        reader.read_line(&mut head).await.unwrap();
        assert_eq!(head, "HTTP/1.1 200 OK\r\n");
        assert_snapshot(&next_snapshot(&mut reader).await, &token, trusted, false);
        task.abort();
    }
}

#[tokio::test]
#[serial]
async fn status_events_keep_independent_visibility_on_updates_and_reconnects() {
    let home = tempfile::tempdir().unwrap();
    let _home = HomeEnvGuard::set(home.path());
    let state = build_test_mesh_api().await;
    let token = state.status().await.token;
    let source = Some("127.0.0.1:40123".parse().unwrap());
    let (mut local, local_task) =
        dispatch(state.clone(), source, "/api/events", "Host: localhost\r\n").await;
    let (mut remote, remote_task) = dispatch(
        state.clone(),
        source,
        "/api/events",
        "Host: attacker.example\r\n",
    )
    .await;
    assert_snapshot(&next_snapshot(&mut local).await, &token, true, false);
    assert_snapshot(&next_snapshot(&mut remote).await, &token, false, false);

    state.update(true, true).await;
    assert_snapshot(&next_snapshot(&mut local).await, &token, true, true);
    assert_snapshot(&next_snapshot(&mut remote).await, &token, false, true);
    remote_task.abort();
    drop(remote);

    let (mut reconnected, reconnect_task) = dispatch(
        state.clone(),
        source,
        "/api/events",
        "Host: attacker.example\r\nLast-Event-ID: 1\r\n",
    )
    .await;
    assert_snapshot(&next_snapshot(&mut reconnected).await, &token, false, true);
    state.update(false, false).await;
    assert_snapshot(&next_snapshot(&mut local).await, &token, true, false);
    assert_snapshot(&next_snapshot(&mut reconnected).await, &token, false, false);
    assert_eq!(state.status().await.token, token);
    local_task.abort();
    reconnect_task.abort();
}

#[tokio::test]
#[serial]
async fn protected_routes_still_reject_untrusted_status_callers() {
    let home = tempfile::tempdir().unwrap();
    let _home = HomeEnvGuard::set(home.path());
    let state = build_test_mesh_api().await;
    for (source, headers, trusted) in callers() {
        if trusted {
            continue;
        }
        for path in [
            "/api/runtime/endpoints",
            "/api/runtime/control-bootstrap",
            "/api/logs/requests",
            "/api/plugins",
        ] {
            let (mut reader, task) = dispatch(state.clone(), source, path, headers).await;
            let mut response = String::new();
            reader.read_to_string(&mut response).await.unwrap();
            task.await.unwrap().unwrap();
            assert!(response.starts_with("HTTP/1.1 403"), "{path}: {response}");
        }
    }
}
