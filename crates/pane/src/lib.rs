//! `pane` is the Glasshouse native harness: a standalone process with its own
//! binary, run and integrated the way any other native harness is -- through
//! a protocol boundary, never a compile-time dependency on the `glasshouse`
//! crate. It builds and tests independently of the rest of the workspace.

pub mod bg;
pub mod commands;
pub mod config;
pub mod contract;
pub mod events;
pub mod glasshouse;
pub mod project;
pub mod prompt;
pub mod rollout;
pub mod ruler;
pub mod runtime;
pub mod sandbox;
pub mod session;
pub mod supervisor;
pub mod tools;
pub mod tui;
pub mod wire;

use std::io::{BufRead, Write};

/// Reads one line from `input` and writes it back to `output`, unchanged.
///
/// Returns `Ok(false)` at end of input so a caller can stop looping without
/// treating a closed stream as an error.
pub fn echo_line(input: &mut impl BufRead, output: &mut impl Write) -> std::io::Result<bool> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(false);
    }
    output.write_all(line.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_a_line_verbatim() {
        let mut input = std::io::Cursor::new(b"hello, pane\n".to_vec());
        let mut output = Vec::new();
        let more = echo_line(&mut input, &mut output).unwrap();
        assert!(more);
        assert_eq!(output, b"hello, pane\n");
    }

    #[test]
    fn reports_end_of_input() {
        let mut input = std::io::Cursor::new(Vec::new());
        let mut output = Vec::new();
        let more = echo_line(&mut input, &mut output).unwrap();
        assert!(!more);
        assert!(output.is_empty());
    }
}
