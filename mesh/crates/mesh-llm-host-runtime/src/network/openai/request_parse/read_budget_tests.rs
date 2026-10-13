use super::{BufferedHttpRequest, HttpReadLimits, MAX_HEADER_BYTES, read_http_request_with_limits};
use anyhow::Result;
use tokio::io::AsyncWriteExt;

fn limits(max_header_bytes: usize, max_chunked_wire_bytes: usize) -> HttpReadLimits {
    HttpReadLimits {
        max_header_bytes,
        max_body_bytes: 1024,
        max_chunked_wire_bytes,
    }
}

fn header_of_len(total: usize) -> Vec<u8> {
    sized_header(b"GET / HTTP/1.1\r\nX-Pad: ", total)
}

/// `Connection` is dropped when the request is forwarded, so a pad here is over
/// the read limit only if the completed-read check runs before that rebuild.
fn connection_header_of_len(total: usize) -> Vec<u8> {
    sized_header(b"GET / HTTP/1.1\r\nConnection: ", total)
}

fn sized_header(prefix: &[u8], total: usize) -> Vec<u8> {
    let suffix = b"\r\n\r\n";
    assert!(total > prefix.len() + suffix.len());
    let mut raw = prefix.to_vec();
    raw.extend(std::iter::repeat_n(
        b'a',
        total - prefix.len() - suffix.len(),
    ));
    raw.extend_from_slice(suffix);
    assert_eq!(raw.len(), total);
    raw
}

fn chunked_wire(body: &[u8]) -> Vec<u8> {
    let mut wire = format!("{:x}\r\n", body.len()).into_bytes();
    wire.extend_from_slice(body);
    wire.extend_from_slice(b"\r\n0\r\n\r\n");
    wire
}

fn chunked_request(wire: &[u8]) -> Vec<u8> {
    let mut raw =
        b"POST /v1/models HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    raw.extend_from_slice(wire);
    raw
}

async fn read_parts(parts: Vec<Vec<u8>>, limits: HttpReadLimits) -> Result<BufferedHttpRequest> {
    let capacity = parts.first().map(Vec::len).unwrap_or(1).max(1);
    let (mut client, mut server) = tokio::io::duplex(capacity);
    let task =
        tokio::spawn(async move { read_http_request_with_limits(&mut server, limits, None).await });
    for part in parts {
        client.write_all(&part).await.unwrap();
    }
    drop(client);
    task.await.unwrap()
}

async fn expect_limit_error(parts: Vec<Vec<u8>>, limits: HttpReadLimits, needle: &str) {
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), read_parts(parts, limits))
        .await
        .expect("limit check must finish")
        .expect_err("over-limit read")
        .to_string();
    assert!(error.contains(needle), "{error}");
}

#[tokio::test]
async fn completed_headers_over_the_limit_are_rejected() {
    let over = header_of_len(180);
    expect_limit_error(vec![over], limits(128, 1024), "HTTP headers exceed").await;

    let mut split = header_of_len(180);
    let rest = split.split_off(60);
    expect_limit_error(vec![split, rest], limits(128, 1024), "HTTP headers exceed").await;
}

#[tokio::test]
async fn stripped_header_over_the_completed_read_limit_is_rejected() {
    let limit = 1024;
    let over = connection_header_of_len(limit + 1);
    expect_limit_error(vec![over], limits(limit, 1024), "HTTP headers exceed").await;

    let mut split = connection_header_of_len(limit + 1);
    let rest = split.split_off(400);
    expect_limit_error(
        vec![split, rest],
        limits(limit, 1024),
        "HTTP headers exceed",
    )
    .await;
}

#[tokio::test]
async fn stripped_header_at_the_completed_read_limit_is_accepted() {
    let limit = 1024;
    let exact = connection_header_of_len(limit);
    let accepted = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_parts(vec![exact], limits(limit, 1024)),
    )
    .await
    .expect("boundary header")
    .expect("header_end equal to the limit must be forwarded");
    assert!(!accepted.raw.windows(8).any(|window| window == b"aaaaaaaa"));
}

#[tokio::test]
async fn header_at_the_limit_is_accepted_until_forwarding_grows_it() {
    let exact = header_of_len(128);
    let accepted = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_parts(vec![exact.clone()], limits(MAX_HEADER_BYTES, 1024)),
    )
    .await
    .expect("boundary header")
    .expect("a header inside the real ceiling must forward");
    assert!(accepted.raw.windows(4).any(|window| window == b"\r\n\r\n"));

    expect_limit_error(vec![exact], limits(128, 1024), "HTTP headers exceed").await;
}

#[tokio::test]
async fn completed_chunked_wire_over_the_limit_is_rejected() {
    let wire = chunked_wire(b"hello-over-limit");
    let limit = wire.len() - 1;
    let request = chunked_request(&wire);
    expect_limit_error(
        vec![request.clone()],
        limits(MAX_HEADER_BYTES, limit),
        "chunked wire body exceeds",
    )
    .await;

    let mut split = request;
    let rest = split.split_off(split.len() - 8);
    expect_limit_error(
        vec![split, rest],
        limits(MAX_HEADER_BYTES, limit),
        "chunked wire body exceeds",
    )
    .await;
}

#[tokio::test]
async fn chunked_wire_at_the_limit_is_accepted_including_lookahead() {
    let wire = chunked_wire(b"hello");
    let mut with_lookahead = chunked_request(&wire);
    with_lookahead.extend_from_slice(b"GET /next HTTP/1.1\r\n\r\n");
    let request = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_parts(vec![with_lookahead], limits(MAX_HEADER_BYTES, wire.len())),
    )
    .await
    .expect("boundary chunked read")
    .expect("consumed wire at the limit must be accepted");
    assert!(request.raw.ends_with(&wire));
    assert!(!request.raw.windows(8).any(|window| window == b"GET /nex"));
}
