//! Local dispatch of image generation requests to `--image-endpoint` servers.
//!
//! Workload admission has already confirmed that some target advertises the
//! requested model for image generation. This module serves the request when
//! that target is this node, forwarding it to the operator's server through
//! the same external-endpoint relay that plugin endpoints use.

use crate::logging::OpenAiRouteObserver;
use crate::mesh::{self, ModelWorkloadClass};
use crate::network::openai::client_stream::ClientStream;
use crate::network::openai::request_parse::replace_json_body;
use crate::network::openai::transport::{self as proxy, BufferedHttpRequest, RouteDispatchOutcome};
use crate::network::openai::workload_routing::{eligible_remote_hosts, request_workload_class};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Selection label recorded for requests served by an image endpoint.
const IMAGE_ENDPOINT_PROVIDER: &str = "image-endpoint";
/// Upper bound on `n`, matching the OpenAI Images API.
const MAX_IMAGES_PER_REQUEST: u64 = 10;
/// The only response format the mesh can return: a URL would point at the
/// serving node's local server, which the caller usually cannot reach.
const INLINE_RESPONSE_FORMAT: &str = "b64_json";

/// Where the caller allows a request to be served.
pub(super) struct ImageRouteScope<'a> {
    /// `x-mesh-target`: serve only on this peer.
    pub(super) target: Option<iroh::EndpointId>,
    /// `x-mesh-exclude`: never serve on these peers.
    pub(super) excluded: &'a [iroh::EndpointId],
    /// The request arrived from another node, which already chose this one.
    /// Forwarding it again could bounce it between nodes whose servers are down.
    pub(super) from_peer: bool,
}

impl ImageRouteScope<'_> {
    fn allows(&self, node_id: iroh::EndpointId) -> bool {
        self.target.is_none_or(|target| target == node_id) && !self.excluded.contains(&node_id)
    }

    /// Peers are tried only by the node that received the request from the
    /// client, and only when the client did not pin this node.
    fn may_try_peers(&self) -> bool {
        !self.from_peer && self.target.is_none()
    }
}

/// Whether this node's own image endpoint serves `model` for a request to
/// `path`. The endpoint is not in the native target table, so automatic
/// routing asks here to count it as ready.
pub(super) fn local_endpoint_serves(node: &mesh::Node, model: &str, path: &str) -> bool {
    request_workload_class(path) == Some(ModelWorkloadClass::ImageGeneration)
        && node.image_endpoint_url(model).is_some()
}

/// Serve `model` from this node's image endpoint when it advertises one and
/// the routing scope allows this node. The stream comes back when this node
/// does not serve the model, or when its server failed before answering and a
/// capable peer can still take the request.
// `RouteDispatchOutcome` is deliberately `Copy`; its usage-plus-output-digests variant
// (three optional 32-byte digests inline) exceeds clippy's 128-byte `Err` threshold.
#[allow(clippy::result_large_err)]
pub(super) async fn route_to_local_image_endpoint(
    node: &mesh::Node,
    tcp_stream: ClientStream,
    request: &mut BufferedHttpRequest,
    model: &str,
    scope: ImageRouteScope<'_>,
    route_observer: OpenAiRouteObserver<'_>,
) -> Result<RouteDispatchOutcome, ClientStream> {
    if !scope.allows(node.id()) {
        return Err(tcp_stream);
    }
    let Some(base_url) = node.image_endpoint_url(model) else {
        return Err(tcp_stream);
    };
    let mut tcp_stream =
        match reject_unservable_request(tcp_stream, request, model, route_observer).await {
            Ok(outcome) => return Ok(outcome),
            Err(tcp_stream) => tcp_stream,
        };
    let outcome = proxy::route_http_endpoint_request(
        node,
        Some(model),
        proxy::RouteSelectionMetadata {
            provider: Some(IMAGE_ENDPOINT_PROVIDER),
            engine: None,
        },
        &mut tcp_stream,
        &base_url,
        request,
        route_observer,
    )
    .await;
    if outcome.response_written() || matches!(outcome, RouteDispatchOutcome::Dropped(_)) {
        return Ok(outcome);
    }
    // Nothing reached the client yet, so a capable peer can still answer.
    if scope.may_try_peers() && peer_serves(node, model, &request.client_path, scope.excluded).await
    {
        tracing::info!(
            model,
            endpoint = base_url,
            "image endpoint failed before answering; routing to a peer"
        );
        return Err(tcp_stream);
    }
    let message = format!("image endpoint for model '{model}' failed");
    Ok(response_outcome(
        503,
        proxy::send_503_observed(tcp_stream, &message, route_observer).await,
    ))
}

/// Answer requests the image server must never see, and ask for inline images
/// when the client left `response_format` unset.
// `RouteDispatchOutcome` is deliberately `Copy`; its usage-plus-output-digests variant
// (three optional 32-byte digests inline) exceeds clippy's 128-byte `Err` threshold.
#[allow(clippy::result_large_err)]
async fn reject_unservable_request(
    tcp_stream: ClientStream,
    request: &mut BufferedHttpRequest,
    model: &str,
    route_observer: OpenAiRouteObserver<'_>,
) -> Result<RouteDispatchOutcome, ClientStream> {
    // Endpoints without a workload class (such as `/v1/messages`) pass
    // admission; never relay them to an image server.
    if request_workload_class(&request.client_path) != Some(ModelWorkloadClass::ImageGeneration) {
        let message = format!(
            "model '{model}' only serves image generation; send POST /v1/images/generations"
        );
        return Ok(response_outcome(
            422,
            proxy::send_error_observed(tcp_stream, 422, &message, route_observer).await,
        ));
    }
    request.ensure_body_json();
    let invalid = match request.body_json.as_ref().and_then(invalid_parameter) {
        Some(invalid) => invalid,
        None => match request_inline_images(request) {
            Ok(()) => return Err(tcp_stream),
            Err(invalid) => invalid,
        },
    };
    Ok(response_outcome(
        400,
        proxy::send_json_with_status_and_headers_observed(
            tcp_stream,
            400,
            &invalid.error_body(),
            &[],
            route_observer,
        )
        .await,
    ))
}

async fn peer_serves(
    node: &mesh::Node,
    model: &str,
    path: &str,
    excluded: &[iroh::EndpointId],
) -> bool {
    let hosts = node
        .hosts_for_model(model)
        .await
        .into_iter()
        .filter(|host| !excluded.contains(host))
        .collect::<Vec<_>>();
    !eligible_remote_hosts(node, model, path, &hosts)
        .await
        .is_empty()
}

/// Forward `response_format: "b64_json"` when the client omitted it or sent
/// `null`. Servers such as LocalAI answer with a URL by default.
fn request_inline_images(request: &mut BufferedHttpRequest) -> Result<(), InvalidParameter> {
    let Some(mut body) = request.body_json.clone() else {
        return Ok(());
    };
    let Some(object) = body.as_object_mut() else {
        return Ok(());
    };
    if object
        .get("response_format")
        .is_some_and(|value| !value.is_null())
    {
        return Ok(());
    }
    object.insert("response_format".into(), INLINE_RESPONSE_FORMAT.into());
    if replace_json_body(request, body) {
        return Ok(());
    }
    Err(InvalidParameter {
        param: None,
        message: "image generation requests must use Content-Type: application/json".to_string(),
    })
}

/// A request parameter the mesh cannot honor, reported before generation starts.
#[derive(Debug, PartialEq)]
struct InvalidParameter {
    param: Option<&'static str>,
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
/// server: images come back inline, and the count stays within the OpenAI
/// limit.
fn invalid_parameter(body: &Value) -> Option<InvalidParameter> {
    if let Some(format) = body.get("response_format").filter(|value| !value.is_null())
        && format.as_str() != Some(INLINE_RESPONSE_FORMAT)
    {
        return Some(InvalidParameter {
            param: Some("response_format"),
            message: "response_format must be \"b64_json\": images generated through the mesh are returned inline, not as URLs".to_string(),
        });
    }
    if let Some(n) = body.get("n").filter(|value| !value.is_null())
        && !n
            .as_u64()
            .is_some_and(|n| (1..=MAX_IMAGES_PER_REQUEST).contains(&n))
    {
        return Some(InvalidParameter {
            param: Some("n"),
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
