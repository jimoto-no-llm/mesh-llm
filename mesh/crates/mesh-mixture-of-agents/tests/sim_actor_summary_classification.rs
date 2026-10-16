//! Pin the acting pass's worker-summary classification.
//!
//! Issue #225: on a tool-bearing turn the actor's output was already
//! normalized (and its tool contract enforced), but the summary pushed for
//! the acting pass reported `output_kind` and `confidence` as `None`. The
//! reference workers report both fields, so the one pass that actually acts
//! was the only one an operator could not observe.
//!
//! Contract: a successful actor reports the classification of the output it
//! produced; a failed actor reports neither field, so `succeeded == false`
//! and a missing classification stay distinguishable.

use async_trait::async_trait;
use mesh_mixture_of_agents as moa;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Backend that answers with a fixed, explicitly classified envelope.
struct ClassifiedBackend {
    text: String,
    calls: AtomicUsize,
}

impl ClassifiedBackend {
    fn new(text: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            text: text.into(),
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl moa::ModelBackend for ClassifiedBackend {
    async fn chat_completion(
        &self,
        _model: &str,
        _messages: &[Value],
        _tools: Option<&Value>,
        _max_tokens: u32,
        _timeout: Duration,
        _sampling: moa::SamplingParams,
    ) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({
            "choices": [{"message": {"content": self.text}}],
        }))
    }
}

/// Backend that always fails, so the acting pass has no output to classify.
struct FailingBackend {
    calls: AtomicUsize,
}

#[async_trait]
impl moa::ModelBackend for FailingBackend {
    async fn chat_completion(
        &self,
        _model: &str,
        _messages: &[Value],
        _tools: Option<&Value>,
        _max_tokens: u32,
        _timeout: Duration,
        _sampling: moa::SamplingParams,
    ) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("backend down".to_string())
    }
}

fn tool_config(backend: Arc<dyn moa::ModelBackend>, model: &str) -> moa::GatewayConfig {
    moa::GatewayConfig {
        backends: vec![backend],
        models: vec![moa::ModelEntry::new(model, 0)],
        worker_timeout: Duration::from_secs(2),
        hedge_delay: Duration::from_millis(20),
        reducer_timeout: Duration::from_secs(2),
        first_answer_grace: Duration::ZERO,
        strong_patience: Duration::ZERO,
        enable_thinking: None,
        actor_candidates: Vec::new(),
        reference_policy: Default::default(),
        refinement_policy: Default::default(),
    }
}

fn read_file_tool() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read a file",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            }
        }
    }])
}

fn tool_body() -> Value {
    json!({
        "model": "mesh",
        "tools": read_file_tool(),
        "messages": [{"role": "user", "content": "Read README.md"}],
        "max_tokens": 64,
    })
}

fn actor_summary(result: &moa::TurnResult) -> &moa::WorkerSummary {
    result
        .worker_summaries
        .iter()
        .find(|w| w.role == moa::worker::WorkerRole::Reducer)
        .expect("the acting pass must be reported as a Reducer-role summary")
}

#[tokio::test]
async fn successful_actor_summary_reports_its_classification() {
    let backend = ClassifiedBackend::new(
        "kind: answer\nconfidence: 0.8\npayload: The actor answered directly.",
    );
    let config = tool_config(backend.clone(), "actor-8b");

    let result = moa::handle_turn(&config, &tool_body()).await;

    assert_eq!(backend.calls.load(Ordering::SeqCst), 1, "one actor call");

    let actor = actor_summary(&result);
    assert!(actor.succeeded, "the actor returned a usable output");
    assert_eq!(
        actor.output_kind,
        Some(moa::normalize::OutputKind::Answer),
        "the acting pass must report the classification it produced, not None",
    );
    assert_eq!(
        actor.confidence,
        Some(0.8),
        "the acting pass must report the confidence it produced, not None",
    );
}

#[tokio::test]
async fn failed_actor_summary_stays_unclassified() {
    let backend = Arc::new(FailingBackend {
        calls: AtomicUsize::new(0),
    });
    let config = tool_config(backend.clone(), "actor-8b");

    let result = moa::handle_turn(&config, &tool_body()).await;

    assert!(
        backend.calls.load(Ordering::SeqCst) >= 1,
        "the actor was tried"
    );

    let actor = actor_summary(&result);
    assert!(!actor.succeeded, "the failed actor must not report success");
    assert_eq!(
        actor.output_kind, None,
        "a failed actor has no output to classify",
    );
    assert_eq!(actor.confidence, None, "a failed actor has no confidence");
}
