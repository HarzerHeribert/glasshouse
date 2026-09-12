//! Versioned machine output comes from typed session records, never the TUI.
use std::cell::RefCell;
use std::io::Write;
use std::time::Instant;

use crate::contract::{Block, Message, Role};
use crate::helpers::HelperRecord;
use crate::runtime::outcome::{CellOutcomeKind, CellRecord, Ended};
use crate::telemetry::RequestMeasurement;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
    StreamJson,
}

struct State {
    format: Format,
    sequence: u64,
    session_id: Option<String>,
    events: Vec<Value>,
    answer: Option<String>,
    delivery_error: Option<String>,
    telemetry: Telemetry,
}

#[derive(Default)]
struct Usage {
    requests: u64,
    responses: u64,
    reported_requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    input_reported_requests: u64,
    output_reported_requests: u64,
    cache_read_reported_requests: u64,
    cache_creation_reported_requests: u64,
}

#[derive(Default)]
struct ModelUsage {
    model: String,
    calls: u64,
    usage_known_calls: u64,
    failed_calls: u64,
    usage: Usage,
}

struct Telemetry {
    started: Instant,
    parent: Usage,
    parent_models: Vec<ModelUsage>,
    helpers: Usage,
    helper_calls: u64,
    helper_usage_known_calls: u64,
    helper_failures: u64,
    helper_models: Vec<ModelUsage>,
    preflight_helpers: Vec<Value>,
    cells: u64,
    cell_failures: u64,
    tool_calls: u64,
    tool_failures: u64,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            parent: Usage::default(),
            parent_models: Vec::new(),
            helpers: Usage::default(),
            helper_calls: 0,
            helper_usage_known_calls: 0,
            helper_failures: 0,
            helper_models: Vec::new(),
            preflight_helpers: Vec::new(),
            cells: 0,
            cell_failures: 0,
            tool_calls: 0,
            tool_failures: 0,
        }
    }
}

thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

pub(super) struct Output;
impl Output {
    pub(super) fn start(format: Format) -> Self {
        if format != Format::Text {
            STATE.with(|state| {
                *state.borrow_mut() = Some(State {
                    format,
                    sequence: 0,
                    session_id: None,
                    events: Vec::new(),
                    answer: None,
                    delivery_error: None,
                    telemetry: Telemetry::default(),
                })
            });
        }
        Self
    }

    pub(super) fn finish(self, outcome: &Result<(), String>) -> Result<(), String> {
        STATE.with(|state| {
            if let Some(mut state) = state.borrow_mut().take() {
                if let Some(error) = state.delivery_error.take() {
                    return Err(error);
                }
                let mut result = json!({
                    "schema_version": 1, "type": "result", "sequence": state.sequence,
                    "session_id": state.session_id, "success": outcome.is_ok(),
                    "answer": state.answer, "error": outcome.as_ref().err(),
                    // Additive fields do not change the v1 event contract.
                    "telemetry": telemetry_value(&state.telemetry),
                });
                if state.format == Format::Json {
                    result["events"] = Value::Array(std::mem::take(&mut state.events));
                }
                print_json(&result)?;
            }
            Ok(())
        })
    }
}

fn known_tokens(usage: &Usage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_read_input_tokens)
        .saturating_add(usage.cache_creation_input_tokens)
}

fn usage_value(usage: &Usage) -> Value {
    json!({
        "known_tokens": known_tokens(usage),
        "requests": usage.requests,
        "successful_responses": usage.responses,
        "requests_without_successful_response": usage.requests.saturating_sub(usage.responses),
        "reported_requests": usage.reported_requests,
        "coverage_complete": usage.reported_requests == usage.requests
            && usage.cache_read_reported_requests == usage.reported_requests
            && usage.cache_creation_reported_requests == usage.reported_requests,
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cache_read_input_tokens,
        "cache_creation_input_tokens": usage.cache_creation_input_tokens,
        "input_reported_requests": usage.input_reported_requests,
        "output_reported_requests": usage.output_reported_requests,
        "cache_read_reported_requests": usage.cache_read_reported_requests,
        "cache_creation_reported_requests": usage.cache_creation_reported_requests,
    })
}

fn usage_model_value(model: &ModelUsage) -> Value {
    let mut value = usage_value(&model.usage);
    value["model"] = json!(model.model);
    value
}

fn helper_model_value(model: &ModelUsage) -> Value {
    let mut value = usage_model_value(model);
    value["calls"] = json!(model.calls);
    value["usage_known_calls"] = json!(model.usage_known_calls);
    value["request_count_coverage_complete"] = json!(model.usage_known_calls == model.calls);
    value["failed_calls"] = json!(model.failed_calls);
    value
}

fn telemetry_value(telemetry: &Telemetry) -> Value {
    let provider_requests = telemetry
        .parent
        .requests
        .saturating_add(telemetry.helpers.requests);
    let reported_requests = telemetry
        .parent
        .reported_requests
        .saturating_add(telemetry.helpers.reported_requests);
    let responses = telemetry
        .parent
        .responses
        .saturating_add(telemetry.helpers.responses);
    let mut parent = usage_value(&telemetry.parent);
    parent["models"] = Value::Array(
        telemetry
            .parent_models
            .iter()
            .map(usage_model_value)
            .collect(),
    );
    let mut helpers = usage_value(&telemetry.helpers);
    helpers["calls"] = json!(telemetry.helper_calls);
    helpers["usage_known_calls"] = json!(telemetry.helper_usage_known_calls);
    helpers["request_count_coverage_complete"] =
        json!(telemetry.helper_usage_known_calls == telemetry.helper_calls);
    helpers["coverage_complete"] = json!(
        telemetry.helper_usage_known_calls == telemetry.helper_calls
            && telemetry.helpers.reported_requests == telemetry.helpers.requests
            && telemetry.helpers.cache_read_reported_requests
                == telemetry.helpers.reported_requests
            && telemetry.helpers.cache_creation_reported_requests
                == telemetry.helpers.reported_requests
    );
    helpers["failed_calls"] = json!(telemetry.helper_failures);
    helpers["models"] = Value::Array(
        telemetry
            .helper_models
            .iter()
            .map(helper_model_value)
            .collect(),
    );
    json!({
        "wall_time_ms": telemetry.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        "cells": {"executed": telemetry.cells, "failed": telemetry.cell_failures},
        "tools": {"calls": telemetry.tool_calls, "failures": telemetry.tool_failures},
        "provider_requests": {
            "total": provider_requests,
            "successful_responses": responses,
            "requests_without_successful_response": provider_requests.saturating_sub(responses),
            "reported": reported_requests,
            "coverage_complete": reported_requests == provider_requests
                && telemetry.helper_usage_known_calls == telemetry.helper_calls
                && telemetry.parent.cache_read_reported_requests == telemetry.parent.reported_requests
                && telemetry.parent.cache_creation_reported_requests == telemetry.parent.reported_requests
                && telemetry.helpers.cache_read_reported_requests == telemetry.helpers.reported_requests
                && telemetry.helpers.cache_creation_reported_requests == telemetry.helpers.reported_requests,
        },
        "tokens": {
            "known_total": known_tokens(&telemetry.parent).saturating_add(known_tokens(&telemetry.helpers)),
            "input_tokens": telemetry.parent.input_tokens.saturating_add(telemetry.helpers.input_tokens),
            "output_tokens": telemetry.parent.output_tokens.saturating_add(telemetry.helpers.output_tokens),
            "cache_read_input_tokens": telemetry.parent.cache_read_input_tokens.saturating_add(telemetry.helpers.cache_read_input_tokens),
            "cache_creation_input_tokens": telemetry.parent.cache_creation_input_tokens.saturating_add(telemetry.helpers.cache_creation_input_tokens),
            "parent": parent,
            "helpers": helpers,
        },
        "preflight_helpers": telemetry.preflight_helpers,
    })
}

impl Drop for Output {
    fn drop(&mut self) {
        STATE.with(|state| {
            state.borrow_mut().take();
        });
    }
}

pub(super) fn active() -> bool {
    STATE.with(|state| state.borrow().is_some())
}

fn print_json(value: &Value) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    write_json(&mut stdout, value)
        .map_err(|error| format!("could not deliver machine output: {error}"))
}

fn write_json(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(std::io::Error::other)?;
    writeln!(writer)?;
    writer.flush()
}

fn emit(kind: &str, data: Value) {
    STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            let event = json!({"schema_version": 1, "type": kind, "sequence": state.sequence, "session_id": state.session_id, "data": data});
            state.sequence += 1;
            match state.format {
                Format::Json => state.events.push(event),
                Format::StreamJson => {
                    if state.delivery_error.is_none() {
                        state.delivery_error = print_json(&event).err();
                    }
                }
                Format::Text => {}
            }
        }
    });
}

pub(super) fn session(id: &str) {
    STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.session_id = Some(id.into());
        }
    });
    emit("session_started", json!({"id": id}));
}

fn model<'a>(models: &'a mut Vec<ModelUsage>, name: &str) -> &'a mut ModelUsage {
    let index = models
        .iter()
        .position(|model| model.model == name)
        .unwrap_or_else(|| {
            models.push(ModelUsage {
                model: name.to_string(),
                ..ModelUsage::default()
            });
            models.len() - 1
        });
    &mut models[index]
}

fn add_parent_response(usage: &mut Usage, measurement: &RequestMeasurement) {
    usage.responses = usage.responses.saturating_add(1);
    if let Some(tokens) = measurement.input_tokens {
        usage.input_tokens = usage.input_tokens.saturating_add(tokens);
        usage.input_reported_requests = usage.input_reported_requests.saturating_add(1);
    }
    if let Some(tokens) = measurement.output_tokens {
        usage.output_tokens = usage.output_tokens.saturating_add(tokens);
        usage.output_reported_requests = usage.output_reported_requests.saturating_add(1);
    }
    if measurement.input_tokens.is_some() && measurement.output_tokens.is_some() {
        usage.reported_requests = usage.reported_requests.saturating_add(1);
    }
    if let Some(tokens) = measurement.cached_input_tokens {
        usage.cache_read_input_tokens = usage.cache_read_input_tokens.saturating_add(tokens);
        usage.cache_read_reported_requests = usage.cache_read_reported_requests.saturating_add(1);
    }
    if let Some(tokens) = measurement.cache_creation_input_tokens {
        usage.cache_creation_input_tokens =
            usage.cache_creation_input_tokens.saturating_add(tokens);
        usage.cache_creation_reported_requests =
            usage.cache_creation_reported_requests.saturating_add(1);
    }
}

/// Counts the attempt before transport begins, so an HTTP, protocol, or
/// context-overflow failure remains part of the benchmark denominator.
pub(super) fn parent_request_started(model_name: &str) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.telemetry.parent.requests = state.telemetry.parent.requests.saturating_add(1);
        let model = model(&mut state.telemetry.parent_models, model_name);
        model.calls = model.calls.saturating_add(1);
        model.usage.requests = model.usage.requests.saturating_add(1);
    });
}

/// Adds provider-reported categories for the successful parent response.
/// Missing categories stay visible through their independent coverage counts.
pub(super) fn parent_response(measurement: &RequestMeasurement) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return;
        };
        add_parent_response(&mut state.telemetry.parent, measurement);
        let model = model(&mut state.telemetry.parent_models, &measurement.model);
        if measurement.input_tokens.is_some() && measurement.output_tokens.is_some() {
            model.usage_known_calls = model.usage_known_calls.saturating_add(1);
        }
        add_parent_response(&mut model.usage, measurement);
    });
}

fn add_helper_usage(usage: &mut Usage, record: &HelperRecord) {
    let measured = &record.usage;
    usage.requests = usage.requests.saturating_add(u64::from(measured.requests));
    usage.responses = usage
        .responses
        .saturating_add(u64::from(measured.responses));
    usage.reported_requests = usage
        .reported_requests
        .saturating_add(u64::from(measured.reported_requests));
    usage.input_tokens = usage.input_tokens.saturating_add(measured.input_tokens);
    usage.output_tokens = usage.output_tokens.saturating_add(measured.output_tokens);
    usage.cache_read_input_tokens = usage
        .cache_read_input_tokens
        .saturating_add(measured.cache_read_input_tokens);
    usage.cache_creation_input_tokens = usage
        .cache_creation_input_tokens
        .saturating_add(measured.cache_creation_input_tokens);
    usage.input_reported_requests = usage
        .input_reported_requests
        .saturating_add(u64::from(measured.reported_requests));
    usage.output_reported_requests = usage
        .output_reported_requests
        .saturating_add(u64::from(measured.reported_requests));
    usage.cache_read_reported_requests = usage
        .cache_read_reported_requests
        .saturating_add(u64::from(measured.cache_read_reported_requests));
    usage.cache_creation_reported_requests = usage
        .cache_creation_reported_requests
        .saturating_add(u64::from(measured.cache_creation_reported_requests));
}

fn helper(call_site: &str, record: &HelperRecord) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.telemetry.helper_calls = state.telemetry.helper_calls.saturating_add(1);
        if record.usage.coverage_known {
            state.telemetry.helper_usage_known_calls =
                state.telemetry.helper_usage_known_calls.saturating_add(1);
        }
        if !record.outcome.ok {
            state.telemetry.helper_failures = state.telemetry.helper_failures.saturating_add(1);
        }
        add_helper_usage(&mut state.telemetry.helpers, record);
        let model_name = if record.usage.model.is_empty() {
            "unknown"
        } else {
            &record.usage.model
        };
        let model = model(&mut state.telemetry.helper_models, model_name);
        model.calls = model.calls.saturating_add(1);
        if record.usage.coverage_known {
            model.usage_known_calls = model.usage_known_calls.saturating_add(1);
        }
        if !record.outcome.ok {
            model.failed_calls = model.failed_calls.saturating_add(1);
        }
        add_helper_usage(&mut model.usage, record);
        if call_site == "preflight" {
            state.telemetry.preflight_helpers.push(json!({
                "call_site": call_site,
                "record": record,
            }));
        }
    });
    emit("helper", json!({"call_site": call_site, "record": record}));
}

pub(super) fn preflight(record: &HelperRecord) {
    helper("preflight", record);
}

pub(super) fn cell_helpers(records: &[HelperRecord]) {
    for record in records {
        helper("cell", record);
    }
}

pub(super) fn message(message: &Message) {
    if !active() {
        return;
    }
    let blocks: Vec<Value> = message.content.iter().map(|block| match block {
        Block::Text(text) => json!({"type": "text", "text": text}),
        Block::Image { media_type, .. } => json!({"type": "image", "media_type": media_type}),
        Block::ToolUse { id, name, input } => json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        Block::ToolResult { tool_use_id, content, is_error } => json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error}),
    }).collect();
    if message.role == Role::Assistant {
        let texts: Vec<&str> = message
            .content
            .iter()
            .filter_map(|block| {
                if let Block::Text(text) = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            STATE.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    state.answer = Some(texts.join("\n"));
                }
            });
        }
    }
    emit(
        "message",
        json!({"role": message.role.as_str(), "content": blocks}),
    );
}

pub(super) fn cell(record: &CellRecord) {
    if active() {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let state = state.as_mut().expect("active machine output has state");
            state.telemetry.cells = state.telemetry.cells.saturating_add(1);
            if record.outcome == CellOutcomeKind::Threw {
                state.telemetry.cell_failures = state.telemetry.cell_failures.saturating_add(1);
            }
            state.telemetry.tool_calls = state
                .telemetry
                .tool_calls
                .saturating_add(record.calls.len() as u64);
            state.telemetry.tool_failures = state.telemetry.tool_failures.saturating_add(
                record
                    .calls
                    .iter()
                    .filter(|call| !matches!(&call.ended, Ended::Ok))
                    .count() as u64,
            );
        });
        emit(
            "cell",
            serde_json::to_value(record).expect("cell record is serializable"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenWriter {
        fail_write: bool,
    }
    impl Write for BrokenWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.fail_write {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture write failure",
                ))
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture flush failure",
            ))
        }
    }

    #[test]
    fn machine_delivery_propagates_write_and_flush_failures() {
        for fail_write in [true, false] {
            assert!(
                write_json(&mut BrokenWriter { fail_write }, &json!({"success":true})).is_err()
            );
        }
    }

    #[test]
    fn failed_stream_delivery_cannot_finish_as_success() {
        let output = Output::start(Format::StreamJson);
        STATE.with(|state| {
            state.borrow_mut().as_mut().unwrap().delivery_error =
                Some("fixture delivery failure".into())
        });
        assert_eq!(
            output.finish(&Ok(())),
            Err("fixture delivery failure".into())
        );
        assert!(!active());
    }
}
