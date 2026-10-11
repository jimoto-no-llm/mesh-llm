//! Already-running OpenAI-compatible image generation servers shared with the mesh.
//!
//! An operator names each server with `--image-endpoint NAME=URL`. While the
//! server answers its model listing, this node advertises `NAME` with the
//! image generation workload, so peers route `POST /v1/images/generations`
//! here and the OpenAI ingress forwards those requests to the server. Mesh
//! never starts or stops the server and never loads its weights; generation
//! runs on this node only.

use crate::mesh::{self, ModelWorkloadClass, ServedModelDescriptor};
use anyhow::{Result, bail};
use mesh_llm_events::{OutputEvent, emit_event};
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;
use url::Url;

#[cfg(test)]
mod tests;

/// Same cadence as plugin inference endpoint health checks.
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Consecutive failed probes before an advertised endpoint is withdrawn, so a
/// single slow answer does not flap the advertisement.
const FAILURE_THRESHOLD: u32 = 2;

/// One operator-declared image generation server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageEndpoint {
    /// Model name advertised to the mesh and matched against the request's `model`.
    pub(crate) model: String,
    /// HTTP base URL of the server, such as `http://127.0.0.1:1234/v1`.
    pub(crate) base_url: String,
}

impl ImageEndpoint {
    /// Validate one `NAME=URL` pair from `--image-endpoint`.
    pub(crate) fn parse(model: &str, url: &str) -> Result<Self> {
        validate_model_name(model)?;
        let base_url = validate_base_url(model, url)?;
        Ok(Self {
            model: model.to_string(),
            base_url: base_url.to_string(),
        })
    }
}

/// Validate `--image-endpoint` before the node starts; only serving nodes
/// host endpoints.
pub(crate) fn image_endpoints_from_options(
    options: &crate::RuntimeOptions,
) -> Result<Vec<ImageEndpoint>> {
    if options.client && !options.image_endpoints.is_empty() {
        bail!("--image-endpoint needs `mesh-llm serve`; a client node does not serve models");
    }
    parse_image_endpoints(&options.image_endpoints)
}

/// Validate every `--image-endpoint` pair before the node starts serving.
pub(crate) fn parse_image_endpoints(pairs: &[(String, String)]) -> Result<Vec<ImageEndpoint>> {
    let mut endpoints: Vec<ImageEndpoint> = Vec::with_capacity(pairs.len());
    for (model, url) in pairs {
        let endpoint = ImageEndpoint::parse(model, url)?;
        if endpoints
            .iter()
            .any(|existing| existing.model == endpoint.model)
        {
            bail!("--image-endpoint names '{model}' more than once");
        }
        endpoints.push(endpoint);
    }
    Ok(endpoints)
}

fn validate_model_name(model: &str) -> Result<()> {
    if model.is_empty() {
        bail!("--image-endpoint needs a model name before '='");
    }
    if model.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("--image-endpoint model name '{model}' must not contain whitespace");
    }
    // `#` selects a serving profile in model names; the automatic directives
    // never name a concrete model.
    if model.contains('#') {
        bail!("--image-endpoint model name '{model}' must not contain '#'");
    }
    if crate::network::openai::automatic::is_directive(model) {
        bail!("--image-endpoint model name '{model}' is reserved for automatic routing");
    }
    Ok(())
}

/// The ingress forwards over plain HTTP only, like plugin inference endpoints.
fn validate_base_url(model: &str, raw: &str) -> Result<Url> {
    let Ok(url) = Url::parse(raw) else {
        bail!("--image-endpoint URL for '{model}' is not a valid URL");
    };
    if url.scheme() != "http" {
        bail!(
            "--image-endpoint URL for '{model}' must use http://; put a local gateway in front of an https:// server"
        );
    }
    if url.host_str().is_none() {
        bail!("--image-endpoint URL for '{model}' has no host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("--image-endpoint URL for '{model}' must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("--image-endpoint URL for '{model}' must not contain a query or fragment");
    }
    Ok(url)
}

/// Model listing used as the health probe. Mirrors how the ingress maps a
/// forwarded `/v1/...` path onto the base URL.
fn models_url(base_url: &str) -> Option<Url> {
    let mut url = Url::parse(base_url).ok()?;
    let base_path = url.path().trim_end_matches('/').to_string();
    let path = if base_path.ends_with("/v1") {
        format!("{base_path}/models")
    } else {
        format!("{base_path}/v1/models")
    };
    url.set_path(&path);
    Some(url)
}

async fn probe(client: &reqwest::Client, base_url: &str) -> bool {
    let Some(url) = models_url(base_url) else {
        return false;
    };
    match client.get(url.clone()).send().await {
        Ok(response) if response.status().is_success() => true,
        Ok(response) => {
            tracing::debug!(%url, status = %response.status(), "image endpoint probe failed");
            false
        }
        Err(error) => {
            tracing::debug!(%url, %error, "image endpoint probe failed");
            false
        }
    }
}

/// Descriptor advertised for an image endpoint. Only the workload is known:
/// the server reports no architecture, context, or input capabilities.
pub(crate) fn image_generation_descriptor(model: &str) -> ServedModelDescriptor {
    ServedModelDescriptor {
        identity: mesh::ServedModelIdentity {
            model_name: model.to_string(),
            ..Default::default()
        },
        metadata: Some(mesh::ServedModelMetadata {
            workload_class: Some(ModelWorkloadClass::ImageGeneration),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Image endpoints this node currently advertises, keyed by model name.
///
/// Kept apart from the native serving lists, which startup and shutdown
/// replace wholesale; announcements, model listings, and workload admission
/// merge these entries when they read local serving state.
#[derive(Clone, Default)]
pub(crate) struct ImageEndpointRegistry(Arc<RwLock<BTreeMap<String, String>>>);

impl ImageEndpointRegistry {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, String>> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<String, String>> {
        self.0.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn base_url(&self, model: &str) -> Option<String> {
        self.read().get(model).cloned()
    }

    fn models(&self) -> Vec<String> {
        self.read().keys().cloned().collect()
    }

    /// Returns whether the model was not advertised before.
    fn insert(&self, endpoint: &ImageEndpoint) -> bool {
        self.write()
            .insert(endpoint.model.clone(), endpoint.base_url.clone())
            .is_none()
    }

    /// Returns whether the model was advertised before.
    fn remove(&self, model: &str) -> bool {
        self.write().remove(model).is_some()
    }
}

impl mesh::Node {
    /// Base URL of the image endpoint this node advertises for `model`.
    pub(crate) fn image_endpoint_url(&self, model: &str) -> Option<String> {
        self.image_endpoints.base_url(model)
    }

    pub(crate) fn image_endpoint_models(&self) -> Vec<String> {
        self.image_endpoints.models()
    }

    /// Descriptors for advertised image endpoints, skipping names a native
    /// model already describes so one model name never carries two classes.
    pub(crate) fn append_image_endpoint_descriptors(
        &self,
        descriptors: &mut Vec<ServedModelDescriptor>,
    ) {
        for model in self.image_endpoints.models() {
            if !descriptors
                .iter()
                .any(|descriptor| descriptor.identity.model_name == model)
            {
                descriptors.push(image_generation_descriptor(&model));
            }
        }
    }

    pub(crate) async fn advertise_image_endpoint(&self, endpoint: &ImageEndpoint, http_port: u16) {
        if !self.image_endpoints.insert(endpoint) {
            return;
        }
        self.claim_host_role(mesh::HostRoleClaim::ImageEndpoint, http_port)
            .await;
        self.regossip().await;
    }

    pub(crate) async fn withdraw_image_endpoint(&self, model: &str) {
        if !self.image_endpoints.remove(model) {
            return;
        }
        self.release_host_role(mesh::HostRoleClaim::ImageEndpoint)
            .await;
        self.regossip().await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealthTransition {
    /// First answer since startup or since the endpoint was withdrawn.
    Advertise,
    /// Repeated failures while advertised.
    Withdraw,
    /// No answer yet; reported once until the endpoint answers again.
    Unreachable,
    Unchanged,
}

/// Debounced probe results for one endpoint.
#[derive(Default)]
struct EndpointHealth {
    advertised: bool,
    consecutive_failures: u32,
    reported_unreachable: bool,
}

impl EndpointHealth {
    fn observe(&mut self, healthy: bool) -> HealthTransition {
        if healthy {
            self.consecutive_failures = 0;
            self.reported_unreachable = false;
            if self.advertised {
                return HealthTransition::Unchanged;
            }
            self.advertised = true;
            return HealthTransition::Advertise;
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.advertised {
            if self.consecutive_failures < FAILURE_THRESHOLD {
                return HealthTransition::Unchanged;
            }
            // The withdrawal itself tells the operator the endpoint stopped.
            self.advertised = false;
            self.reported_unreachable = true;
            return HealthTransition::Withdraw;
        }
        if self.reported_unreachable {
            return HealthTransition::Unchanged;
        }
        self.reported_unreachable = true;
        HealthTransition::Unreachable
    }
}

/// Probe every endpoint now and then on the health interval, advertising each
/// one while it answers and withdrawing it after repeated failures.
pub(crate) fn spawn_image_endpoint_watch(
    node: mesh::Node,
    endpoints: Vec<ImageEndpoint>,
    http_port: u16,
) {
    if endpoints.is_empty() {
        return;
    }
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            let _ = emit_event(OutputEvent::Warning {
                message: format!(
                    "Image endpoints disabled: cannot build the health probe client: {error}"
                ),
                context: None,
            });
            return;
        }
    };
    for endpoint in endpoints {
        tokio::spawn(watch_endpoint(
            node.clone(),
            client.clone(),
            endpoint,
            http_port,
        ));
    }
}

async fn watch_endpoint(
    node: mesh::Node,
    client: reqwest::Client,
    endpoint: ImageEndpoint,
    http_port: u16,
) {
    let mut health = EndpointHealth::default();
    let mut interval = tokio::time::interval(HEALTH_CHECK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let transition = health.observe(probe(&client, &endpoint.base_url).await);
        // Operator-facing: the default runtime log filter drops host-runtime
        // `tracing` warnings, so status changes use the output surface.
        if let Some(event) = status_event(transition, &endpoint) {
            let _ = emit_event(event);
        }
        match transition {
            HealthTransition::Advertise => {
                node.advertise_image_endpoint(&endpoint, http_port).await
            }
            HealthTransition::Withdraw => node.withdraw_image_endpoint(&endpoint.model).await,
            HealthTransition::Unreachable | HealthTransition::Unchanged => {}
        }
    }
}

fn status_event(transition: HealthTransition, endpoint: &ImageEndpoint) -> Option<OutputEvent> {
    let ImageEndpoint { model, base_url } = endpoint;
    match transition {
        HealthTransition::Advertise => Some(OutputEvent::Info {
            message: format!(
                "Image endpoint '{model}' is answering at {base_url}; serving image generation"
            ),
            context: None,
        }),
        HealthTransition::Withdraw => Some(OutputEvent::Warning {
            message: format!(
                "Image endpoint '{model}' stopped answering at {base_url}; withdrew it from the mesh"
            ),
            context: None,
        }),
        HealthTransition::Unreachable => Some(OutputEvent::Warning {
            message: format!(
                "Image endpoint '{model}' does not answer GET {}; retrying every {}s",
                models_url(base_url).map_or_else(|| base_url.clone(), |url| url.to_string()),
                HEALTH_CHECK_INTERVAL.as_secs()
            ),
            context: None,
        }),
        HealthTransition::Unchanged => None,
    }
}
