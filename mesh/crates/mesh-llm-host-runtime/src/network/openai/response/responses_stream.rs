//! Live Responses SSE translation at the Mesh public proxy.
//! Upstream chat chunks are translated without buffering the client response.
use super::super::send::openai_error_body;
use super::*;
use anyhow::Context;
use std::collections::BTreeMap;

/// Response output slots are assigned in first-appearance order, independent
/// of the upstream tool-call indexes used to fold argument fragments.
#[derive(Clone, Copy)]
enum OutputSlot {
    Reasoning,
    Message,
    ToolCall(u64),
}

/// Maximum distinct tool calls folded into one translated Responses stream.
///
/// OpenAI serves at most 128 parallel function calls per response, so more
/// distinct upstream `index` values can only come from crafted fragments.
/// Bounding entries also bounds `slots`, which grows once per tool call.
const MAX_STREAMED_TOOL_CALLS: usize = 128;
/// Maximum folded `function.arguments` bytes retained per tool call. Real
/// argument payloads are kilobytes; 1 MiB leaves ample headroom while
/// bounding per-stream memory no matter how many fragments arrive.
const MAX_STREAMED_TOOL_ARGUMENTS_BYTES: usize = 1024 * 1024;
/// Maximum reasoning bytes buffered before the first output item opens.
/// Reasoning can arrive before any slot exists, so without a cap a fragment
/// dribble could grow `pending_reasoning` without ever emitting an item.
const MAX_BUFFERED_REASONING_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct StreamedResponsesToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
    output_index: i32,
    added_emitted: bool,
}

struct ResponsesStreamRelayState {
    created_at: i64,
    response_id: String,
    item_id: String,
    model: String,
    output_text: String,
    usage: Option<serde_json::Value>,
    observed_usage: Option<TokenUsage>,
    observed_cache_cost: Option<CacheCostObservation>,
    sequence_number: i32,
    created_emitted: bool,
    text_output_index: Option<i32>,
    reasoning_output_index: Option<i32>,
    reasoning_text: String,
    slots: Vec<OutputSlot>,
    tool_calls: BTreeMap<u64, StreamedResponsesToolCall>,
    pending_reasoning: Vec<String>,
    pending_reasoning_bytes: usize,
}

impl ResponsesStreamRelayState {
    fn new() -> Self {
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        Self {
            created_at,
            response_id: format!("resp_{created_at}"),
            item_id: format!("msg_{created_at}"),
            model: String::new(),
            output_text: String::new(),
            usage: None,
            observed_usage: None,
            observed_cache_cost: None,
            sequence_number: 0,
            created_emitted: false,
            text_output_index: None,
            reasoning_output_index: None,
            reasoning_text: String::new(),
            slots: Vec::new(),
            tool_calls: BTreeMap::new(),
            pending_reasoning: Vec::new(),
            pending_reasoning_bytes: 0,
        }
    }

    fn next_sequence_number(&mut self) -> i32 {
        self.sequence_number = self.sequence_number.saturating_add(1);
        self.sequence_number
    }

    fn reserve_slot(&mut self, slot: OutputSlot) -> i32 {
        let output_index = self.slots.len() as i32;
        self.slots.push(slot);
        output_index
    }

    fn reasoning_item_id(&self) -> String {
        format!("rs_{}", self.created_at)
    }

    fn tool_item_id(&self, index: u64) -> String {
        format!("fc_{}_{}", self.created_at, index)
    }

    fn function_call_item(&self, index: u64, status: &str, arguments: &str) -> serde_json::Value {
        let call = &self.tool_calls[&index];
        serde_json::json!({
            "id": self.tool_item_id(index),
            "type": "function_call",
            "status": status,
            "call_id": call.call_id.clone().unwrap_or_else(|| format!("call_{}_{}", self.created_at, index)),
            "name": call.name.as_deref().unwrap_or("tool"),
            "arguments": arguments,
        })
    }
}

/// Relay a streaming chat-completions upstream response translated into Responses-API SSE.
pub(in crate::network::openai::response) async fn relay_translated_responses_stream<
    R: AsyncRead + Unpin,
>(
    tcp_stream: &mut ClientStream,
    reader: &mut R,
    probe: ResponseProbe,
    retry_policy: ResponseRetryPolicy,
    served_by: Option<&str>,
    route_observer: OpenAiRouteObserver<'_>,
) -> Result<RouteAttemptResult> {
    fn should_parse_stream_chunk(data: &str, model_missing: bool, usage_missing: bool) -> bool {
        model_missing
            || usage_missing
            || data.contains("\"delta\"")
            || data.contains("\"content\"")
            || data.contains("\"logprobs\"")
            || data.contains("\"usage\"")
    }

    #[derive(Debug, Default)]
    struct TranslatedStreamProgress {
        done_seen: bool,
        first_chunk_seen: bool,
        upstream_error_seen: bool,
    }

    async fn relay_translated_frame(
        tcp_stream: &mut ClientStream,
        response_capture: &mut Option<OpenAiStreamArtifactCapture>,
        state: &mut ResponsesStreamRelayState,
        progress: &mut TranslatedStreamProgress,
        assembly: &mut StreamedChatAssembly,
        route_observer: &OpenAiRouteObserver<'_>,
        data: &str,
    ) -> Result<()> {
        if data == "[DONE]" {
            progress.done_seen = true;
            return Ok(());
        }
        if sse_data_frame_is_openai_error(data) {
            // The upstream backend framed a mid-stream failure. Relay the
            // frame as-is so the client still sees the error body, but do
            // not treat it as stream progress or terminal success.
            write_captured_sse_event(tcp_stream, response_capture, Some("error"), data).await?;
            progress.upstream_error_seen = true;
            return Ok(());
        }
        // The upstream frames are chat-completions chunks: fold every one, as
        // the chat-completions stream relay does, before the parse filter
        // below skips any, so the digest covers the whole served message.
        assembly.ingest_chunk(data);
        if !should_parse_stream_chunk(data, state.model.is_empty(), state.usage.is_none()) {
            return Ok(());
        }
        state.observed_cache_cost = state
            .observed_cache_cost
            .or_else(|| parse_cache_cost_from_json_body(data.as_bytes()));
        process_translated_responses_frame(tcp_stream, response_capture, state, data).await?;
        if progress.first_chunk_seen {
            route_observer.stream_chunk();
        } else {
            route_observer.stream_first_token();
            progress.first_chunk_seen = true;
        }
        Ok(())
    }

    if retry_policy.context_overflow && probe.retryable_context_overflow {
        return Ok(RouteAttemptResult::RetryableContextOverflow);
    }

    if !(200..300).contains(&probe.status_code) {
        route_observer.stream_error("upstream_status");
        return relay_error_response(tcp_stream, reader, probe, served_by, route_observer).await;
    }

    let parsed = try_parse_response_headers(&probe.buffered)?
        .ok_or_else(|| anyhow!("incomplete HTTP response"))?;
    let mut carry = String::from_utf8_lossy(&probe.buffered[parsed.header_end..]).to_string();
    let mut state = ResponsesStreamRelayState::new();
    let mut header = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nCache-Control: no-cache\r\n",
    );
    append_capsule_nonce_headers(
        &mut header,
        parsed.client_nonce.as_deref(),
        parsed.nonce_origin.as_deref(),
    );
    append_mesh_served_by_header(&mut header, served_by);
    header.push_str("Connection: close\r\n\r\n");
    tcp_stream.write_all(header.as_bytes()).await?;
    let mut response_capture = route_observer.begin_stream_response_capture();
    route_observer.stream_started(None);

    let mut progress = TranslatedStreamProgress::default();
    let mut assembly = StreamedChatAssembly::default();
    loop {
        let mut processed = 0usize;
        while let Some(frame_end_rel) = carry[processed..].find("\n\n") {
            let frame_end = processed + frame_end_rel;
            let frame = &carry[processed..frame_end];
            processed = frame_end + 2;
            let data_lines = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>();
            if data_lines.is_empty() {
                continue;
            }
            let data = data_lines.join("\n");
            relay_translated_frame(
                tcp_stream,
                &mut response_capture,
                &mut state,
                &mut progress,
                &mut assembly,
                &route_observer,
                &data,
            )
            .await?;
            if progress.done_seen {
                break;
            }
        }
        if processed > 0 {
            carry = carry[processed..].to_string();
        }

        if progress.done_seen {
            break;
        }

        let mut chunk = [0u8; 8192];
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let new_data = String::from_utf8_lossy(&chunk[..n]);
        carry.push_str(&new_data);
        // Normalize CRLF so frame parsing works for both LF and CRLF upstreams
        if carry.contains('\r') {
            carry = carry.replace("\r\n", "\n");
        }
    }

    if progress.upstream_error_seen {
        tcp_stream.record_exchange_outcome("backend_error");
        write_captured_sse_event(tcp_stream, &mut response_capture, Some("done"), "[DONE]").await?;
        let _ = tcp_stream.write_all(b"0\r\n\r\n").await;
        let _ = tcp_stream.shutdown().await;
        route_observer.stream_error("upstream_stream_error");
        return Ok(RouteAttemptResult::Delivered {
            status_code: 200,
            usage: None,
            cache_cost: None,
            output_digests: Default::default(),
        });
    }
    if !progress.done_seen {
        tcp_stream.finish_wire_bytes(
            skippy_inference_api::wire_bytes::WireBytesIncomplete::TransportError,
        );
        route_observer.stream_error("upstream_stream_incomplete");
        return Err(anyhow!("upstream Responses stream ended before [DONE]"));
    }
    finish_translated_responses_stream(tcp_stream, &mut response_capture, &mut state).await?;
    write_captured_sse_event(tcp_stream, &mut response_capture, Some("done"), "[DONE]").await?;
    let _ = tcp_stream.write_all(b"0\r\n\r\n").await;
    let _ = tcp_stream.shutdown().await;
    route_observer.complete_stream_response_capture(response_capture);
    route_observer.stream_completed(state.observed_usage);
    Ok(RouteAttemptResult::Delivered {
        status_code: 200,
        usage: state.observed_usage,
        cache_cost: state.observed_cache_cost,
        // The assembly folded every upstream chat chunk before translation.
        // Publish its digest only after a complete [DONE] frame.
        output_digests: assembly.output_digests(),
    })
}

async fn process_translated_responses_frame(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    data: &str,
) -> Result<()> {
    let chunk = skippy_inference_api::parse_chat_stream_chunk(data)
        .context("parse typed upstream chat stream chunk")?;
    update_translated_responses_model(state, &chunk);
    emit_translated_response_created(tcp_stream, capture, state, false).await?;
    queue_translated_reasoning_delta(tcp_stream, capture, state, &chunk).await?;
    flush_translated_reasoning_deltas(tcp_stream, capture, state).await?;
    emit_translated_output_delta(tcp_stream, capture, state, &chunk).await?;
    emit_translated_tool_deltas(tcp_stream, capture, state, &chunk).await?;
    update_translated_responses_usage(state, &chunk);
    Ok(())
}

fn update_translated_responses_model(
    state: &mut ResponsesStreamRelayState,
    chunk: &skippy_inference_api::responses::ChatCompletionStreamChunk,
) {
    if let Some(chunk_model) = chunk.model.as_deref().filter(|_| state.model.is_empty()) {
        state.model = chunk_model.to_string();
    }
}

async fn emit_translated_response_created(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    allow_missing_model: bool,
) -> Result<()> {
    if state.created_emitted || (!allow_missing_model && state.model.is_empty()) {
        return Ok(());
    }
    let sequence_number = state.next_sequence_number();
    let created = serde_json::to_string(
        &response_adapter::responses_stream_created_event_with_sequence(
            &state.model,
            state.created_at,
            sequence_number,
        ),
    )
    .context("serialize response.created stream event")?;
    write_captured_sse_event(tcp_stream, capture, Some("response.created"), &created).await?;
    state.created_emitted = true;
    Ok(())
}

// Reasoning streams live on its own reasoning item, so a tool-only response
// never needs an empty assistant message to carry it.
async fn queue_translated_reasoning_delta(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    chunk: &skippy_inference_api::responses::ChatCompletionStreamChunk,
) -> Result<()> {
    if let Some(delta) = chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.as_ref())
        .and_then(|delta| delta.reasoning_content.as_deref())
    {
        if state.pending_reasoning_bytes + delta.len() > MAX_BUFFERED_REASONING_BYTES {
            return reject_translated_stream_over_limit(
                tcp_stream,
                capture,
                format!("buffered reasoning deltas exceed {MAX_BUFFERED_REASONING_BYTES} bytes"),
            )
            .await;
        }
        state.pending_reasoning_bytes += delta.len();
        state.pending_reasoning.push(delta.to_owned());
    }
    Ok(())
}

/// Reject a translated stream that breached a folding limit. Streaming
/// already committed `200 OK` headers, so a fresh 400 status is impossible;
/// instead relay the same 400 error body `send_400` would have written, then
/// fail the relay so the connection closes without `response.completed`.
/// The protocol is unchanged — `error` events already exist on this path.
async fn reject_translated_stream_over_limit(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    message: String,
) -> Result<()> {
    let body = String::from_utf8(openai_error_body(400, &message))
        .context("serialize stream limit error")?;
    write_captured_sse_event(tcp_stream, capture, Some("error"), &body).await?;
    Err(anyhow!(message))
}

async fn flush_translated_reasoning_deltas(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<()> {
    if state.pending_reasoning.is_empty() {
        return Ok(());
    }
    let output_index = emit_translated_reasoning_prelude(tcp_stream, capture, state).await?;
    let item_id = state.reasoning_item_id();
    state.pending_reasoning_bytes = 0;
    for delta in std::mem::take(&mut state.pending_reasoning) {
        state.reasoning_text.push_str(&delta);
        let sequence_number = state.next_sequence_number();
        let event = with_output_index(
            response_adapter::responses_stream_reasoning_delta_event_with_sequence(
                &item_id,
                &delta,
                sequence_number,
            ),
            output_index,
        );
        emit_translated_event(tcp_stream, capture, "response.reasoning_text.delta", event).await?;
    }
    Ok(())
}

async fn emit_translated_reasoning_prelude(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<i32> {
    if let Some(output_index) = state.reasoning_output_index {
        return Ok(output_index);
    }
    let output_index = state.reserve_slot(OutputSlot::Reasoning);
    state.reasoning_output_index = Some(output_index);
    let sequence_number = state.next_sequence_number();
    let added = serde_json::json!({
        "type": "response.output_item.added",
        "sequence_number": sequence_number,
        "output_index": output_index,
        "item": {"id": state.reasoning_item_id(), "type": "reasoning", "summary": []},
    });
    emit_translated_event(tcp_stream, capture, "response.output_item.added", added).await?;
    Ok(output_index)
}

fn with_output_index(mut event: serde_json::Value, output_index: i32) -> serde_json::Value {
    event["output_index"] = serde_json::json!(output_index);
    event
}

async fn emit_translated_event(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    event_name: &'static str,
    event: serde_json::Value,
) -> Result<()> {
    let payload = serde_json::to_string(&event)
        .with_context(|| format!("serialize {event_name} stream event"))?;
    write_captured_sse_event(tcp_stream, capture, Some(event_name), &payload).await?;
    Ok(())
}

async fn emit_translated_output_delta(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    chunk: &skippy_inference_api::responses::ChatCompletionStreamChunk,
) -> Result<()> {
    let Some(delta) = chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.as_ref())
        .and_then(|delta| delta.content.as_deref())
        .filter(|delta| !delta.is_empty())
    else {
        return Ok(());
    };
    let output_index = emit_translated_text_prelude(tcp_stream, capture, state).await?;
    flush_translated_reasoning_deltas(tcp_stream, capture, state).await?;
    let logprobs = chunk
        .choices
        .first()
        .and_then(|choice| choice.logprobs.clone());
    state.output_text.push_str(delta);
    let sequence_number = state.next_sequence_number();
    let event = with_output_index(
        response_adapter::responses_stream_delta_event_with_logprobs_and_sequence(
            &state.item_id,
            delta,
            logprobs,
            sequence_number,
        ),
        output_index,
    );
    emit_translated_event(tcp_stream, capture, "response.output_text.delta", event).await
}

async fn emit_translated_text_prelude(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<i32> {
    if let Some(output_index) = state.text_output_index {
        return Ok(output_index);
    }
    let output_index = state.reserve_slot(OutputSlot::Message);
    state.text_output_index = Some(output_index);
    let sequence_number = state.next_sequence_number();
    let item_added = with_output_index(
        response_adapter::responses_stream_output_item_added_event(&state.item_id, sequence_number),
        output_index,
    );
    emit_translated_event(
        tcp_stream,
        capture,
        "response.output_item.added",
        item_added,
    )
    .await?;
    let sequence_number = state.next_sequence_number();
    let part_added = with_output_index(
        response_adapter::responses_stream_content_part_added_event(
            &state.item_id,
            sequence_number,
        ),
        output_index,
    );
    emit_translated_event(
        tcp_stream,
        capture,
        "response.content_part.added",
        part_added,
    )
    .await?;
    Ok(output_index)
}

async fn emit_translated_tool_deltas(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    chunk: &skippy_inference_api::responses::ChatCompletionStreamChunk,
) -> Result<()> {
    let Some(tool_calls) = chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.as_ref())
        .and_then(|delta| delta.tool_calls.as_ref())
    else {
        return Ok(());
    };
    let calls = tool_calls
        .as_array()
        .ok_or_else(|| anyhow!("upstream chat stream tool_calls must be an array"))?;
    for (position, fragment) in calls.iter().enumerate() {
        fold_translated_tool_delta(tcp_stream, capture, state, fragment, position).await?;
    }
    Ok(())
}

async fn fold_translated_tool_delta(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    fragment: &serde_json::Value,
    position: usize,
) -> Result<()> {
    if !fragment.is_object() {
        return Err(anyhow!(
            "upstream chat stream tool_calls entry must be an object"
        ));
    }
    let index = fragment
        .get("index")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(position as u64);
    let function = fragment.get("function");
    let name = function
        .and_then(|function| function.get("name"))
        .or_else(|| fragment.get("name"))
        .and_then(serde_json::Value::as_str);
    let arguments = function
        .and_then(|function| function.get("arguments"))
        .or_else(|| fragment.get("arguments"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let opened_here = !state.tool_calls.contains_key(&index);
    if opened_here {
        if state.tool_calls.len() >= MAX_STREAMED_TOOL_CALLS {
            return reject_translated_stream_over_limit(
                tcp_stream,
                capture,
                format!("too many streamed tool calls (limit {MAX_STREAMED_TOOL_CALLS})"),
            )
            .await;
        }
        let output_index = state.reserve_slot(OutputSlot::ToolCall(index));
        state.tool_calls.insert(
            index,
            StreamedResponsesToolCall {
                output_index,
                ..StreamedResponsesToolCall::default()
            },
        );
    }
    let call = state
        .tool_calls
        .get_mut(&index)
        .expect("tool call slot reserved");
    if let Some(id) = fragment.get("id").and_then(serde_json::Value::as_str) {
        call.call_id = Some(id.to_owned());
    }
    if let Some(name) = name {
        call.name = Some(name.to_owned());
    }
    if call.arguments.len() + arguments.len() > MAX_STREAMED_TOOL_ARGUMENTS_BYTES {
        return reject_translated_stream_over_limit(
            tcp_stream,
            capture,
            format!(
                "streamed tool call arguments exceed {MAX_STREAMED_TOOL_ARGUMENTS_BYTES} bytes"
            ),
        )
        .await;
    }
    call.arguments.push_str(arguments);
    let output_index = call.output_index;
    let identity_ready = call.call_id.is_some() && call.name.is_some();
    let already_announced = call.added_emitted;
    // When the identity completes on this fragment, deliver everything
    // folded so far as one delta so no argument is lost.
    let held_arguments = if already_announced {
        None
    } else {
        call.added_emitted = identity_ready;
        identity_ready.then(|| call.arguments.clone())
    };
    let item_id = state.tool_item_id(index);
    // Hold the added event (and argument deltas) until the upstream names
    // the call. Emitting provisional ids breaks client tracking when the
    // real id/name arrive later in done/completed.
    if !already_announced && !identity_ready {
        // Still deliver any pending reasoning live.
        flush_translated_reasoning_deltas(tcp_stream, capture, state).await?;
        return Ok(());
    }
    if !already_announced {
        // The added item starts empty; its done item carries the final
        // folded arguments.
        let item = state.function_call_item(index, "in_progress", "");
        let sequence_number = state.next_sequence_number();
        let added = serde_json::json!({
            "type": "response.output_item.added",
            "sequence_number": sequence_number,
            "output_index": output_index,
            "item": item,
        });
        emit_translated_event(tcp_stream, capture, "response.output_item.added", added).await?;
    }
    flush_translated_reasoning_deltas(tcp_stream, capture, state).await?;
    let delta_arguments = held_arguments.as_deref().unwrap_or(arguments);
    if !delta_arguments.is_empty() {
        let sequence_number = state.next_sequence_number();
        let delta = serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "sequence_number": sequence_number,
            "item_id": item_id,
            "output_index": output_index,
            "delta": delta_arguments,
        });
        emit_translated_event(
            tcp_stream,
            capture,
            "response.function_call_arguments.delta",
            delta,
        )
        .await?;
    }
    Ok(())
}

fn update_translated_responses_usage(
    state: &mut ResponsesStreamRelayState,
    chunk: &skippy_inference_api::responses::ChatCompletionStreamChunk,
) {
    if let Some(usage) = chunk.usage.as_ref() {
        state.usage = Some(response_adapter::stream_usage_to_responses_usage(usage));
        if let Some(authoritative) = TokenUsage::from_counts(
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
        ) {
            state.observed_usage = Some(
                authoritative.with_cached_prompt_tokens(
                    usage
                        .prompt_tokens_details
                        .as_ref()
                        .and_then(|details| details.cached_tokens),
                ),
            );
        }
    }
}

async fn finish_translated_responses_stream(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<()> {
    emit_translated_response_created(tcp_stream, capture, state, true).await?;
    if state.text_output_index.is_none() && state.tool_calls.is_empty() {
        // Empty and reasoning-only completions retain the existing single
        // empty message; tool-only completions must not invent one.
        emit_translated_text_prelude(tcp_stream, capture, state).await?;
    }
    flush_translated_reasoning_deltas(tcp_stream, capture, state).await?;
    let mut output = Vec::with_capacity(state.slots.len());
    for slot in state.slots.clone() {
        let item = match slot {
            OutputSlot::Reasoning => {
                finalize_translated_reasoning(tcp_stream, capture, state).await?
            }
            OutputSlot::Message => finalize_translated_text(tcp_stream, capture, state).await?,
            OutputSlot::ToolCall(index) => {
                finalize_translated_tool(tcp_stream, capture, state, index).await?
            }
        };
        output.push(item);
    }
    let sequence_number = state.next_sequence_number();
    // Reuse the established completed response envelope (usage, defaults,
    // created_at, output_text), replacing only the ordered output items.
    let mut completed = response_adapter::responses_stream_completed_event_with_sequence(
        &state.response_id,
        state.created_at,
        &state.model,
        &state.item_id,
        &state.output_text,
        state.usage.clone(),
        sequence_number,
    );
    completed["response"]["output"] = serde_json::Value::Array(output);
    emit_translated_event(tcp_stream, capture, "response.completed", completed).await
}

async fn finalize_translated_text(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<serde_json::Value> {
    let output_index = state.text_output_index.expect("message slot reserved");
    let sequence_number = state.next_sequence_number();
    let text_done = with_output_index(
        response_adapter::responses_stream_text_done_event_with_sequence(
            &state.item_id,
            &state.output_text,
            sequence_number,
        ),
        output_index,
    );
    emit_translated_event(tcp_stream, capture, "response.output_text.done", text_done).await?;
    let sequence_number = state.next_sequence_number();
    let part_done = with_output_index(
        response_adapter::responses_stream_content_part_done_event(
            &state.item_id,
            &state.output_text,
            sequence_number,
        ),
        output_index,
    );
    emit_translated_event(tcp_stream, capture, "response.content_part.done", part_done).await?;
    let sequence_number = state.next_sequence_number();
    let item_done = with_output_index(
        response_adapter::responses_stream_output_item_done_event(
            &state.item_id,
            &state.output_text,
            sequence_number,
        ),
        output_index,
    );
    let item = item_done["item"].clone();
    emit_translated_event(tcp_stream, capture, "response.output_item.done", item_done).await?;
    Ok(item)
}

async fn finalize_translated_reasoning(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
) -> Result<serde_json::Value> {
    let output_index = state
        .reasoning_output_index
        .expect("reasoning slot reserved");
    let item_id = state.reasoning_item_id();
    let sequence_number = state.next_sequence_number();
    let text_done = serde_json::json!({
        "type": "response.reasoning_text.done",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "content_index": 0,
        "text": state.reasoning_text,
    });
    emit_translated_event(
        tcp_stream,
        capture,
        "response.reasoning_text.done",
        text_done,
    )
    .await?;
    let item = serde_json::json!({
        "id": item_id,
        "type": "reasoning",
        "summary": [],
        "content": [{"type": "reasoning_text", "text": state.reasoning_text}],
    });
    let sequence_number = state.next_sequence_number();
    let item_done = serde_json::json!({
        "type": "response.output_item.done",
        "sequence_number": sequence_number,
        "output_index": output_index,
        "item": item,
    });
    emit_translated_event(tcp_stream, capture, "response.output_item.done", item_done).await?;
    Ok(item)
}

async fn finalize_translated_tool(
    tcp_stream: &mut ClientStream,
    capture: &mut Option<OpenAiStreamArtifactCapture>,
    state: &mut ResponsesStreamRelayState,
    index: u64,
) -> Result<serde_json::Value> {
    if !state
        .tool_calls
        .get(&index)
        .map(|call| call.added_emitted)
        .unwrap_or(false)
    {
        // The upstream never named the call: announce first so done always
        // follows a matching added event.
        let output_index = state.tool_calls[&index].output_index;
        let item = state.function_call_item(index, "in_progress", "");
        let sequence_number = state.next_sequence_number();
        let added = serde_json::json!({
            "type": "response.output_item.added",
            "sequence_number": sequence_number,
            "output_index": output_index,
            "item": item,
        });
        emit_translated_event(tcp_stream, capture, "response.output_item.added", added).await?;
        if let Some(call) = state.tool_calls.get_mut(&index) {
            call.added_emitted = true;
        }
    }
    let call = &state.tool_calls[&index];
    let output_index = call.output_index;
    let arguments = call.arguments.clone();
    let item_id = state.tool_item_id(index);
    let sequence_number = state.next_sequence_number();
    let args_done = serde_json::json!({
        "type": "response.function_call_arguments.done",
        "sequence_number": sequence_number,
        "item_id": item_id,
        "output_index": output_index,
        "arguments": arguments,
    });
    emit_translated_event(
        tcp_stream,
        capture,
        "response.function_call_arguments.done",
        args_done,
    )
    .await?;
    let item = state.function_call_item(index, "completed", &arguments);
    let sequence_number = state.next_sequence_number();
    let item_done = serde_json::json!({
        "type": "response.output_item.done",
        "sequence_number": sequence_number,
        "output_index": output_index,
        "item": item,
    });
    emit_translated_event(tcp_stream, capture, "response.output_item.done", item_done).await?;
    Ok(item)
}

#[cfg(test)]
#[path = "responses_stream_tests.rs"]
mod tests;
