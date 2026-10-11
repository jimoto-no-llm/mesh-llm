//! Local dispatch of image generation requests to `--image-endpoint` servers.
//!
//! Workload admission has already confirmed that some target advertises the
//! requested model for image generation. This module serves the request when
//! that target is this node, forwarding it to the operator's server through
//! the same external-endpoint relay that plugin endpoints use.

use crate::logging::OpenAiRouteObserver;
use crate::mesh::{self, ModelWorkloadClass};
use crate::network::openai::client_stream::ClientStream;
use crate::network::openai::transport::{self as proxy, BufferedHttpRequest, RouteDispatchOutcome};
use crate::network::openai::workload_routing::request_workload_class;
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Selection label recorded for requests served by an image endpoint.
const IMAGE_ENDPOINT_PROVIDER: &str = "image-endpoint";
/// Upper bound on `n`, matching the OpenAI Images API.
const MAX_IMAGES_PER_REQUEST: u64 = 10;

/// Serve `model` from this node's image endpoint when it advertises one and
/// the mesh routing headers allow this node; otherwise return the stream so
/// remote and plugin routing can continue.
// `RouteDispatchOutcome` is deliberately `Copy`; its usage-plus-output-digests variant
// (three optional 32-byte digests inline) exceeds clippy's 128-byte `Err` threshold.
#[allow(clippy::result_large_err)]
pub(super) async fn route_to_local_image_endpoint(
    node: &mesh::Node,
    tcp_stream: ClientStream,
    request: &mut BufferedHttpRequest,
    model: &str,
    target: Option<iroh::EndpointId>,
    excluded: &[iroh::EndpointId],
    route_observer: OpenAiRouteObserver<'_>,
) -> Result<RouteDispatchOutcome, ClientStream> {
    let self_id = node.id();
    if target.is_some_and(|target| target != self_id) || excluded.contains(&self_id) {
        return Err(tcp_stream);
    }
    let Some(base_url) = node.image_endpoint_url(model) else {
        return Err(tcp_stream);
    };
    Ok(dispatch(node, tcp_stream, request, model, &base_url, route_observer).await)
}

async fn dispatch(
    node: &mesh::Node,
    tcp_stream: ClientStream,
    request: &mut BufferedHttpRequest,
    model: &str,
    base_url: &str,
    route_observer: OpenAiRouteObserver<'_>,
) -> RouteDispatchOutcome {
    // Endpoints without a workload class (such as `/v1/messages`) pass
    // admission; never relay them to an image server.
    if request_workload_class(&request.client_path) != Some(ModelWorkloadClass::ImageGeneration) {
        let message = format!(
            "model '{model}' only serves image generation; send POST /v1/images/generations"
        );
        return response_outcome(
            422,
            proxy::send_error_observed(tcp_stream, 422, &message, route_observer).await,
        );
    }
    request.ensure_body_json();
    if let Some(invalid) = request.body_json.as_ref().and_then(invalid_parameter) {
        return response_outcome(
            400,
            proxy::send_json_with_status_and_headers_observed(
                tcp_stream,
                400,
                &invalid.error_body(),
                &[],
                route_observer,
            )
            .await,
        );
    }
    let mut tcp_stream = tcp_stream;
    let outcome = proxy::route_http_endpoint_request(
        node,
        Some(model),
        proxy::RouteSelectionMetadata {
            provider: Some(IMAGE_ENDPOINT_PROVIDER),
            engine: None,
        },
        &mut tcp_stream,
        base_url,
        request,
        route_observer,
    )
    .await;
    if outcome.response_written() || matches!(outcome, RouteDispatchOutcome::Dropped(_)) {
        return outcome;
    }
    let message = format!("image endpoint for model '{model}' failed");
    response_outcome(
        503,
        proxy::send_503_observed(tcp_stream, &message, route_observer).await,
    )
}

/// A request parameter the mesh cannot honor, reported before generation starts.
#[derive(Debug, PartialEq)]
struct InvalidParameter {
    param: &'static str,
    message: String,
}

impl InvalidParameter {
    fn error_body(&self) -> Value {
        json!({
            "error": {
                "message": self.message,
                "type": "invalid_request_error",
                "param": self.param,
                "code": "invalid_value",
            }
        })
    }
}

/// Check the parameters whose meaning depends on the mesh rather than the
/// server. A URL points at the serving node's local server, which the caller
/// usually cannot reach, so images must come back inline.
fn invalid_parameter(body: &Value) -> Option<InvalidParameter> {
    if let Some(format) = body.get("response_format").filter(|value| !value.is_null())
        && format.as_str() != Some("b64_json")
    {
        return Some(InvalidParameter {
            param: "response_format",
            message: "response_format must be \"b64_json\": images generated through the mesh are returned inline, not as URLs".to_string(),
        });
    }
    if let Some(n) = body.get("n").filter(|value| !value.is_null())
        && !n
            .as_u64()
            .is_some_and(|n| (1..=MAX_IMAGES_PER_REQUEST).contains(&n))
    {
        return Some(InvalidParameter {
            param: "n",
            message: format!("n must be an integer from 1 to {MAX_IMAGES_PER_REQUEST}"),
        });
    }
    None
}

fn response_outcome(status_code: u16, result: std::io::Result<()>) -> RouteDispatchOutcome {
    match result {
        Ok(()) => RouteDispatchOutcome::Responded(status_code),
        Err(_) => RouteDispatchOutcome::Dropped("response_write_failed"),
    }
}
