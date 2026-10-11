//! Live `/v1/responses` SSE streaming state.
//!
//! [`ResponseSseState`] folds the upstream chat stream — text deltas and tool
//! call deltas alike — into the Responses event sequence the router serves:
//! text opens one `message` output item, each upstream tool call becomes a
//! `function_call` output item with its own argument deltas, and the terminal
//! `response.completed` carries every output item in emission order.
//!
//! Output items are numbered by first appearance, so `output_index` is the
//! position in [`ResponseSseState::slots`] and stays stable for the whole
//! stream. The buffered Responses translation stays in [`super`]; this module
//! only owns the live (chunk-by-chunk) path.

use std::{collections::BTreeMap, sync::Mutex};

use axum::response::sse::Event;
use serde::Serialize;
use serde_json::Value;

use super::{
    apply_agent_compat_response_defaults, now_unix_secs_i64,
    responses_stream_created_event_with_sequence, usage_to_responses_usage,
};
use crate::{
    InferenceError, InferenceResult, chat::ChatCompletionChunk,
    stream_lifecycle::StreamLifecycle,
};

/// One output item's slot in the response's `output` array.
///
/// Slots are handed out in first-appearance order, so a slot's position is its
/// `output_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum OutputSlot {
    Message,
    ToolCall(i64),
}

/// One upstream tool call's deltas folded across `chat.completion.chunk`s.
///
/// Keyed by the `index` field OpenAI streaming clients use to tell parallel
/// tool calls apart. `id` and `function.name` are overwritten by the latest
/// fragment because providers resend them whole; `function.arguments`
/// accumulates.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
struct StreamedToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
    output_index: i32,
    added_emitted: bool,
    /// Argument fragments wait here until both call id and function name are
    /// known, so the first announced item cannot carry a temporary identity.
    pending_argument_deltas: Vec<String>,
}

/// Maximum distinct tool calls folded into one live Responses stream.
///
/// OpenAI serves at most 128 parallel function calls per response, so more
/// distinct upstream `index` values can only come from crafted fragments.
/// Bounding entries also bounds `slots`, which grows once per tool call.
const MAX_STREAMED_TOOL_CALLS: usize = 128;
/// Maximum folded `function.arguments` bytes retained per tool call (pending
/// deltas share the same bytes, so this caps both). Real argument payloads
/// are kilobytes; 1 MiB leaves ample headroom while bounding per-stream
/// memory no matter how many fragments arrive.
const MAX_STREAMED_TOOL_ARGUMENTS_BYTES: usize = 1024 * 1024;
/// Maximum argument fragments held back per tool call while its id or name
/// is still unknown. Legitimate upstreams send both up front, so the
/// backlog drains on every fold; thousands of anonymous fragments can only
/// come from a fragment dribble aimed at multiplying `String` overhead.
const MAX_PENDING_ARGUMENT_DELTAS: usize = 4096;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponseSseState {
    pub response_id: String,
    pub item_id: String,
    pub created_at: i64,
    pub sequence_number: i32,
    pub model: String,
    pub output_text: String,
    pub usage: Option<Value>,
    pub created_emitted: bool,
    pub failed: bool,
    /// Output slots in first-appearance order; a slot's position is its index.
    slots: Vec<OutputSlot>,
    /// Tool-call deltas folded by upstream `delta.tool_calls[].index`.
    tool_calls: BTreeMap<i64, StreamedToolCall>,
}

impl ResponseSseState {
    pub fn new(model: impl Into<String>) -> Self {
        let created_at = now_unix_secs_i64();
        Self {
            response_id: format!("resp_{created_at}"),
            item_id: format!("msg_{created_at}"),
            created_at,
            sequence_number: 0,
            model: model.into(),
            output_text: String::new(),
            usage: None,
            created_emitted: false,
            failed: false,
            slots: Vec::new(),
            tool_calls: BTreeMap::new(),
        }
    }

    pub fn next_sequence_number(&mut self) -> i32 {
        self.sequence_number = self.sequence_number.saturating_add(1);
        self.sequence_number
    }

    /// Claim the next `output_index` for a new output item.
    fn reserve_slot(&mut self, slot: OutputSlot) -> i32 {
        let output_index = self.slots.len() as i32;
        self.slots.push(slot);
        output_index
    }

    fn slot_index(&self, slot: OutputSlot) -> Option<i32> {
        self.slots
            .iter()
            .position(|existing| *existing == slot)
            .map(|position| position as i32)
    }

    /// The Responses item id a tool call streams under, matching the
    /// `fc_<created>_<index>` ids the buffered translation hands out.
    fn tool_call_item_id(&self, index: i64) -> String {
        format!("fc_{}_{}", self.created_at, index)
    }

    /// The identity a tool call streams with: upstream values when the stream
    /// carried them, the same fallbacks the buffered translation uses when it
    /// did not.
    fn tool_call_identity(&self, index: i64) -> (String, String, String) {
        let fallback_id = format!("call_{}_{}", self.created_at, index);
        match self.tool_calls.get(&index) {
            Some(entry) => (
                entry.call_id.clone().unwrap_or(fallback_id),
                entry.name.clone().unwrap_or_else(|| "tool".to_string()),
                entry.arguments.clone(),
            ),
            None => (fallback_id, "tool".to_string(), String::new()),
        }
    }
}

/// Fold one upstream result into the Responses event sequence.
pub(crate) fn body_events(
    state_machine: &Mutex<ResponseSseState>,
    usage_lifecycle: &StreamLifecycle,
    item: InferenceResult<ChatCompletionChunk>,
) -> Vec<Event> {
    let mut state = state_machine
        .lock()
        .expect("responses stream state lock poisoned");
    if state.failed {
        return Vec::new();
    }
    let mut out = Vec::new();
    match item {
        Ok(chunk) => {
            if !state.created_emitted {
                state.model = chunk.model.clone();
            }
            emit_created(&mut state, &mut out);
            emit_text_delta(&mut state, &mut out, &chunk);
            emit_tool_call_deltas(&mut state, &mut out, &chunk);
            if let Some(usage) = chunk.usage.as_ref() {
                usage_lifecycle.capture_usage(usage);
                state.usage = Some(usage_to_responses_usage(usage));
            }
        }
        Err(error) => {
            state.failed = true;
            push_event(&mut out, "error", error.body());
        }
    }
    out
}

/// Close every open output item and emit `response.completed`.
pub(crate) fn tail_events(
    state_machine: &Mutex<ResponseSseState>,
    include_usage: bool,
) -> Vec<Event> {
    let mut state = state_machine
        .lock()
        .expect("responses stream state lock poisoned");
    let mut out = Vec::new();
    if state.failed {
        return out;
    }
    emit_created(&mut state, &mut out);
    reserve_message_slot(&mut state, &mut out);
    let output = finalize_output_items(&mut state, &mut out);
    let sequence_number = state.next_sequence_number();
    let event = completed_event(&state, output, include_usage, sequence_number);
    push_event(&mut out, "response.completed", event);
    out
}

/// The text delta of the first choice, when the chunk carries one.
pub fn chunk_delta_text(chunk: &ChatCompletionChunk) -> Option<String> {
    chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.content.clone())
}

fn push_event<T: Serialize>(out: &mut Vec<Event>, name: &'static str, data: T) {
    out.push(
        Event::default()
            .event(name)
            .json_data(data)
            .unwrap_or_else(|_| Event::default().data("{}")),
    );
}

fn emit_created(state: &mut ResponseSseState, out: &mut Vec<Event>) {
    if state.created_emitted {
        return;
    }
    let sequence_number = state.next_sequence_number();
    let event = responses_stream_created_event_with_sequence(
        &state.model,
        state.created_at,
        sequence_number,
    );
    push_event(out, "response.created", event);
    state.created_emitted = true;
}

fn emit_text_delta(
    state: &mut ResponseSseState,
    out: &mut Vec<Event>,
    chunk: &ChatCompletionChunk,
) {
    let Some(delta) = chunk_delta_text(chunk) else {
        return;
    };
    if delta.is_empty() {
        // An empty delta opens no item: a tool-call-only stream must not grow
        // an empty message item along the way.
        return;
    }
    if state.slot_index(OutputSlot::Message).is_none() {
        let output_index = state.reserve_slot(OutputSlot::Message);
        emit_message_added(state, out, output_index);
    }
    let output_index = state.slot_index(OutputSlot::Message).unwrap_or(0);
    let logprobs = chunk
        .choices
        .first()
        .and_then(|choice| choice.logprobs.clone());
    state.output_text.push_str(&delta);
    let sequence_number = state.next_sequence_number();
    let event = text_delta_event(
        &state.item_id,
        output_index,
        &delta,
        logprobs,
        sequence_number,
    );
    push_event(out, "response.output_text.delta", event);
}

fn emit_tool_call_deltas(
    state: &mut ResponseSseState,
    out: &mut Vec<Event>,
    chunk: &ChatCompletionChunk,
) {
    let Some(tool_calls) = chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.tool_calls.as_ref())
        .and_then(Value::as_array)
    else {
        return;
    };
    for (position, delta) in tool_calls.iter().enumerate() {
        fold_tool_call_delta(state, out, position as i64, delta);
    }
}

/// Fold one `delta.tool_calls` fragment and reserve an output slot on first
/// appearance. Announce the item once its id and name are both known.
fn fold_tool_call_delta(
    state: &mut ResponseSseState,
    out: &mut Vec<Event>,
    position: i64,
    delta: &Value,
) {
    if state.failed {
        return;
    }
    let index = delta
        .get("index")
        .and_then(Value::as_i64)
        .unwrap_or(position);
    let fragment_call_id = string_field(delta, "id");
    let function = delta.get("function");
    let fragment_name = function.and_then(|function| string_field(function, "name"));
    let arguments = function
        .and_then(|function| string_field(function, "arguments"))
        .unwrap_or_default();

    if !state.tool_calls.contains_key(&index) {
        if state.tool_calls.len() >= MAX_STREAMED_TOOL_CALLS {
            reject_over_limit(
                state,
                out,
                format!(
                    "too many streamed tool calls (limit {MAX_STREAMED_TOOL_CALLS})"
                ),
            );
            return;
        }
        let output_index = state.reserve_slot(OutputSlot::ToolCall(index));
        state.tool_calls.insert(
            index,
            StreamedToolCall {
                output_index,
                ..StreamedToolCall::default()
            },
        );
    }
    let entry = state
        .tool_calls
        .get_mut(&index)
        .expect("tool call entry exists once its slot is reserved");
    if let Some(call_id) = fragment_call_id {
        entry.call_id = Some(call_id);
    }
    if let Some(name) = fragment_name {
        entry.name = Some(name);
    }
    if !arguments.is_empty() {
        // Pending deltas duplicate bytes already folded into `arguments`, so
        // the byte budget below caps both accumulations at once.
        let over_bytes =
            entry.arguments.len() + arguments.len() > MAX_STREAMED_TOOL_ARGUMENTS_BYTES;
        let over_pending =
            entry.pending_argument_deltas.len() >= MAX_PENDING_ARGUMENT_DELTAS;
        if over_bytes || over_pending {
            let message = if over_bytes {
                format!(
                    "streamed tool call arguments exceed \
                     {MAX_STREAMED_TOOL_ARGUMENTS_BYTES} bytes"
                )
            } else {
                format!(
                    "too many pending tool call argument fragments \
                     (limit {MAX_PENDING_ARGUMENT_DELTAS})"
                )
            };
            reject_over_limit(state, out, message);
            return;
        }
        entry.arguments.push_str(&arguments);
        entry.pending_argument_deltas.push(arguments);
    }
    emit_ready_tool_call(state, out, index, false);
}

/// Reject a stream that breached a folding limit: mark it failed so no tail
/// follows, and surface the same `invalid_request` (400) error shape an
/// upstream failure would carry. The protocol is unchanged — `error` events
/// already exist on this path.
fn reject_over_limit(state: &mut ResponseSseState, out: &mut Vec<Event>, message: String) {
    state.failed = true;
    push_event(out, "error", InferenceError::invalid_request(message).body());
}

/// Replay argument fragments after the item is announced. If an upstream
/// omitted id or name entirely, finalize with the same buffered fallbacks.
fn emit_ready_tool_call(
    state: &mut ResponseSseState,
    out: &mut Vec<Event>,
    index: i64,
    at_end: bool,
) {
    let (output_index, added_here, deltas) = {
        let Some(entry) = state.tool_calls.get_mut(&index) else {
            return;
        };
        if !at_end && (entry.call_id.is_none() || entry.name.is_none()) {
            return;
        }
        let added_here = !entry.added_emitted;
        entry.added_emitted = true;
        (
            entry.output_index,
            added_here,
            std::mem::take(&mut entry.pending_argument_deltas),
        )
    };
    let item_id = state.tool_call_item_id(index);
    if added_here {
        let (call_id, name, _) = state.tool_call_identity(index);
        let sequence_number = state.next_sequence_number();
        let event = output_item_added_event(
            output_index,
            &function_call_item(&item_id, "in_progress", &call_id, &name, ""),
            sequence_number,
        );
        push_event(out, "response.output_item.added", event);
    }
    for delta in deltas {
        let sequence_number = state.next_sequence_number();
        let event = arguments_delta_event(&item_id, output_index, &delta, sequence_number);
        push_event(out, "response.function_call_arguments.delta", event);
    }
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn emit_message_added(state: &mut ResponseSseState, out: &mut Vec<Event>, output_index: i32) {
    let item_id = state.item_id.clone();
    let sequence_number = state.next_sequence_number();
    let event = output_item_added_event(
        output_index,
        &message_item_in_progress(&item_id),
        sequence_number,
    );
    push_event(out, "response.output_item.added", event);

    let sequence_number = state.next_sequence_number();
    let event = content_part_added_event(&item_id, output_index, sequence_number);
    push_event(out, "response.content_part.added", event);
}

/// Open the assistant text item exactly when the response has text to report —
/// or nothing else to report at all, so a stream that produced no output still
/// ends on one empty message item the way the buffered translation does.
fn reserve_message_slot(state: &mut ResponseSseState, out: &mut Vec<Event>) {
    let opens_message = !state.output_text.is_empty() || state.tool_calls.is_empty();
    if opens_message && state.slot_index(OutputSlot::Message).is_none() {
        let output_index = state.reserve_slot(OutputSlot::Message);
        emit_message_added(state, out, output_index);
    }
}

fn finalize_output_items(state: &mut ResponseSseState, out: &mut Vec<Event>) -> Vec<Value> {
    let mut output = Vec::new();
    for slot in state.slots.clone() {
        match slot {
            OutputSlot::Message => finalize_message(state, out, &mut output),
            OutputSlot::ToolCall(index) => finalize_tool_call(state, out, &mut output, index),
        }
    }
    output
}

fn finalize_message(state: &mut ResponseSseState, out: &mut Vec<Event>, output: &mut Vec<Value>) {
    let Some(output_index) = state.slot_index(OutputSlot::Message) else {
        return;
    };
    let item_id = state.item_id.clone();
    let text = state.output_text.clone();

    let sequence_number = state.next_sequence_number();
    let event = text_done_event(&item_id, output_index, &text, sequence_number);
    push_event(out, "response.output_text.done", event);

    let sequence_number = state.next_sequence_number();
    let event = content_part_done_event(&item_id, output_index, &text, sequence_number);
    push_event(out, "response.content_part.done", event);

    let sequence_number = state.next_sequence_number();
    let item = message_item_completed(&item_id, &text);
    let event = output_item_done_event(output_index, &item, sequence_number);
    push_event(out, "response.output_item.done", event);
    output.push(item);
}

fn finalize_tool_call(
    state: &mut ResponseSseState,
    out: &mut Vec<Event>,
    output: &mut Vec<Value>,
    index: i64,
) {
    let Some(output_index) = state.slot_index(OutputSlot::ToolCall(index)) else {
        return;
    };
    if !state.tool_calls.contains_key(&index) {
        return;
    }
    emit_ready_tool_call(state, out, index, true);
    let item_id = state.tool_call_item_id(index);
    let (call_id, name, arguments) = state.tool_call_identity(index);

    let sequence_number = state.next_sequence_number();
    let event = arguments_done_event(&item_id, output_index, &arguments, sequence_number);
    push_event(out, "response.function_call_arguments.done", event);

    let sequence_number = state.next_sequence_number();
    let item = function_call_item(&item_id, "completed", &call_id, &name, &arguments);
    let event = output_item_done_event(output_index, &item, sequence_number);
    push_event(out, "response.output_item.done", event);
    output.push(item);
}

fn output_item_added_event(output_index: i32, item: &Value, sequence_number: i32) -> Value {
    serde_json::json!({
        "type": "response.output_item.added",
        "sequence_number": sequence_number,
        "output_index": output_index,
        "item": item,
    })
}

fn output_item_done_event(output_index: i32, item: &Value, sequence_number: i32) -> Value {
    serde_json::json!({
        "type": "response.output_item.done",
        "sequence_number": sequence_number,
        "output_index": output_index,
        "item": item,
    })
}

fn content_part_added_event(item_id: &str, output_index: i32, sequence_number: i32) -> Value {
    serde_json::json!({
        "type": "response.content_part.added",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "part": {"type": "output_text", "text": "", "annotations": []},
    })
}

fn text_delta_event(
    item_id: &str,
    output_index: i32,
    delta: &str,
    logprobs: Option<Value>,
    sequence_number: i32,
) -> Value {
    let mut event = serde_json::json!({
        "type": "response.output_text.delta",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "delta": delta,
    });
    if let Some(logprobs) = logprobs
        && let Some(object) = event.as_object_mut()
    {
        object.insert("logprobs".to_string(), logprobs);
    }
    event
}

fn text_done_event(item_id: &str, output_index: i32, text: &str, sequence_number: i32) -> Value {
    serde_json::json!({
        "type": "response.output_text.done",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "text": text,
    })
}

fn content_part_done_event(
    item_id: &str,
    output_index: i32,
    text: &str,
    sequence_number: i32,
) -> Value {
    serde_json::json!({
        "type": "response.content_part.done",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "part": {"type": "output_text", "text": text, "annotations": []},
    })
}

fn arguments_delta_event(
    item_id: &str,
    output_index: i32,
    delta: &str,
    sequence_number: i32,
) -> Value {
    serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "delta": delta,
    })
}

fn arguments_done_event(
    item_id: &str,
    output_index: i32,
    arguments: &str,
    sequence_number: i32,
) -> Value {
    serde_json::json!({
        "type": "response.function_call_arguments.done",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "arguments": arguments,
    })
}

fn message_item_in_progress(item_id: &str) -> Value {
    serde_json::json!({
        "id": item_id,
        "type": "message",
        "status": "in_progress",
        "role": "assistant",
        "content": [],
    })
}

fn message_item_completed(item_id: &str, text: &str) -> Value {
    serde_json::json!({
        "id": item_id,
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": text,
            "annotations": [],
        }],
    })
}

fn function_call_item(
    item_id: &str,
    status: &str,
    call_id: &str,
    name: &str,
    arguments: &str,
) -> Value {
    serde_json::json!({
        "id": item_id,
        "type": "function_call",
        "status": status,
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

fn completed_event(
    state: &ResponseSseState,
    output: Vec<Value>,
    include_usage: bool,
    sequence_number: i32,
) -> Value {
    let usage = if include_usage {
        state.usage.clone()
    } else {
        None
    };
    let mut event = serde_json::json!({
        "type": "response.completed",
        "sequence_number": sequence_number,
        "response": {
            "id": state.response_id,
            "object": "response",
            "created_at": state.created_at,
            "status": "completed",
            "error": Value::Null,
            "incomplete_details": Value::Null,
            "model": state.model,
            "output": output,
            "output_text": state.output_text,
            "usage": usage.unwrap_or(Value::Null),
        }
    });
    if let Some(response) = event.get_mut("response").and_then(Value::as_object_mut) {
        apply_agent_compat_response_defaults(response, state.created_at);
    }
    event
}

#[cfg(test)]
mod tests;
