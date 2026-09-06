//! The bytes the model receives and the one thing it sends back —
//! `docs/product/pane/model-contract.md`. This module renders the system
//! block and each turn's result message, and extracts the model's program
//! from its reply. It runs nothing and calls nothing outside itself: every
//! function here is a pure string transformation over its own plain input
//! types.
//!
//! **No message this module builds carries a second serialisation of a call
//! and its outcome.** A handle's rendered text arrives already made, in
//! [`CellResult::handle_table`]; this module has no type standing in for a
//! provider-native call-and-answer pair and never produces one.

pub mod declarations;

use crate::tools::registry::{Arg, Tool};

/// `model-contract.md` §2, verbatim. Compared byte for byte by
/// `prompt_bytes.rs::the_preamble_is_the_contracts_verbatim`.
pub const PREAMBLE: &str = "You act by writing TypeScript. Each turn you emit exactly one code block\ntagged `pane`; pane runs it in a persistent V8 isolate and answers with\nwhat your program produced.\n\nTool results are live objects, not text. `await grep(...)` returns an\narray you can filter, index and count in the next line of the same\nprogram. You are shown each object's name and a short preview; you are\nnever shown its payload, and you never need it.\n\nBindings persist. A top-level `const` in one cell is in scope in the\nnext. Redeclaring a name replaces the object and frees the old one.\n\nA cell that runs off the end yields: you get the handle table and another\nturn. A top-level `return` ends the task with that value. Return when the\ntask is answered, not before.\n\nReturning a string answers the person directly — it is rendered and kept\nas your reply, and nothing is asked of you afterwards, so fill it from what\nthe run actually produced rather than from what you expected it to. Do not\nanswer from a call that threw, was refused, was cancelled, or whose guard\ndid not hold: yield instead and say what you found. Call `yieldNow(reason)`\nto hand back from inside a branch; it is a yield, not an error.\n\nA cell that throws is answered, not retried. You get the error, the line,\nand every binding that completed before the throw. Write the next cell.\n\nA call outside this session's sandbox grant throws PermissionDenied. It\nis catchable and it is final: nothing you write widens a grant.";

/// Why the preamble is being replaced: §6's spent task budget, or three
/// prose turns in a row (the primary's addendum of 2026-09-06 — a model that
/// never programs must not spend hundreds of requests to find out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustedReason {
    TaskBudget,
    ThreeTurnsWithoutAProgram,
}

/// The one sentence that replaces the preamble when the task is exhausted —
/// §6's last paragraph, naming the reason. The only permitted action is a
/// top-level `return`.
pub fn exhausted_preamble(reason: ExhaustedReason) -> &'static str {
    match reason {
        ExhaustedReason::TaskBudget => {
            "The task budget is exhausted; the only action this turn may take is a top-level \
             `return`."
        }
        ExhaustedReason::ThreeTurnsWithoutAProgram => {
            "Three turns without a program; the only action this turn may take is a top-level \
             `return`."
        }
    }
}

/// §1's system block: the preamble, one declaration per tool in `tools`'
/// order, then the project's own instructions.
pub fn render_system(instructions: &str, tools: &[&Tool]) -> String {
    let rendered: Vec<String> = tools.iter().map(|tool| render_declaration(tool)).collect();
    format!(
        "{PREAMBLE}\n\n## Tools\n\n{}\n\n{instructions}",
        rendered.join("\n\n")
    )
}

fn render_declaration(tool: &Tool) -> String {
    let entry = declarations::lookup(tool.name())
        .unwrap_or_else(|| panic!("no declarations entry for tool `{}`", tool.name()));
    format!(
        "declare function {name}(a: {params}): Promise<{ret}>;\n// {summary} {purity}\n// @callers program",
        name = tool.name(),
        params = render_params(tool.args()),
        ret = entry.return_type,
        summary = entry.summary,
        purity = declarations::purity_clause(tool.purity()),
    )
}

fn render_params(args: &[Arg]) -> String {
    if args.is_empty() {
        return "{}".to_string();
    }
    let fields: Vec<String> = args
        .iter()
        .map(|arg| {
            let optional_mark = if arg.is_required() { "" } else { "?" };
            format!("{}{optional_mark}: string", arg.name())
        })
        .collect();
    format!("{{{}}}", fields.join("; "))
}

/// One cell's outcome, as this module's own plain input — the wiring
/// package adapts the isolate's real outcome to it. `handle_table` is
/// already-rendered text; this module never builds one.
pub struct CellResult {
    pub cell: u64,
    pub elapsed_ms: u64,
    pub error: Option<ErrorSection>,
    /// Why the cell yielded on purpose — `runtime-contract.md` §9.3's one
    /// line under the cell line. Never rendered beside an error: a throw is
    /// not a yield, whatever else the caller filled in.
    pub yield_reason: Option<String>,
    pub handle_table: String,
    pub stdout_tail: Option<String>,
    pub budget: Budget,
}

/// §6's `## Error` section: the class, the message, where in the model's own
/// program it happened, and up to three in-program frames.
pub struct ErrorSection {
    pub class: String,
    pub message: String,
    /// The line and column inside the model's own program, when the runtime
    /// attributed the throw to one. Absent, no position line is written —
    /// never `line 0, column 0`, which names a place that does not exist.
    pub position: Option<(u64, u64)>,
    pub frames: Vec<String>,
}

impl ErrorSection {
    /// [`position`](Self::position) from a runtime error's own line and
    /// column, which is where the doc comment above is enforced.
    ///
    /// `line.zip(column)` alone is not enough: it distinguishes *absent*
    /// from *present*, and V8 reports a stack overflow as
    /// present-and-**zero**, so `function f(n) { return f(n + 1); } f(0)` —
    /// an ordinary bug in a model-written traversal — was rendered as
    /// `line 0, column 0`. There is no line 0 in anyone's program.
    pub fn position_of(line: Option<u32>, column: Option<u32>) -> Option<(u64, u64)> {
        line.zip(column)
            .filter(|&(line, column)| line != 0 || column != 0)
            .map(|(line, column)| (u64::from(line), u64::from(column)))
    }
}

/// The three figures §6's budget line reports.
pub struct Budget {
    pub turn_cap: u64,
    pub task_used: u64,
    pub task_cap: u64,
    pub cells_used: u64,
    pub cells_cap: u64,
}

/// §6's user message: the yield/throw line, then `## Handles`, `## Error`,
/// `## stdout` and `## Budget`, in that order, each omitted when there is
/// nothing to say — except `## Handles`, which is never omitted and writes
/// `(none)` for an empty table.
pub fn render_result(result: &CellResult) -> String {
    let verb = if result.error.is_some() {
        "threw"
    } else {
        "yielded"
    };
    let mut out = format!("[cell {} {verb} in {} ms]", result.cell, result.elapsed_ms);
    if result.error.is_none()
        && let Some(reason) = &result.yield_reason
    {
        out.push('\n');
        out.push_str(reason);
    }

    out.push_str("\n\n## Handles\n");
    if result.handle_table.is_empty() {
        out.push_str("(none)");
    } else {
        out.push_str(&result.handle_table);
    }

    if let Some(error) = &result.error {
        out.push_str("\n\n## Error\n");
        out.push_str(&format!("{}: {}", error.class, error.message));
        if let Some((line, column)) = error.position {
            out.push_str(&format!("\nline {line}, column {column}"));
        }
        for frame in error.frames.iter().take(3) {
            out.push_str(&format!("\n  at {frame}"));
        }
    }

    if let Some(stdout) = &result.stdout_tail {
        out.push_str("\n\n## stdout\n");
        out.push_str(stdout);
    }

    out.push_str("\n\n## Budget\n");
    out.push_str(&render_budget_line(&result.budget));

    out
}

fn render_budget_line(budget: &Budget) -> String {
    let mut line = format!(
        "turn cap {} · task {}/{} · cells {}/{}",
        thousands(budget.turn_cap),
        thousands(budget.task_used),
        thousands(budget.task_cap),
        thousands(budget.cells_used),
        thousands(budget.cells_cap),
    );
    if budget.task_cap > 0
        && budget.task_used.saturating_mul(100) >= budget.task_cap.saturating_mul(90)
    {
        line.push_str(" — finish or return");
    }
    line
}

/// `n` with a comma every three digits from the right — the budget line's
/// own formatting, `model-contract.md` §6's `8,000` / `3,412` / `400,000`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

/// What one assistant message contained, per §5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// Exactly one fenced block tagged `pane`; its source, unparsed.
    Program(String),
    /// Two or more `pane` blocks in the same message. Neither runs.
    TwoBlocks,
    /// No `pane` block — including a message whose only fenced block is
    /// tagged something else, such as `ts`.
    Prose,
}

/// §5: the fence is three backticks at the start of a line, the info string
/// is the rest of that line trimmed, and a block ends at the next line that
/// is exactly three backticks.
pub fn extract_program(assistant_text: &str) -> Extracted {
    let lines: Vec<&str> = assistant_text.lines().collect();
    let mut programs = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(rest) = lines[i].strip_prefix("```") {
            let info = rest.trim();
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "```" {
                body.push(lines[j]);
                j += 1;
            }
            if info == "pane" {
                programs.push(body.join("\n"));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    match programs.len() {
        0 => Extracted::Prose,
        1 => Extracted::Program(programs.into_iter().next().expect("length checked above")),
        _ => Extracted::TwoBlocks,
    }
}
