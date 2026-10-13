//! Ingress request framing: one `Content-Length` or a single final `chunked`.
//!
//! Ambiguous framing is rejected before the body is read. A forwarded request
//! then carries exactly one of those schemes, never the raw header list.

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestFraming {
    Fixed(usize),
    Chunked,
    None,
}

impl RequestFraming {
    pub(super) fn into_parts(self) -> (Option<usize>, bool) {
        match self {
            Self::Fixed(length) => (Some(length), false),
            Self::Chunked => (None, true),
            Self::None => (None, false),
        }
    }
}

pub(super) fn request_framing(headers: &[httparse::Header<'_>]) -> Result<RequestFraming> {
    let content_length = single_content_length(headers)?;
    let chunked = classify_transfer_codings(&transfer_codings(headers)?)?;
    match (content_length, chunked) {
        (Some(_), true) => {
            bail!("HTTP request contains both Content-Length and Transfer-Encoding")
        }
        (Some(length), false) => Ok(RequestFraming::Fixed(length)),
        (None, true) => Ok(RequestFraming::Chunked),
        (None, false) => Ok(RequestFraming::None),
    }
}

pub(super) fn append_request_framing(rebuilt: &mut String, framing: RequestFraming) {
    match framing {
        RequestFraming::Fixed(length) => {
            rebuilt.push_str(&format!("Content-Length: {length}\r\n"));
        }
        RequestFraming::Chunked => rebuilt.push_str("Transfer-Encoding: chunked\r\n"),
        RequestFraming::None => {}
    }
}

fn single_content_length(headers: &[httparse::Header<'_>]) -> Result<Option<usize>> {
    let mut length = None;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if length.is_some() {
            bail!("duplicate Content-Length");
        }
        length = Some(parse_content_length(header.value)?);
    }
    Ok(length)
}

fn parse_content_length(value: &[u8]) -> Result<usize> {
    let value = std::str::from_utf8(value).context("invalid Content-Length encoding")?;
    let value = trim_ows(value);
    if value.contains(',') {
        bail!("duplicate Content-Length");
    }
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("invalid Content-Length: {value}");
    }
    value
        .parse::<usize>()
        .with_context(|| format!("invalid Content-Length: {value}"))
}

fn transfer_codings(headers: &[httparse::Header<'_>]) -> Result<Vec<String>> {
    let mut codings = Vec::new();
    for header in headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            codings.extend(parse_transfer_coding_list(header.value)?);
        }
    }
    Ok(codings)
}

fn parse_transfer_coding_list(value: &[u8]) -> Result<Vec<String>> {
    let value = std::str::from_utf8(value).context("invalid Transfer-Encoding encoding")?;
    if trim_ows(value).is_empty() {
        bail!("empty Transfer-Encoding");
    }
    value.split(',').map(parse_transfer_coding).collect()
}

fn parse_transfer_coding(part: &str) -> Result<String> {
    let part = trim_ows(part);
    if part.is_empty() {
        bail!("empty Transfer-Encoding");
    }
    let (coding, parameter) = match part.split_once(';') {
        Some((coding, parameter)) => (trim_ows(coding), Some(parameter)),
        None => (part, None),
    };
    if parameter.is_some() {
        bail!("unsupported Transfer-Encoding parameter");
    }
    if coding.is_empty() {
        bail!("empty Transfer-Encoding");
    }
    Ok(coding.to_ascii_lowercase())
}

fn classify_transfer_codings(codings: &[String]) -> Result<bool> {
    if codings.is_empty() {
        return Ok(false);
    }
    let chunked_count = codings
        .iter()
        .filter(|coding| coding.as_str() == "chunked")
        .count();
    if chunked_count > 1 {
        bail!("duplicate Transfer-Encoding");
    }
    if chunked_count == 1 && codings.last().is_some_and(|coding| coding != "chunked") {
        bail!("misordered Transfer-Encoding: {}", codings.join(", "));
    }
    if codings.len() != 1 || codings[0] != "chunked" {
        bail!("unsupported Transfer-Encoding: {}", codings.join(", "));
    }
    Ok(true)
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framing_error(raw: &str) -> String {
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut request = httparse::Request::new(&mut headers);
        request.parse(raw.as_bytes()).expect("header fixture");
        request_framing(request.headers).expect_err(raw).to_string()
    }

    fn framing_ok(raw: &str) -> RequestFraming {
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut request = httparse::Request::new(&mut headers);
        request.parse(raw.as_bytes()).expect("header fixture");
        request_framing(request.headers).expect(raw)
    }

    #[test]
    fn duplicate_and_listed_content_lengths_are_rejected() {
        for raw in [
            "POST /api/objects HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n",
            "POST /api/objects HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 2\r\n\r\n",
            "POST /api/objects HTTP/1.1\r\nContent-Length: 5, 5\r\n\r\n",
            "POST /api/objects HTTP/1.1\r\nContent-Length: 5, 2\r\n\r\n",
        ] {
            assert!(
                framing_error(raw).contains("duplicate Content-Length"),
                "{raw}"
            );
        }
    }

    #[test]
    fn mixed_and_invalid_transfer_encodings_are_rejected() {
        let cases = [
            (
                "POST /api/objects HTTP/1.1\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
                "both Content-Length",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
                "unsupported Transfer-Encoding",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nTransfer-Encoding: chunked, gzip\r\n\r\n",
                "misordered Transfer-Encoding",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
                "duplicate Transfer-Encoding",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nTransfer-Encoding: identity\r\n\r\n",
                "unsupported Transfer-Encoding",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nTransfer-Encoding: chunked;ext=1\r\n\r\n",
                "unsupported Transfer-Encoding parameter",
            ),
        ];
        for (raw, needle) in cases {
            let error = framing_error(raw);
            assert!(error.contains(needle), "{raw} -> {error}");
        }
    }

    #[test]
    fn one_content_length_or_one_chunked_coding_is_accepted() {
        assert_eq!(
            framing_ok("POST /api/objects HTTP/1.1\r\nContent-Length: 5\r\n\r\n"),
            RequestFraming::Fixed(5)
        );
        assert_eq!(
            framing_ok("POST /api/objects HTTP/1.1\r\nContent-Length: \t0042\t\r\n\r\n"),
            RequestFraming::Fixed(42)
        );
        assert_eq!(
            framing_ok("GET /v1/models HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            RequestFraming::None
        );
        assert_eq!(
            framing_ok("POST /api/objects HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n"),
            RequestFraming::Chunked
        );
    }

    fn header_values(raw: &[u8], name: &str) -> Vec<String> {
        let end = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("headers");
        std::str::from_utf8(&raw[..end])
            .unwrap()
            .split("\r\n")
            .filter_map(|line| {
                let (header, value) = line.split_once(':')?;
                header
                    .eq_ignore_ascii_case(name)
                    .then(|| trim_ows(value).to_string())
            })
            .collect()
    }

    fn forwarded(raw: &str, rewritten_body: Option<&[u8]>) -> Result<Vec<u8>> {
        let header_end = raw.find("\r\n\r\n").expect("headers").saturating_add(4);
        super::super::finalize_forwarded_request(
            raw.as_bytes().to_vec(),
            header_end,
            false,
            None,
            rewritten_body,
            mesh_llm_events::logging::identifiers::RequestId::new(),
            super::super::MAX_HEADER_BYTES,
        )
    }

    #[test]
    fn forwarding_rejects_ambiguous_framing_or_emits_one_scheme() {
        let mixed = "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let error = forwarded(mixed, None).expect_err("mixed framing");
        assert!(error.to_string().contains("both Content-Length"));
        let rewritten =
            forwarded(mixed, Some(b"{}")).expect_err("rewrite must not launder framing");
        assert!(rewritten.to_string().contains("both Content-Length"));

        let fixed = forwarded(
            "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0042\r\n\r\n",
            None,
        )
        .unwrap();
        assert_eq!(
            header_values(&fixed, "content-length"),
            vec!["42".to_string()]
        );
        assert!(header_values(&fixed, "transfer-encoding").is_empty());

        let chunked = forwarded(
            "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: Chunked\r\n\r\n0\r\n\r\n",
            None,
        )
        .unwrap();
        assert_eq!(
            header_values(&chunked, "transfer-encoding"),
            vec!["chunked".to_string()]
        );
        assert!(header_values(&chunked, "content-length").is_empty());
    }

    async fn read_request(raw: &str) -> Result<super::super::BufferedHttpRequest> {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        use tokio::io::AsyncWriteExt;
        client.write_all(raw.as_bytes()).await.unwrap();
        drop(client);
        super::super::read_http_request_with_limits(
            &mut server,
            super::super::HTTP_READ_LIMITS,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn ingress_rejects_ambiguous_frames_that_previously_parsed() {
        let cases = [
            (
                "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nhello",
                "duplicate Content-Length",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nContent-Length: 2\r\n\r\nhello",
                "duplicate Content-Length",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
                "both Content-Length",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n",
                "unsupported Transfer-Encoding",
            ),
            (
                "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked, gzip\r\n\r\n0\r\n\r\n",
                "misordered Transfer-Encoding",
            ),
        ];
        for (raw, needle) in cases {
            let error = read_request(raw).await.expect_err(raw).to_string();
            assert!(error.contains(needle), "{raw} -> {error}");
        }
    }

    #[tokio::test]
    async fn ingress_still_forwards_fixed_and_chunked_bodies() {
        let fixed = read_request(
            "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello",
        )
        .await
        .unwrap();
        assert_eq!(
            header_values(&fixed.raw, "content-length"),
            vec!["5".to_string()]
        );
        assert!(header_values(&fixed.raw, "transfer-encoding").is_empty());
        assert!(fixed.raw.ends_with(b"hello"));

        let chunked = read_request(
            "POST /api/objects HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: Chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        )
        .await
        .unwrap();
        assert_eq!(
            header_values(&chunked.raw, "transfer-encoding"),
            vec!["chunked".to_string()]
        );
        assert!(header_values(&chunked.raw, "content-length").is_empty());
        assert!(chunked.raw.ends_with(b"5\r\nhello\r\n0\r\n\r\n"));
    }
}
