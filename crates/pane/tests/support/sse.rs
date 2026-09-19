//! One fixture answer, rendered in whichever transport the request asked for.
//!
//! A one-shot helper sets `"stream": true` and reads Server-Sent Events, so
//! that its ceiling can measure silence rather than duration
//! (`wire::SIDE_ERRAND_SILENCE`). Task, supervisor and decision traffic still
//! take the whole response. Rendering the same assistant JSON both ways keeps
//! every expectation about content and usage identical across the two paths
//! — a fixture that answered SSE to everybody would be testing a transport no
//! caller asked for.

use serde_json::Value;

/// The content type and body a fixture should write back, given the request
/// it just read and the complete Messages response it means to send.
pub fn response_for(request: &Value, whole: &str) -> (&'static str, String) {
    if request.get("stream").and_then(Value::as_bool) != Some(true) {
        return ("application/json", whole.to_string());
    }

    let value: Value = serde_json::from_str(whole).expect("fixture reply is a JSON object");
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text: String = blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => block
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            // Only the one-shot helper path streams, and it asserts its spec
            // reaches no tool. A fixture handing a streamed errand anything
            // else is a mistake worth seeing rather than dropping.
            other => panic!("a streamed fixture reply may only carry text blocks, not {other:?}"),
        })
        .collect();

    let stop_reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let usage = value
        .get("usage")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    // The whole usage object rides on `message_start` and `message_delta`
    // carries none. A real provider splits it — input there, output here —
    // and the accumulator merges the two; splitting it here as well would
    // make these fixtures report different numbers over the two transports,
    // which is the one thing this function exists to prevent.
    let events = [
        serde_json::json!({"type":"message_start","message":{"role":"assistant","usage":usage}}),
        serde_json::json!({"type":"ping"}),
        serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}),
        serde_json::json!({"type":"content_block_stop","index":0}),
        serde_json::json!({"type":"message_delta","delta":{"stop_reason":stop_reason}}),
        serde_json::json!({"type":"message_stop"}),
    ];
    let body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    ("text/event-stream", body)
}
