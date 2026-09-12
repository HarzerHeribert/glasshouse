//! Versioned machine output comes from typed session records, never the TUI.
use std::cell::RefCell;
use std::io::Write;

use crate::contract::{Block, Message, Role};
use crate::runtime::outcome::CellRecord;
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
