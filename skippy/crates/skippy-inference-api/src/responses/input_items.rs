//! Translate `/v1/responses` request input into the chat message list.
//!
//! This owns the request side of the Responses compatibility layer: a Responses
//! `input` value (string, content blocks, or typed input items such as
//! `function_call` / `function_call_output`) becomes the `messages` array the
//! chat backend contract expects. It does not own response translation, which
//! lives in [`super`], nor tool schema normalization, which lives in
//! [`super::tools`].

use serde_json::{Map, Value};

use super::tools;
use crate::errors::InferenceError;

/// Message content-block types a Responses `input` array may carry directly.
const CONTENT_BLOCK_TYPES: &[&str] = &[
    "input_text",
    "output_text",
    "text",
    "input_image",
    "image_url",
    "image",
    "input_audio",
    "audio",
    "audio_url",
    "input_file",
    "file",
];

/// How one Responses input element maps onto the chat contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputItemKind {
    /// Carries `role`/`content`, or announces itself with `type: "message"`.
    Message,
    /// A `function_call` item: an assistant tool call.
    FunctionCall,
    /// A `function_call_output` item: the result of an earlier tool call.
    FunctionCallOutput,
    /// A `reasoning` item echoed back from an earlier response. Chat messages
    /// have no place for it, so it is skipped.
    Reasoning,
    /// A message content block (`input_text`, `input_image`, ...).
    ContentBlock,
    /// A `type` this layer does not translate. Rejected, never dropped.
    Unsupported,
}

fn map_response_role(role: &str) -> String {
    match role {
        "developer" => "system".to_string(),
        other => other.to_string(),
    }
}

fn object_or_url_container(
    value: Option<&Value>,
    fallback_url: Option<&str>,
) -> Option<Map<String, Value>> {
    match value {
        Some(Value::Object(map)) => Some(map.clone()),
        Some(Value::String(url)) => Some(Map::from_iter([(
            "url".to_string(),
            Value::String(url.clone()),
        )])),
        _ => fallback_url
            .map(|url| Map::from_iter([("url".to_string(), Value::String(url.to_string()))])),
    }
}

fn input_item_kind(item: &Value) -> InputItemKind {
    let Some(object) = item.as_object() else {
        // A bare string inside an input array is message text.
        return InputItemKind::ContentBlock;
    };
    match object.get("type") {
        Some(Value::String(item_type)) => match item_type.as_str() {
            "message" => InputItemKind::Message,
            "function_call" => InputItemKind::FunctionCall,
            "function_call_output" => InputItemKind::FunctionCallOutput,
            "reasoning" => InputItemKind::Reasoning,
            content_type if CONTENT_BLOCK_TYPES.contains(&content_type) => {
                InputItemKind::ContentBlock
            }
            _ => InputItemKind::Unsupported,
        },
        Some(_) => InputItemKind::Unsupported,
        None if object.contains_key("role") || object.contains_key("content") => {
            InputItemKind::Message
        }
        None => InputItemKind::ContentBlock,
    }
}

fn is_input_item(item: &Value) -> bool {
    matches!(
        input_item_kind(item),
        InputItemKind::Message
            | InputItemKind::FunctionCall
            | InputItemKind::FunctionCallOutput
            | InputItemKind::Reasoning
    )
}

fn unsupported_input_item(item: &Value) -> InferenceError {
    let item_type = match item.get("type") {
        Some(Value::String(item_type)) => item_type.as_str(),
        Some(_) => "<non-string>",
        None => "content block",
    };
    InferenceError::unsupported(format!(
        "unsupported /v1/responses input item type '{item_type}'"
    ))
}

fn required_item_field(
    item: &Value,
    item_type: &str,
    field: &str,
) -> Result<String, InferenceError> {
    item.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            InferenceError::invalid_request(format!(
                "responses {item_type} item is missing {field}"
            ))
        })
}

/// Render a JSON argument/output value as the string form chat tool messages
/// carry: strings pass through, `null` reads as nothing, anything else is
/// serialized.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn function_call_tool_call(item: &Value) -> Result<Value, InferenceError> {
    let call_id = required_item_field(item, "function_call", "call_id")?;
    let name = required_item_field(item, "function_call", "name")?;
    let arguments = item.get("arguments").map(scalar_text).unwrap_or_default();
    Ok(serde_json::json!({
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": arguments},
    }))
}

fn function_call_output_message(item: &Value) -> Result<Value, InferenceError> {
    let call_id = required_item_field(item, "function_call_output", "call_id")?;
    let output = item.get("output").ok_or_else(|| {
        InferenceError::invalid_request("responses function_call_output item is missing output")
    })?;
    // Lists (and single block objects) go through the message content path
    // so text collapses to a string while image/file blocks keep working.
    let output = match output {
        Value::Array(_) | Value::Object(_) => translate_responses_message_content(output)?,
        single => Value::String(scalar_text(single)),
    };
    Ok(serde_json::json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": output,
    }))
}

/// Append the assistant message that carries the tool calls collected so far.
fn flush_tool_calls(pending_calls: &mut Vec<Value>, messages: &mut Vec<Value>) {
    if pending_calls.is_empty() {
        return;
    }
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": Value::Null,
        "tool_calls": std::mem::take(pending_calls),
    }));
}

/// Translate a Responses item list: messages become chat messages,
/// `function_call` items collapse into the assistant tool call message that
/// precedes their results, and `function_call_output` items become the `tool`
/// message that answers them.
fn translate_responses_input_items(items: &[Value]) -> Result<Vec<Value>, InferenceError> {
    let mut messages = Vec::new();
    let mut pending_calls: Vec<Value> = Vec::new();
    for item in items {
        match input_item_kind(item) {
            InputItemKind::Message => {
                flush_tool_calls(&mut pending_calls, &mut messages);
                messages.push(Value::Object(translate_responses_input_message(item)?));
            }
            InputItemKind::FunctionCall => pending_calls.push(function_call_tool_call(item)?),
            InputItemKind::FunctionCallOutput => {
                flush_tool_calls(&mut pending_calls, &mut messages);
                messages.push(function_call_output_message(item)?);
            }
            InputItemKind::Reasoning => {}
            InputItemKind::ContentBlock | InputItemKind::Unsupported => {
                return Err(unsupported_input_item(item));
            }
        }
    }
    flush_tool_calls(&mut pending_calls, &mut messages);
    Ok(messages)
}

fn translate_responses_content_item(item: &Value) -> Result<Value, InferenceError> {
    let Some(object) = item.as_object() else {
        let Some(text) = item.as_str() else {
            return Err(InferenceError::unsupported(
                "unsupported /v1/responses content block shape",
            ));
        };
        return Ok(serde_json::json!({"type": "text", "text": text}));
    };
    let item_type = match object.get("type") {
        Some(Value::String(item_type)) => item_type.as_str(),
        None if object.contains_key("text") => "text",
        None => return Err(unsupported_input_item(item)),
        Some(_) => return Err(unsupported_input_item(item)),
    };

    match item_type {
        "input_text" | "output_text" | "text" => {
            let text = object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(serde_json::json!({"type": "text", "text": text}))
        }
        "input_image" | "image_url" | "image" => {
            let container = object_or_url_container(
                object.get("image_url").or_else(|| object.get("image")),
                object.get("url").and_then(Value::as_str),
            )
            .ok_or_else(|| {
                InferenceError::invalid_request(
                    "responses input_image block is missing image_url/url",
                )
            })?;
            Ok(serde_json::json!({"type": "image_url", "image_url": container}))
        }
        "input_audio" | "audio" | "audio_url" => {
            let mut container = object_or_url_container(
                object
                    .get("input_audio")
                    .or_else(|| object.get("audio_url")),
                object.get("url").and_then(Value::as_str),
            )
            .unwrap_or_default();
            for key in [
                "data",
                "format",
                "mime_type",
                "mesh_token",
                "blob_token",
                "token",
            ] {
                if let Some(value) = object.get(key) {
                    container
                        .entry(key.to_string())
                        .or_insert_with(|| value.clone());
                }
            }
            if container.is_empty() {
                return Err(InferenceError::invalid_request(
                    "responses input_audio block is missing input_audio/audio_url/url",
                ));
            }
            Ok(serde_json::json!({"type": "input_audio", "input_audio": container}))
        }
        "input_file" | "file" => {
            let mut container = object_or_url_container(
                object.get("input_file").or_else(|| object.get("file")),
                object.get("url").and_then(Value::as_str),
            )
            .ok_or_else(|| {
                InferenceError::invalid_request(
                    "responses input_file block is missing input_file/file/url",
                )
            })?;
            for key in [
                "mime_type",
                "file_name",
                "filename",
                "mesh_token",
                "blob_token",
                "token",
            ] {
                if let Some(value) = object.get(key) {
                    container
                        .entry(key.to_string())
                        .or_insert_with(|| value.clone());
                }
            }
            Ok(serde_json::json!({"type": "input_file", "input_file": container}))
        }
        other => Err(InferenceError::unsupported(format!(
            "unsupported /v1/responses content block type '{other}'"
        ))),
    }
}

fn collapse_blocks_if_text_only(blocks: Vec<Value>) -> Value {
    if blocks.len() == 1
        && let Some(text) = blocks[0].get("text").and_then(Value::as_str)
    {
        return Value::String(text.to_string());
    }
    Value::Array(blocks)
}

fn translate_responses_message_content(content: &Value) -> Result<Value, InferenceError> {
    match content {
        Value::String(text) => Ok(Value::String(text.clone())),
        Value::Array(items) => {
            let blocks = items
                .iter()
                .map(translate_responses_content_item)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(collapse_blocks_if_text_only(blocks))
        }
        Value::Object(_) => Ok(collapse_blocks_if_text_only(vec![
            translate_responses_content_item(content)?,
        ])),
        _ => Err(InferenceError::unsupported(
            "unsupported /v1/responses input content shape",
        )),
    }
}

fn translate_responses_input_message(
    message: &Value,
) -> Result<Map<String, Value>, InferenceError> {
    let Some(object) = message.as_object() else {
        return Err(InferenceError::unsupported(
            "unsupported /v1/responses message shape",
        ));
    };

    let role = map_response_role(object.get("role").and_then(Value::as_str).unwrap_or("user"));
    let content_value = object
        .get("content")
        .map(translate_responses_message_content)
        .transpose()?
        .unwrap_or_else(|| Value::String(String::new()));

    Ok(Map::from_iter([
        ("role".to_string(), Value::String(role)),
        ("content".to_string(), content_value),
    ]))
}

/// Translate a single input element that is not part of an item list.
fn translate_responses_input_value(item: &Value) -> Result<Vec<Value>, InferenceError> {
    match input_item_kind(item) {
        InputItemKind::Message => Ok(vec![Value::Object(translate_responses_input_message(
            item,
        )?)]),
        InputItemKind::FunctionCall => Ok(vec![serde_json::json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": [function_call_tool_call(item)?],
        })]),
        InputItemKind::FunctionCallOutput => Ok(vec![function_call_output_message(item)?]),
        InputItemKind::Reasoning => Ok(Vec::new()),
        InputItemKind::ContentBlock => {
            let content = translate_responses_message_content(item)?;
            Ok(vec![serde_json::json!({
                "role": "user",
                "content": content,
            })])
        }
        InputItemKind::Unsupported => Err(unsupported_input_item(item)),
    }
}

fn translate_responses_content_blocks(items: &[Value]) -> Result<Vec<Value>, InferenceError> {
    let blocks = items
        .iter()
        .map(translate_responses_content_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(vec![serde_json::json!({
        "role": "user",
        "content": collapse_blocks_if_text_only(blocks),
    })])
}

fn translate_responses_input_to_messages(input: &Value) -> Result<Vec<Value>, InferenceError> {
    match input {
        Value::String(text) => Ok(vec![serde_json::json!({
            "role": "user",
            "content": text,
        })]),
        Value::Array(items) => {
            if items.is_empty() {
                return Ok(Vec::new());
            }
            if let Some(unsupported) = items
                .iter()
                .find(|item| input_item_kind(item) == InputItemKind::Unsupported)
            {
                return Err(unsupported_input_item(unsupported));
            }
            if items.iter().any(is_input_item) {
                translate_responses_input_items(items)
            } else {
                translate_responses_content_blocks(items)
            }
        }
        Value::Object(_) => translate_responses_input_value(input),
        _ => Err(InferenceError::unsupported(
            "unsupported /v1/responses input shape",
        )),
    }
}

pub(super) fn translate_openai_responses_input(
    object: &mut Map<String, Value>,
) -> Result<bool, InferenceError> {
    let mut changed = tools::normalize(object)?;
    let mut messages = Vec::new();
    let mut state_cache_key = None;

    if let Some(instructions_value) = object.remove("instructions") {
        if let Some(instructions) = instructions_value.as_str().map(str::trim)
            && !instructions.is_empty()
        {
            messages.push(serde_json::json!({
                "role": "system",
                "content": instructions,
            }));
        }
        changed = true;
    }

    if let Some(input) = object.remove("input") {
        messages.extend(translate_responses_input_to_messages(&input)?);
        changed = true;
    } else if let Some(existing_messages) = object.remove("messages") {
        messages.extend(translate_responses_input_to_messages(&existing_messages)?);
        changed = true;
    }

    if !messages.is_empty() {
        object.insert("messages".to_string(), Value::Array(messages));
    }

    if let Some(value) = object.get("previous_response_id") {
        state_cache_key = value.as_str().map(ToString::to_string);
    }
    if state_cache_key.is_none()
        && let Some(value) = object.get("conversation")
    {
        state_cache_key = responses_conversation_id(value);
    }
    if let Some(cache_key) = state_cache_key {
        object
            .entry("prompt_cache_key".to_string())
            .or_insert(Value::String(cache_key));
    }

    for key in [
        "conversation",
        "include",
        "output",
        "output_text",
        "previous_response_id",
        "store",
        "text",
        "truncation",
    ] {
        if object.remove(key).is_some() {
            changed = true;
        }
    }

    Ok(changed)
}

pub(super) fn responses_conversation_id(value: &Value) -> Option<String> {
    if let Some(id) = value.as_str() {
        return Some(id.to_string());
    }
    let object = value.as_object()?;
    for key in ["id", "conversation_id"] {
        if let Some(id) = object.get(key).and_then(Value::as_str) {
            return Some(id.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_call_output_becomes_tool_message_after_its_call() {
        let input = serde_json::json!([
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "weather"}]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{\"city\":\"Sydney\"}"
            },
            {"type": "function_call_output", "call_id": "call_1", "output": "sunny"}
        ]);

        let messages = translate_responses_input_to_messages(&input)
            .expect("function-call round trip is supported input");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], Value::Null);
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(messages[1]["tool_calls"][0]["function"]["name"], "lookup");
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["arguments"],
            serde_json::json!("{\"city\":\"Sydney\"}")
        );
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["content"], "sunny");
    }

    #[test]
    fn function_call_output_list_uses_content_item_translation() {
        let input = serde_json::json!([
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [{"type": "input_text", "text": "sunny"}]
            }
        ]);

        let messages = translate_responses_input_to_messages(&input)
            .expect("output lists are supported input");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["content"], "sunny");

        let input = serde_json::json!([
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{}"
            },
            {
                "type": "function_call_output",
                "call_id": "call_1",
                "output": [
                    {"type": "input_text", "text": "sunny"},
                    {"type": "input_image", "image_url": "https://example.test/sky.png"}
                ]
            }
        ]);

        let messages = translate_responses_input_to_messages(&input)
            .expect("multimodal output lists are supported input");

        assert_eq!(
            messages[1]["content"],
            serde_json::json!([
                {"type": "text", "text": "sunny"},
                {"type": "image_url", "image_url": {"url": "https://example.test/sky.png"}}
            ])
        );
    }

    #[test]
    fn normalization_accepts_function_call_round_trip_and_preserves_output() {
        let mut body = serde_json::json!({
            "model": "test-model",
            "input": [
                {"type": "message", "role": "user", "content": "weather"},
                {"type": "function_call", "call_id": "call_1", "name": "lookup",
                    "arguments": "{\"city\":\"Sydney\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "sunny"}
            ]
        });

        let outcome = crate::responses::normalize_openai_compat_request("/v1/responses", &mut body)
            .expect("round-trip input normalizes");

        assert_eq!(
            outcome.rewritten_path.as_deref(),
            Some("/v1/chat/completions")
        );
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(body["messages"][2]["content"], "sunny");
        let request: crate::chat::ChatCompletionRequest =
            serde_json::from_value(body).expect("normalized chat request parses");
        request
            .validate()
            .expect("normalized chat request is valid");
    }

    #[test]
    fn echoed_reasoning_items_are_skipped() {
        let input = serde_json::json!([
            {"type": "message", "role": "user", "content": "weather"},
            {"type": "reasoning", "id": "rs_1", "summary": [],
                "content": [{"type": "reasoning_text", "text": "plan"}]},
            {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "sunny"}
        ]);

        let messages = translate_responses_input_to_messages(&input)
            .expect("reasoning items from an earlier response are accepted");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn consecutive_function_calls_share_one_assistant_message() {
        let input = serde_json::json!([
            {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{}"},
            {"type": "function_call", "call_id": "call_2", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "one"},
            {"type": "function_call_output", "call_id": "call_2", "output": "two"}
        ]);

        let messages = translate_responses_input_to_messages(&input)
            .expect("parallel function calls are supported input");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[1]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["tool_call_id"], "call_2");
    }

    #[test]
    fn function_call_output_without_call_id_is_rejected() {
        let input = serde_json::json!([{"type": "function_call_output", "output": "sunny"}]);

        let error =
            translate_responses_input_to_messages(&input).expect_err("call_id links the result");

        assert!(error.message().contains("function_call_output"));
        assert!(error.message().contains("call_id"));
    }

    #[test]
    fn function_call_output_without_output_is_rejected() {
        let input = serde_json::json!([{"type": "function_call_output", "call_id": "call_1"}]);

        let error = translate_responses_input_to_messages(&input)
            .expect_err("missing tool output must not silently become empty");

        assert!(error.message().contains("function_call_output"));
        assert!(error.message().contains("output"));
    }

    #[test]
    fn unsupported_input_item_type_is_rejected_not_dropped() {
        let input = serde_json::json!([
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "item_reference", "id": "msg_1"}
        ]);

        let error =
            translate_responses_input_to_messages(&input).expect_err("unknown item types fail");

        assert_eq!(
            error.message(),
            "unsupported /v1/responses input item type 'item_reference'"
        );
    }

    #[test]
    fn unsupported_typed_items_cannot_hide_in_message_fields() {
        let input = serde_json::json!([
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "item_reference", "role": "assistant", "content": "ignored"}
        ]);

        let error = translate_responses_input_to_messages(&input)
            .expect_err("an unknown type is not a message");

        assert!(error.message().contains("item_reference"));
    }

    #[test]
    fn unsupported_content_items_never_become_empty_text() {
        for input in [
            serde_json::json!([{"type": "item_reference", "id": "msg_1"}]),
            serde_json::json!([{"type": 42}]),
            serde_json::json!([42]),
            serde_json::json!([{"unrecognized": "item"}]),
            serde_json::json!([{"type": "input_text", "text": "hi"}, {"type": 42}]),
            serde_json::json!({"type": "message", "role": "user", "content": [{"type": 42}]}),
        ] {
            let error = translate_responses_input_to_messages(&input)
                .expect_err("unsupported inputs must be rejected");
            assert!(error.message().contains("unsupported"), "{error:?}");
        }
    }

    #[test]
    fn content_block_arrays_still_collapse_into_one_user_message() {
        let input = serde_json::json!([
            {"type": "input_text", "text": "a"},
            {"type": "input_text", "text": "b"}
        ]);

        let messages =
            translate_responses_input_to_messages(&input).expect("content arrays stay valid");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(
            messages[0]["content"],
            serde_json::json!([
                {"type": "text", "text": "a"},
                {"type": "text", "text": "b"}
            ])
        );
    }

    #[test]
    fn chat_style_message_arrays_are_translated_item_by_item() {
        let input = serde_json::json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"}
        ]);

        let messages =
            translate_responses_input_to_messages(&input).expect("chat-style input stays valid");

        assert_eq!(
            Value::Array(messages),
            serde_json::json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"}
            ])
        );
    }

    #[test]
    fn empty_input_array_produces_no_messages() {
        let messages =
            translate_responses_input_to_messages(&serde_json::json!([])).expect("empty input");

        assert!(messages.is_empty());
    }
}
