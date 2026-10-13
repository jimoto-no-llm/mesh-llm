//! End-to-end translated Responses SSE fixtures over the public-proxy relay.
use super::*;
use serde_json::{Value, json};
use tokio::{io::AsyncReadExt, net::TcpListener};

async fn relay_frames(frames: Vec<Value>) -> (RouteAttemptResult, Vec<Value>) {
    let body = frames
        .into_iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut client = ClientStream::from(socket);
        let mut reader = std::io::Cursor::new(body.into_bytes());
        relay_translated_responses_stream(
            &mut client,
            &mut reader,
            ResponseProbe {
                buffered: header.to_vec(),
                header_end: header.len(),
                status_code: 200,
                retryable_context_overflow: false,
            },
            ResponseRetryPolicy::next_target_available(false),
            None,
            OpenAiRouteObserver::default(),
        )
        .await
        .unwrap()
    });
    let mut socket = ClientStream::connect(addr).await.unwrap();
    let mut wire = String::new();
    socket.read_to_string(&mut wire).await.unwrap();
    let events = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).expect("valid JSON SSE event"))
        .collect();
    (server.await.unwrap(), events)
}

fn events_of_type<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event["type"] == kind)
        .collect()
}

/// Drive the relay like [`relay_frames`], but keep the fallible result and
/// the raw wire so limit rejections (which abort the stream) can be asserted.
async fn relay_frames_result(frames: Vec<Value>) -> (anyhow::Result<RouteAttemptResult>, String) {
    let body = frames
        .into_iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>()
        + "data: [DONE]\n\n";
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut client = ClientStream::from(socket);
        let mut reader = std::io::Cursor::new(body.into_bytes());
        relay_translated_responses_stream(
            &mut client,
            &mut reader,
            ResponseProbe {
                buffered: header.to_vec(),
                header_end: header.len(),
                status_code: 200,
                retryable_context_overflow: false,
            },
            ResponseRetryPolicy::next_target_available(false),
            None,
            OpenAiRouteObserver::default(),
        )
        .await
    });
    let mut socket = ClientStream::connect(addr).await.unwrap();
    let mut wire = String::new();
    socket.read_to_string(&mut wire).await.unwrap();
    (server.await.unwrap(), wire)
}

#[tokio::test]
async fn text_stream_preserves_chunk_deltas_and_completed_usage() {
    let (result, events) = relay_frames(vec![
        json!({"model":"qwen","choices":[{"delta":{"content":"Hello"}}]}),
        json!({"model":"qwen","choices":[{"delta":{"content":" world"}}]}),
        json!({"model":"qwen","choices":[{"delta":{"content":"!"}}]}),
        json!({"model":"qwen","choices":[{"delta":{},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":5,"completion_tokens":13,"total_tokens":18}}),
    ])
    .await;
    assert_eq!(
        events_of_type(&events, "response.output_text.delta").len(),
        3
    );
    assert_eq!(
        events_of_type(&events, "response.output_item.added").len(),
        1
    );
    let completed = events_of_type(&events, "response.completed")[0];
    assert_eq!(completed["response"]["output_text"], "Hello world!");
    assert_eq!(completed["response"]["output"][0]["type"], "message");
    assert_eq!(completed["response"]["usage"]["output_tokens"], 13);
    assert_eq!(
        result,
        RouteAttemptResult::Delivered {
            status_code: 200,
            usage: TokenUsage::from_counts(Some(5), Some(13), Some(18)),
            cache_cost: None,
            output_digests: ExchangeOutputDigests::from_response_value(&json!({
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello world!"}}]
            })),
        }
    );
}

#[tokio::test]
async fn interleaved_tool_calls_keep_first_appearance_order_and_latest_identity() {
    let (result, events) = relay_frames(vec![
            json!({"model":"qwen","choices":[{"index":0,"delta":{"content":"",
                "reasoning_content":"think", "tool_calls":[{"index":7,"id":"call_old",
                    "type":"function","function":{"name":"old_name","arguments":"{\"city\":\""}}]}}]}),
            json!({"model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[
                {"index":0,"id":"call_second","type":"function",
                    "function":{"name":"other","arguments":"{\"unit\":\""}}]}}]}),
            json!({"model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[
                {"index":7,"id":"call_latest","function":{"name":"weather",
                    "arguments":r#"Paris"}"#}},
                {"index":0,"function":{"arguments":r#"C"}"#}}]}}]}),
            json!({"model":"qwen","choices":[{"index":0,"delta":{"content":"Hi"}}]}),
            json!({"model":"qwen","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":6,
                "total_tokens":15,"prompt_tokens_details":{"cached_tokens":3}}}),
        ]).await;
    assert_eq!(events[0]["type"], "response.created");
    let added = events_of_type(&events, "response.output_item.added");
    assert_eq!(
        added
            .iter()
            .map(|event| event["item"]["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["function_call", "function_call", "message"]
    );
    assert_eq!(
        added
            .iter()
            .map(|event| event["output_index"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let deltas = events_of_type(&events, "response.function_call_arguments.delta");
    assert_eq!(
        deltas
            .iter()
            .map(|event| event["output_index"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    let completed = events_of_type(&events, "response.completed")[0];
    let output = completed["response"]["output"].as_array().unwrap();
    assert_eq!(
        output[0]["id"],
        format!("fc_{}_7", completed["response"]["created_at"])
    );
    assert_eq!(output[0]["call_id"], "call_latest");
    assert_eq!(output[0]["name"], "weather");
    assert_eq!(output[0]["arguments"], r#"{"city":"Paris"}"#);
    assert_eq!(output[1]["arguments"], r#"{"unit":"C"}"#);
    assert_eq!(output[2]["content"][0]["text"], "Hi");
    assert_eq!(completed["response"]["output_text"], "Hi");
    assert_eq!(
        completed["response"]["usage"]["input_tokens_details"]["cached_tokens"],
        3
    );
    let done = events_of_type(&events, "response.function_call_arguments.done");
    assert_eq!(
        done.iter()
            .map(|event| event["arguments"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [r#"{"city":"Paris"}"#, r#"{"unit":"C"}"#]
    );
    let item_done = events_of_type(&events, "response.output_item.done");
    assert_eq!(
        item_done
            .iter()
            .map(|event| event["output_index"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(
        events_of_type(&events, "response.reasoning_text.delta")[0]["item_id"],
        output[0]["id"]
    );
    assert_eq!(
        events_of_type(&events, "response.output_text.delta")[0]["output_index"],
        2
    );
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["sequence_number"], (index + 1) as i32);
    }
    let RouteAttemptResult::Delivered {
        usage,
        output_digests,
        ..
    } = result
    else {
        panic!("expected delivered tool stream: {result:?}");
    };
    assert_eq!(usage.unwrap().cached_prompt_tokens, Some(3));
    assert!(output_digests.tool_calls.is_some());
    assert!(output_digests.reasoning.is_some());
}

#[tokio::test]
async fn tool_only_reasoning_stream_has_no_phantom_message_and_matches_buffered_shape() {
    let (result, events) = relay_frames(vec![
        json!({"model":"qwen","choices":[{"index":0,"delta":{"content":"",
                "reasoning_content":"plan", "tool_calls":[{"index":0,"id":"call_1",
                    "function":{"name":"old","arguments":"{"}}]}}]}),
        json!({"model":"qwen","choices":[{"index":0,"delta":{"tool_calls":[
                {"index":0,"id":"call_final","function":{"name":"lookup","arguments":"}"}}]}}]}),
    ])
    .await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    assert!(events_of_type(&events, "response.output_text.delta").is_empty());
    assert!(events_of_type(&events, "response.content_part.added").is_empty());
    let completed = events_of_type(&events, "response.completed")[0];
    let output = completed["response"]["output"].as_array().unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0]["type"], "function_call");
    assert_eq!(output[0]["arguments"], "{}");
    assert_eq!(output[0]["call_id"], "call_final");
    assert_eq!(output[0]["name"], "lookup");
    assert_eq!(
        events_of_type(&events, "response.reasoning_text.delta")[0]["item_id"],
        output[0]["id"]
    );
    let buffered = response_adapter::translate_chat_completion_to_responses(
        json!({"model":"qwen","choices":[{"message":{"role":"assistant","content":"",
                "tool_calls":[{"id":"call_final","type":"function","function":{
                    "name":"lookup","arguments":"{}"}}]}}]})
        .to_string()
        .as_bytes(),
    )
    .unwrap();
    let buffered: Value = serde_json::from_slice(&buffered).unwrap();
    for key in ["type", "call_id", "name", "arguments"] {
        assert_eq!(output[0][key], buffered["output"][0][key]);
    }
}

#[tokio::test]
async fn idle_stream_still_emits_one_empty_message() {
    let (result, events) = relay_frames(vec![]).await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    let completed = events_of_type(&events, "response.completed")[0];
    assert_eq!(completed["response"]["output"].as_array().unwrap().len(), 1);
    assert_eq!(completed["response"]["output"][0]["content"][0]["text"], "");
    assert_eq!(
        events_of_type(&events, "response.output_item.added").len(),
        1
    );
}

#[tokio::test]
async fn text_before_tool_reserves_message_output_index_zero() {
    let (result, events) = relay_frames(vec![
        json!({"model":"qwen","choices":[{"delta":{"content":"Ready", "tool_calls":[{
            "index":3,"id":"call_3","function":{"name":"lookup","arguments":"{}"}
        }]}}]}),
    ])
    .await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    let added = events_of_type(&events, "response.output_item.added");
    assert_eq!(added.len(), 2);
    assert_eq!(added[0]["item"]["type"], "message");
    assert_eq!(added[0]["output_index"], 0);
    assert_eq!(added[1]["item"]["type"], "function_call");
    assert_eq!(added[1]["output_index"], 1);
    let completed = events_of_type(&events, "response.completed");
    let output = &completed[0]["response"]["output"];
    assert_eq!(output[0]["content"][0]["text"], "Ready");
    assert_eq!(output[1]["arguments"], "{}");
}

#[tokio::test]
async fn function_arguments_reach_client_before_upstream_done() {
    use tokio::io::AsyncWriteExt;
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut client = ClientStream::from(socket);
        relay_translated_responses_stream(
            &mut client,
            &mut reader,
            ResponseProbe {
                buffered: header.to_vec(),
                header_end: header.len(),
                status_code: 200,
                retryable_context_overflow: false,
            },
            ResponseRetryPolicy::next_target_available(false),
            None,
            OpenAiRouteObserver::default(),
        )
        .await
        .unwrap()
    });
    let mut client = ClientStream::connect(addr).await.unwrap();
    let first = json!({"model":"qwen","choices":[{"delta":{"tool_calls":[{
        "index":0,"id":"call_live","function":{"name":"lookup","arguments":"{"}
    }]}}]});
    writer
        .write_all(format!("data: {first}\n\n").as_bytes())
        .await
        .unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !String::from_utf8_lossy(&received).contains("response.function_call_arguments.delta")
        {
            let mut chunk = [0u8; 4096];
            let count = client.read(&mut chunk).await.unwrap();
            assert!(count > 0, "relay closed before sending tool arguments");
            received.extend_from_slice(&chunk[..count]);
        }
    })
    .await
    .expect("tool delta must arrive before upstream [DONE]");
    assert!(!String::from_utf8_lossy(&received).contains("response.completed"));
    writer.write_all(b"data: [DONE]\n\n").await.unwrap();
    writer.shutdown().await.unwrap();
    client.read_to_end(&mut received).await.unwrap();
    assert!(String::from_utf8_lossy(&received).contains("response.completed"));
    assert!(matches!(
        server.await.unwrap(),
        RouteAttemptResult::Delivered { .. }
    ));
}

#[tokio::test]
async fn tool_call_count_is_bounded_and_rejected_explicitly() {
    let mut frames: Vec<Value> = (0..MAX_STREAMED_TOOL_CALLS as u64)
        .map(|index| {
            json!({"model":"qwen","choices":[{"delta":{"tool_calls": [{
                "index": index, "id": format!("call_{index}"),
                "function": {"name": "lookup", "arguments": "{}"}
            }]}}]})
        })
        .collect();
    frames.push(json!({"model":"qwen","choices":[{"delta":{"tool_calls": [{
        "index": MAX_STREAMED_TOOL_CALLS as u64, "id": "call_over",
        "function": {"name": "lookup", "arguments": "{}"}
    }]}}]}));
    let (result, wire) = relay_frames_result(frames).await;
    let err = result.expect_err("the 129th tool call must abort the relay");
    assert!(
        err.to_string().contains("too many streamed tool calls"),
        "unexpected error: {err}"
    );
    assert!(
        wire.contains("invalid_request_error"),
        "the client must see the 400 error body: {wire}"
    );
    assert!(!wire.contains("response.completed"));
}

#[tokio::test]
async fn tool_call_arguments_are_bounded_and_rejected_explicitly() {
    let budget = "a".repeat(MAX_STREAMED_TOOL_ARGUMENTS_BYTES);
    let (result, wire) = relay_frames_result(vec![
        json!({"model":"qwen","choices":[{"delta":{"tool_calls": [{
            "index": 0, "id": "call_1",
            "function": {"name": "lookup", "arguments": budget}
        }]}}]}),
        json!({"model":"qwen","choices":[{"delta":{"tool_calls": [{
            "index": 0, "function": {"arguments": "b"}
        }]}}]}),
    ])
    .await;
    let err = result.expect_err("arguments past the byte budget must abort the relay");
    assert!(
        err.to_string()
            .contains("streamed tool call arguments exceed"),
        "unexpected error: {err}"
    );
    assert!(wire.contains("invalid_request_error"));
    assert!(!wire.contains("response.completed"));
}

#[tokio::test]
async fn reasoning_dribble_without_items_streams_live() {
    // Reasoning arrives before any output slot opens; deltas flush live
    // through an opened message item instead of accumulating, so memory
    // stays bounded by one fragment and the cap is never reached.
    let dribble = "r".repeat(1024);
    let frames: Vec<Value> = (0..=(MAX_BUFFERED_REASONING_BYTES / 1024))
        .map(|_| {
            json!({"model":"qwen","choices":[{"delta":{
                "reasoning_content": dribble
            }}]})
        })
        .collect();
    let (result, events) = relay_frames(frames).await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    let reasoning = events_of_type(&events, "response.reasoning_text.delta");
    assert_eq!(reasoning.len(), 1025);
    let added = events_of_type(&events, "response.output_item.added");
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["item"]["type"], "message");
}

#[tokio::test]
async fn reasoning_only_stream_opens_message_item_live() {
    let (result, events) = relay_frames(vec![
        json!({"model":"qwen","choices":[{"delta":{"reasoning_content":"step one"}}]}),
        json!({"model":"qwen","choices":[{"delta":{"reasoning_content":"step two"}}]}),
        json!({"model":"qwen","choices":[{"delta":{},"finish_reason":"stop"}]}),
    ])
    .await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    let reasoning = events_of_type(&events, "response.reasoning_text.delta");
    assert_eq!(reasoning.len(), 2);
    let added_position = events
        .iter()
        .position(|event| event["type"] == "response.output_item.added")
        .expect("message item must open");
    let first_reasoning_position = events
        .iter()
        .position(|event| event["type"] == "response.reasoning_text.delta")
        .expect("reasoning must stream");
    assert!(
        added_position < first_reasoning_position,
        "item must open before the first reasoning delta"
    );
    let added = events_of_type(&events, "response.output_item.added");
    assert_eq!(added[0]["item"]["type"], "message");
    assert_eq!(reasoning[0]["item_id"], added[0]["item"]["id"]);
    assert_eq!(reasoning[1]["item_id"], added[0]["item"]["id"]);
}

#[tokio::test]
async fn tool_call_added_waits_for_identity() {
    let (result, events) = relay_frames(vec![
        json!({"model":"qwen","choices":[{"delta":{"tool_calls":[
            {"index":0,"function":{"arguments":r#"{"a":"#}}
        ]}}]}),
        json!({"model":"qwen","choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"call_real","function":{"name":"lookup"}}
        ]}}]}),
        json!({"model":"qwen","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
    ])
    .await;
    assert!(matches!(result, RouteAttemptResult::Delivered { .. }));
    let added = events_of_type(&events, "response.output_item.added");
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["item"]["call_id"], "call_real");
    assert_eq!(added[0]["item"]["name"], "lookup");
    let deltas = events_of_type(&events, "response.function_call_arguments.delta");
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0]["delta"], r#"{"a":"#);
    let done = events_of_type(&events, "response.output_item.done");
    assert_eq!(done[0]["item"]["call_id"], "call_real");
    assert_eq!(done[0]["item"]["name"], "lookup");
}
