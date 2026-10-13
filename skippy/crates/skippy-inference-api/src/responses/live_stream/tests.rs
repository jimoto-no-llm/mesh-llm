use axum::response::{IntoResponse, Sse};
use futures_util::stream;
use http_body_util::BodyExt;
use serde_json::json;

use super::*;
use crate::{
    chat::{ChatCompletionChunkChoice, ChatCompletionDelta},
    lifecycle::{
        InferenceBackendOperation, InferenceFrontendRoute, InferenceLifecycleContext,
        InferenceRequestMethod, generate_request_id,
    },
};

fn usage_lifecycle() -> StreamLifecycle {
    StreamLifecycle::new(
        None,
        InferenceLifecycleContext::new(
            generate_request_id(),
            InferenceRequestMethod::Post,
            InferenceFrontendRoute::Responses,
        ),
        InferenceBackendOperation::ResponsesStream,
    )
}

fn tool_call_chunk(model: &str, tool_calls: Value) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: "chatcmpl_tool".to_string(),
        object: "chat.completion.chunk",
        created: 123,
        model: model.to_string(),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta: ChatCompletionDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: Some(tool_calls),
            },
            logprobs: None,
            finish_reason: None,
        }],
        usage: None,
        timings: None,
    }
}

fn state() -> Mutex<ResponseSseState> {
    Mutex::new(ResponseSseState::new("test-model"))
}

/// Parse the actual axum SSE body frames back into `(event, data)` pairs.
fn parsed_events(events: &[Event]) -> Vec<(String, Value)> {
    // Sse requires an owned 'static stream, so clone the events being inspected.
    let events = events.to_vec();
    let body = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime")
        .block_on(async move {
            let events = events.into_iter().map(Ok::<_, std::convert::Infallible>);
            Sse::new(stream::iter(events))
                .into_response()
                .into_body()
                .collect()
                .await
                .expect("SSE body")
                .to_bytes()
        });
    let framed = String::from_utf8(body.to_vec()).expect("UTF-8 SSE body");
    framed
        .split("event: ")
        .filter_map(|block| {
            let (name, rest) = block.split_once('\n')?;
            let data = rest
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap_or("{}");
            Some((
                name.trim().to_string(),
                serde_json::from_str(data).expect("JSON SSE data"),
            ))
        })
        .collect()
}

fn event_names(events: &[Event]) -> Vec<String> {
    parsed_events(events)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn payloads(events: &[Event], event_type: &str) -> Vec<Value> {
    parsed_events(events)
        .into_iter()
        .filter(|(name, _)| name == event_type)
        .map(|(_, data)| data)
        .collect()
}

fn payload(events: &[Event], event_type: &str) -> Value {
    let mut matches = payloads(events, event_type);
    assert_eq!(matches.len(), 1, "expected exactly one {event_type} event");
    matches.pop().unwrap()
}

fn output_items(events: &[Event]) -> Vec<Value> {
    let completed = payload(events, "response.completed");
    completed["response"]["output"]
        .as_array()
        .cloned()
        .expect("completed response carries an output array")
}

#[test]
fn tool_call_stream_reports_function_call_items_without_an_empty_text_item() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{
                "index": 0,
                "id": "call_123",
                "type": "function",
                "function": {"name": "lookup", "arguments": "{\"city\":"}
            }]),
        )),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "function": {"arguments": "\"Sydney\"}"}}]),
        )),
    ));
    events.extend(tail_events(&state, true));

    let names = event_names(&events);
    assert!(
        !names.iter().any(|name| name.contains("output_text")),
        "a tool-call-only stream must not emit text events: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name.contains("content_part")),
        "a tool-call-only stream must not emit content part events: {names:?}"
    );

    let added = payload(&events, "response.output_item.added");
    assert_eq!(added["output_index"], 0);
    assert_eq!(added["item"]["type"], "function_call");
    assert_eq!(added["item"]["status"], "in_progress");
    assert_eq!(added["item"]["call_id"], "call_123");
    assert_eq!(added["item"]["name"], "lookup");
    assert_eq!(added["item"]["arguments"], "");

    let deltas: Vec<String> = payloads(&events, "response.function_call_arguments.delta")
        .iter()
        .map(|delta| delta["delta"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deltas, vec!["{\"city\":", "\"Sydney\"}"]);

    let done = payload(&events, "response.function_call_arguments.done");
    assert_eq!(done["arguments"], "{\"city\":\"Sydney\"}");

    let items = output_items(&events);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "function_call");
    assert_eq!(items[0]["status"], "completed");
    assert_eq!(items[0]["id"], added["item"]["id"]);
    assert_eq!(items[0]["call_id"], "call_123");
    assert_eq!(items[0]["name"], "lookup");
    assert_eq!(items[0]["arguments"], "{\"city\":\"Sydney\"}");
}

#[test]
fn text_and_tool_calls_stream_as_separate_output_items() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(ChatCompletionChunk::delta("test-model", "hello")),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{
                "index": 0,
                "id": "call_7",
                "type": "function",
                "function": {"name": "lookup", "arguments": "{}"}
            }]),
        )),
    ));
    events.extend(tail_events(&state, false));

    let items = output_items(&events);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["content"][0]["text"], "hello");
    assert_eq!(items[1]["type"], "function_call");
    assert_eq!(items[1]["call_id"], "call_7");

    let message_added = &payloads(&events, "response.output_item.added")[0];
    assert_eq!(message_added["output_index"], 0);
    let tool_added = &payloads(&events, "response.output_item.added")[1];
    assert_eq!(tool_added["output_index"], 1);
    assert_eq!(tool_added["item"]["type"], "function_call");
}

#[test]
fn text_only_stream_keeps_the_single_message_item_shape() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(ChatCompletionChunk::delta("test-model", "hel")),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(ChatCompletionChunk::delta("test-model", "lo")),
    ));
    events.extend(tail_events(&state, false));

    let names = event_names(&events);
    for expected in [
        "response.created",
        "response.output_item.added",
        "response.content_part.added",
        "response.output_text.delta",
        "response.output_text.done",
        "response.content_part.done",
        "response.output_item.done",
        "response.completed",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected} missing from {names:?}"
        );
    }
    assert!(!names.iter().any(|name| name.contains("function_call")));

    let items = output_items(&events);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["content"][0]["text"], "hello");
}

#[test]
fn idle_stream_still_reports_one_empty_message_item() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(ChatCompletionChunk::delta("test-model", "")),
    ));
    events.extend(tail_events(&state, false));

    let items = output_items(&events);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "message");
    assert_eq!(items[0]["content"][0]["text"], "");
}

#[test]
fn parallel_tool_calls_stream_as_separate_output_items() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([
                {"index": 0, "id": "call_1", "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}},
                {"index": 1, "id": "call_2", "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}}
            ]),
        )),
    ));
    events.extend(tail_events(&state, false));

    let items = output_items(&events);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["call_id"], "call_1");
    assert_eq!(items[1]["call_id"], "call_2");
    assert_ne!(items[0]["id"], items[1]["id"]);
}

#[test]
fn metadata_only_fragment_opens_the_tool_item_once() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "id": "call_1", "function": {"name": "lookup"}}]),
        )),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "function": {"arguments": "{}"}}]),
        )),
    ));
    events.extend(tail_events(&state, false));

    assert_eq!(payloads(&events, "response.output_item.added").len(), 1);
    assert_eq!(
        payloads(&events, "response.function_call_arguments.delta").len(),
        1
    );
    assert_eq!(output_items(&events)[0]["arguments"], "{}");
}

#[test]
fn late_tool_metadata_replays_arguments_after_announce_with_matching_identity() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "function": {"arguments": "{\"city\":"}}]),
        )),
    );
    assert_eq!(event_names(&events), ["response.created"]);

    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "id": "call_late",
                "function": {"name": "lookup", "arguments": "\"Sydney\"}"}}]),
        )),
    ));
    events.extend(tail_events(&state, false));

    let added = payload(&events, "response.output_item.added");
    let done = payload(&events, "response.output_item.done");
    assert_eq!(added["item"]["call_id"], "call_late");
    assert_eq!(added["item"]["name"], "lookup");
    assert_eq!(done["item"]["call_id"], added["item"]["call_id"]);
    assert_eq!(done["item"]["name"], added["item"]["name"]);
    let deltas = payloads(&events, "response.function_call_arguments.delta");
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0]["delta"], "{\"city\":");
    assert_eq!(deltas[1]["delta"], "\"Sydney\"}");
    assert_eq!(
        output_items(&events)[0]["arguments"],
        "{\"city\":\"Sydney\"}"
    );
}

#[test]
fn interleaved_tool_and_text_deltas_keep_first_appearance_order() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 7, "id": "call_seven",
                "function": {"name": "lookup", "arguments": "{"}}]),
        )),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(ChatCompletionChunk::delta("test-model", "hi")),
    ));
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([
                {"index": 2, "id": "call_two", "function": {"name": "lookup", "arguments": "{}"}},
                {"index": 7, "function": {"arguments": "}"}}
            ]),
        )),
    ));
    events.extend(tail_events(&state, false));

    let items = output_items(&events);
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["call_id"], "call_seven");
    assert_eq!(items[0]["arguments"], "{}");
    assert_eq!(items[1]["type"], "message");
    assert_eq!(items[1]["content"][0]["text"], "hi");
    assert_eq!(items[2]["call_id"], "call_two");
    assert_eq!(
        payload(&events, "response.output_text.delta")["output_index"],
        1
    );
    let added = payloads(&events, "response.output_item.added");
    let indices: Vec<i64> = added
        .iter()
        .map(|event| event["output_index"].as_i64().unwrap())
        .collect();
    assert_eq!(indices, [0, 1, 2]);
}

#[test]
fn missing_upstream_call_id_falls_back_like_the_buffered_translation() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{
                "index": 0,
                "type": "function",
                "function": {"name": "lookup", "arguments": "{}"}
            }]),
        )),
    ));
    events.extend(tail_events(&state, false));

    let completed = payload(&events, "response.completed");
    let created_at = completed["response"]["created_at"].as_i64().unwrap();
    let items = output_items(&events);
    assert_eq!(items[0]["call_id"], format!("call_{created_at}_0"));
    assert_eq!(items[0]["id"], format!("fc_{created_at}_0"));
}

#[test]
fn tool_call_count_is_bounded_with_an_invalid_request_error() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    for index in 0..MAX_STREAMED_TOOL_CALLS as i64 {
        events.extend(body_events(
            &state,
            &lifecycle,
            Ok(tool_call_chunk(
                "test-model",
                json!([{"index": index, "id": format!("call_{index}"),
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}}]),
            )),
        ));
    }
    assert!(
        payloads(&events, "error").is_empty(),
        "exactly {MAX_STREAMED_TOOL_CALLS} tool calls must still stream"
    );

    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": MAX_STREAMED_TOOL_CALLS as i64, "id": "call_over",
                "type": "function",
                "function": {"name": "lookup", "arguments": "{}"}}]),
        )),
    ));
    let errors = payloads(&events, "error");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["error"]["type"], "invalid_request_error");
    assert!(
        errors[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("too many streamed tool calls")
    );

    // The stream is failed: no tail follows and later chunks are dropped.
    assert!(tail_events(&state, true).is_empty());
    assert!(
        body_events(
            &state,
            &lifecycle,
            Ok(ChatCompletionChunk::delta("test-model", "hi"))
        )
        .is_empty()
    );
}

#[test]
fn tool_call_arguments_are_bounded_with_an_invalid_request_error() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    let big = "a".repeat(MAX_STREAMED_TOOL_ARGUMENTS_BYTES);
    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": "lookup", "arguments": big}}]),
        )),
    ));
    assert!(
        payloads(&events, "error").is_empty(),
        "arguments exactly at the byte budget must still stream"
    );

    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "function": {"arguments": "b"}}]),
        )),
    ));
    let errors = payloads(&events, "error");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["error"]["type"], "invalid_request_error");
    assert!(tail_events(&state, true).is_empty());
}

#[test]
fn anonymous_argument_fragments_are_bounded_with_an_invalid_request_error() {
    let state = state();
    let lifecycle = usage_lifecycle();
    let mut events = Vec::new();
    // No id or name: every fragment waits in the pending backlog, which must
    // not grow without bound.
    for _ in 0..MAX_PENDING_ARGUMENT_DELTAS {
        events.extend(body_events(
            &state,
            &lifecycle,
            Ok(tool_call_chunk(
                "test-model",
                json!([{"index": 0, "function": {"arguments": "x"}}]),
            )),
        ));
    }
    assert!(
        payloads(&events, "error").is_empty(),
        "a backlog exactly at the fragment budget must still stream"
    );

    events.extend(body_events(
        &state,
        &lifecycle,
        Ok(tool_call_chunk(
            "test-model",
            json!([{"index": 0, "function": {"arguments": "x"}}]),
        )),
    ));
    let errors = payloads(&events, "error");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["error"]["type"], "invalid_request_error");
    assert!(tail_events(&state, true).is_empty());
}

#[test]
fn failed_streams_emit_no_tail() {
    let state = state();
    state.lock().expect("stream state lock").failed = true;

    assert!(tail_events(&state, true).is_empty());
}
