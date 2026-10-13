use super::*;

fn management_json_http_request(method: &str, path: &str, body: &str, chunked: bool) -> String {
    let headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n",
    );
    if !chunked {
        return format!("{headers}Content-Length: {}\r\n\r\n{body}", body.len());
    }
    let (first, second) = body.split_at(3);
    format!(
        "{headers}Transfer-Encoding: chunked\r\n\r\n{:x};part=first\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\nX-Test-Trailer: complete\r\n\r\n",
        first.len(),
        second.len(),
    )
}

#[tokio::test]
async fn management_body_activity_override_accepts_both_http_framings() {
    let state = build_test_mesh_api().await;
    for mode in ["active", "idle", "auto"] {
        for chunked in [false, true] {
            let (address, server) = spawn_management_test_server(state.clone()).await;
            let response = send_management_request(
                address,
                management_json_http_request(
                    "PUT",
                    "/api/runtime/activity/override",
                    &format!("\"{mode}\""),
                    chunked,
                ),
            )
            .await;
            server.await.unwrap().unwrap();

            assert!(
                response.starts_with("HTTP/1.1 200 OK"),
                "mode={mode}, chunked={chunked}, response: {response}",
            );
            assert_eq!(json_body(&response)["override_mode"], mode);
        }
    }
}

#[tokio::test]
async fn management_body_chunked_payload_keeps_semantic_validation() {
    let state = build_test_mesh_api().await;
    for chunked in [false, true] {
        let (address, server) = spawn_management_test_server(state.clone()).await;
        let response = send_management_request(
            address,
            management_json_http_request(
                "PUT",
                "/api/runtime/activity/override",
                "\"unsupported\"",
                chunked,
            ),
        )
        .await;
        server.await.unwrap().unwrap();

        assert!(
            response.starts_with("HTTP/1.1 400 Bad Request"),
            "chunked={chunked}, response: {response}",
        );
        assert!(
            json_body(&response)["error"]
                .as_str()
                .is_some_and(|error| error.contains("unknown variant `unsupported`")),
            "chunked={chunked}, response: {response}",
        );
    }
}

#[tokio::test]
async fn management_body_mcp_initialization_accepts_both_http_framings() {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "management-test", "version": "1.0" },
        },
    })
    .to_string();
    for chunked in [false, true] {
        let state = build_test_mesh_api().await;
        let (address, server) = spawn_management_test_server(state).await;
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            send_management_request(
                address,
                management_json_http_request("POST", "/mcp", &body, chunked),
            ),
        )
        .await
        .expect("MCP initialization completes");
        server.await.unwrap().unwrap();

        assert!(
            response.starts_with("HTTP/1.1 200 OK"),
            "chunked={chunked}, response: {response}",
        );
        assert!(response.contains("\"serverInfo\""), "response: {response}");
        assert!(
            response.contains("\"protocolVersion\""),
            "response: {response}"
        );
    }
}
