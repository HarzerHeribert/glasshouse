//! One cell, from the model's TypeScript to the JavaScript V8 runs —
//! `runtime-contract.md` §1 and §2.
//!
//! **Types are erased in place, never re-printed.** Every TypeScript-only
//! span is overwritten with spaces (newlines kept), so the JavaScript handed
//! to V8 has the model's own program at the model's own line and column. That
//! is what lets `runtime-contract.md` §5's "the source line and column inside
//! the model's own program" be a subtraction rather than a source map, and it
//! is why this module uses `oxc`'s parser and not its transformer: a
//! transformer's output is correct JavaScript at the wrong positions.
//!
//! **The persistent scope is the context's global object, and a cell is a
//! function body.** The model's statements are wrapped in one `async function`
//! so that a top-level `return` is legal (§1) and a redeclaration in a later
//! cell is a fresh function scope rather than a `SyntaxError` (§2).
//!
//! **A binding is captured where it is made, not at the end of the cell.**
//! One `s("name", name)` call is inserted after each top-level declaration —
//! at the end of the line it finishes on, so nothing before it moves — which
//! is what makes `runtime-contract.md` §5's "the bindings made before the
//! throw persist" true without a `finally`. A `finally` could not do it
//! anyway: `const` and `let` are block-scoped, so a `try { const x = 1 }`
//! leaves nothing for `finally` to see.

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    AccessorProperty, BindingPattern, Class, Expression, FormalParameter, Function,
    MethodDefinition, Program, PropertyDefinition, Statement, TSAsExpression, TSEnumDeclaration,
    TSExternalModuleDeclaration, TSGlobalDeclaration, TSImportEqualsDeclaration,
    TSInstantiationExpression, TSInterfaceDeclaration, TSNamespaceDeclaration, TSNonNullExpression,
    TSSatisfiesExpression, TSTypeAliasDeclaration, TSTypeAnnotation, TSTypeAssertion,
    TSTypeParameterDeclaration, TSTypeParameterInstantiation, VariableDeclarator,
};
use oxc::ast_visit::{Visit, walk};
use oxc::parser::{ParseOptions, Parser};
use oxc::span::{GetSpan, SourceType, Span};
use oxc::syntax::scope::ScopeFlags;

/// The name every runtime-owned binding in a generated cell starts with. A
/// model program that declares one is refused rather than silently losing
/// its own bindings — see [`CellError::ReservedName`].
pub const RESERVED_PREFIX: &str = "__pane_";

/// The names the isolate puts on the persistent scope: the four tools and
/// the three handle functions. A top-level binding may not take one, because
/// the capture that makes a handle persist would overwrite it for the whole
/// task.
///
/// `console` is deliberately absent: it is not a capability, shadowing it
/// costs the model only its own logging, and a program that assigns to it is
/// doing something it can undo.
pub const HOST_FUNCTIONS: [&str; 7] = ["read", "glob", "grep", "bash", "keep", "free", "handles"];

/// The one runtime binding a generated cell carries: the host object whose
/// `s` captures a completed binding and whose `e` marks that the body ran
/// off its end. It is a parameter, not a global, so it is gone the moment
/// the cell's function returns.
const HOST: &str = "__pane_cell";

/// How many lines the wrapper puts before the model's first line. The
/// prologue is exactly one line, so a V8 position maps back by subtracting
/// it and columns need no adjustment at all.
pub const LINE_OFFSET: u32 = 1;

/// A cell's source, erased and wrapped, with everything the isolate needs to
/// map V8's answers back onto the model's own program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledCell {
    /// The JavaScript V8 compiles: one parenthesised `async function`.
    pub javascript: String,
    /// Every top-level binding name the cell declares, in source order.
    pub declared: Vec<String>,
    /// The name V8 knows this script by, and the only script name whose
    /// stack frames reach the model (`runtime-contract.md` §5).
    pub script_name: String,
}

/// Why a cell could not be turned into JavaScript.
///
/// Each variant is a *value* the isolate turns into a throw in the model's
/// own turn slot; none of them is an error the runtime reports upwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellError {
    /// The TypeScript did not parse. The position is the model's own.
    Parse {
        message: String,
        line: u32,
        column: u32,
    },
    /// A TypeScript construct that has no JavaScript to erase to — an
    /// `enum`, a `namespace`, an `import =`, or a constructor parameter
    /// property. `tsc` compiles these by *emitting* code; a type eraser
    /// cannot, and pretending otherwise would run a program that means
    /// something else.
    NotErasable {
        construct: &'static str,
        line: u32,
        column: u32,
    },
    /// A top-level binding whose name is the runtime's own.
    ReservedName { name: String },
    /// A top-level binding whose name is one of the host functions. It would
    /// be captured onto the persistent scope and would shadow that function
    /// for **every later cell of the task**, with nothing the model could
    /// write to get it back.
    ShadowsHostFunction { name: String },
}

impl CellError {
    pub fn class(&self) -> &'static str {
        match self {
            CellError::Parse { .. } => "SyntaxError",
            CellError::NotErasable { .. } => "TypeScriptNotErasable",
            CellError::ReservedName { .. } => "ReservedName",
            CellError::ShadowsHostFunction { .. } => "ShadowsHostFunction",
        }
    }

    pub fn message(&self) -> String {
        match self {
            CellError::Parse { message, .. } => message.clone(),
            CellError::NotErasable { construct, .. } => format!(
                "`{construct}` is TypeScript that has no JavaScript to erase to; pane strips \
                 types, it does not compile them"
            ),
            CellError::ReservedName { name } => format!(
                "`{name}` starts with `{RESERVED_PREFIX}`, which the runtime reserves for its own \
                 bindings"
            ),
            CellError::ShadowsHostFunction { name } => format!(
                "`{name}` is a host function; binding it would replace it on the persistent scope \
                 for the rest of the task and nothing could put it back"
            ),
        }
    }

    pub fn position(&self) -> Option<(u32, u32)> {
        match self {
            CellError::Parse { line, column, .. } | CellError::NotErasable { line, column, .. } => {
                Some((*line, *column))
            }
            CellError::ReservedName { .. } | CellError::ShadowsHostFunction { .. } => None,
        }
    }
}

/// Erases `source`'s types, names its top-level bindings, and wraps it in the
/// one function the isolate runs.
pub fn compile(source: &str, cell: u64) -> Result<CompiledCell, CellError> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts())
        .with_options(ParseOptions {
            // A cell's top-level `return` ends the task (§1); it is the one
            // way out of a task and it is not an error here.
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if let Some(first) = parsed.diagnostics.iter().next() {
        let offset = first.labels.first().map_or(0, |label| label.offset());
        let (line, column) = line_and_column(source, offset);
        return Err(CellError::Parse {
            message: first.message.to_string(),
            line,
            column,
        });
    }

    let mut eraser = Eraser {
        source,
        blanks: Vec::new(),
        not_erasable: None,
    };
    eraser.visit_program(&parsed.program);
    if let Some((construct, offset)) = eraser.not_erasable {
        let (line, column) = line_and_column(source, offset);
        return Err(CellError::NotErasable {
            construct,
            line,
            column,
        });
    }
    let blanks = eraser.blanks;

    let captures = top_level_captures(&parsed.program, source);
    let declared: Vec<String> = captures
        .iter()
        .flat_map(|capture| capture.names.iter().cloned())
        .collect();
    if let Some(name) = declared.iter().find(|n| n.starts_with(RESERVED_PREFIX)) {
        return Err(CellError::ReservedName { name: name.clone() });
    }
    if let Some(name) = declared
        .iter()
        .find(|n| HOST_FUNCTIONS.contains(&n.as_str()))
    {
        return Err(CellError::ShadowsHostFunction { name: name.clone() });
    }

    let body = render(source, &blanks, &captures);
    Ok(CompiledCell {
        javascript: wrap(&body),
        declared,
        script_name: script_name(cell),
    })
}

/// The name V8 gives the cell's script, and the only one whose frames are
/// shown to the model.
pub fn script_name(cell: u64) -> String {
    format!("pane:cell:{cell}")
}

/// The generated program. Line 1 is the whole prologue, so the model's line
/// *n* is the script's line *n + [`LINE_OFFSET`]* and every column is
/// unchanged.
fn wrap(body: &str) -> String {
    let mut out = format!("(async function({HOST}){{\n");
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    // A leading `;` so no statement of the model's can continue into the
    // epilogue, and `e()` last so it runs exactly when the body fell off its
    // end rather than returning.
    out.push_str(&format!(";{HOST}.e();\n}})"));
    out
}

/// One top-level statement that binds a name, and where its capture call
/// goes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TopLevelCapture {
    names: Vec<String>,
    /// The byte offset the `s("name", name)` calls are inserted at: the end
    /// of the line the statement finishes on, so a program written one
    /// statement per line has every column exactly where the model put it.
    insert_at: u32,
}

/// Every name a cell's top level binds, in source order, with the point each
/// is captured at.
///
/// `const`, `let`, `var`, `function` and `class` are §2's "top-level
/// binding". A bare top-level assignment to an identifier is included too:
/// it also survives into the next cell, because the wrapper's scope chain
/// ends at the global object, and a value that persists and is not in the
/// table is exactly the untracked object this contract exists to not have.
fn top_level_captures(program: &Program<'_>, source: &str) -> Vec<TopLevelCapture> {
    let bounds: Vec<(u32, u32)> = program
        .body
        .iter()
        .map(|statement| {
            let span = statement.span();
            (span.start, span.end)
        })
        .collect();

    let mut captures: Vec<TopLevelCapture> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (index, statement) in program.body.iter().enumerate() {
        let mut names: Vec<String> = Vec::new();
        match statement {
            Statement::VariableDeclaration(declaration) => {
                for declarator in &declaration.declarations {
                    for ident in binding_names(&declarator.id) {
                        names.push(ident.to_string());
                    }
                }
            }
            Statement::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    names.push(id.name.to_string());
                }
            }
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    names.push(id.name.to_string());
                }
            }
            Statement::ExpressionStatement(expression) => {
                if let Expression::AssignmentExpression(assignment) = &expression.expression
                    && let Some(target) = assignment.left.as_simple_assignment_target()
                    && let Some(ident) = target.get_identifier_name()
                {
                    names.push(ident.to_string());
                }
            }
            _ => continue,
        }
        names.retain(|name| {
            if seen.contains(name) {
                false
            } else {
                seen.push(name.clone());
                true
            }
        });
        if names.is_empty() {
            continue;
        }
        let end = bounds[index].1;
        let next_start = bounds.get(index + 1).map_or(source.len() as u32, |b| b.0);
        captures.push(TopLevelCapture {
            names,
            insert_at: insertion_point(source, end, next_start),
        });
    }
    captures
}

/// The end of the line `end` falls on, unless the next top-level statement
/// starts before it — in which case the capture goes immediately after the
/// statement, which shifts that line's later columns and is the price of two
/// statements sharing a line.
fn insertion_point(source: &str, end: u32, next_start: u32) -> u32 {
    let line_end = source[end as usize..]
        .find('\n')
        .map_or(source.len() as u32, |offset| end + offset as u32);
    line_end.min(next_start).max(end)
}

fn binding_names<'a>(pattern: &BindingPattern<'a>) -> Vec<&'a str> {
    pattern
        .get_binding_identifiers()
        .into_iter()
        .map(|ident| ident.name.as_str())
        .collect()
}

/// Rewrites `source` with every span in `blanks` replaced by spaces, one
/// space per character so a column never moves, `\n`/`\r` kept so a line
/// never moves either, and each capture's `s(...)` calls spliced in at its
/// own offset.
///
/// Both are applied in one pass, over the *original* offsets: blanking a
/// multi-byte character to one space changes byte positions, so a second
/// pass would splice in the wrong place.
fn render(source: &str, blanks: &[Span], captures: &[TopLevelCapture]) -> String {
    let mut ranges: Vec<(u32, u32)> = blanks
        .iter()
        .filter(|span| span.end > span.start)
        .map(|span| (span.start, span.end))
        .collect();
    ranges.sort_unstable();

    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }

    let mut splices: Vec<(u32, String)> = captures
        .iter()
        .map(|capture| {
            let mut text = String::new();
            for name in &capture.names {
                text.push_str(&format!(";{HOST}.s(\"{name}\",{name})"));
            }
            text.push(';');
            (capture.insert_at, text)
        })
        .collect();
    splices.sort_by_key(|(at, _)| *at);

    let mut out = String::with_capacity(source.len());
    let mut next = 0usize;
    let mut spliced = 0usize;
    for (offset, ch) in source.char_indices() {
        while spliced < splices.len() && splices[spliced].0 as usize <= offset {
            out.push_str(&splices[spliced].1);
            spliced += 1;
        }
        while next < merged.len() && (offset as u32) >= merged[next].1 {
            next += 1;
        }
        let blanked = next < merged.len()
            && (offset as u32) >= merged[next].0
            && (offset as u32) < merged[next].1;
        if blanked && ch != '\n' && ch != '\r' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    while spliced < splices.len() {
        out.push_str(&splices[spliced].1);
        spliced += 1;
    }
    out
}

/// The 1-based line and 0-based column of a byte offset, counting columns in
/// characters so they agree with what V8 reports for the same position.
fn line_and_column(source: &str, offset: u32) -> (u32, u32) {
    let offset = offset as usize;
    let mut line = 1u32;
    let mut column = 0u32;
    for (index, ch) in source.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// Collects the spans that are TypeScript and nothing else.
struct Eraser<'s> {
    source: &'s str,
    blanks: Vec<Span>,
    not_erasable: Option<(&'static str, u32)>,
}

impl<'s> Eraser<'s> {
    fn blank(&mut self, span: Span) {
        self.blanks.push(span);
    }

    /// Blanks the tail of `whole` that follows `inner` — the ` as Foo`, the
    /// `!`, the `<T>` of an instantiation.
    fn blank_tail(&mut self, whole: Span, inner: Span) {
        if whole.end > inner.end {
            self.blank(Span::new(inner.end, whole.end));
        }
    }

    /// Blanks a single-character marker (`?` or `!`) that follows `after`.
    fn blank_marker(&mut self, after: u32, marker: char) {
        let rest = &self.source[after as usize..];
        if let Some(index) = rest.find(marker) {
            let at = after + index as u32;
            // Only when nothing but whitespace separates the two: a `?` any
            // further away belongs to something else.
            if rest[..index].chars().all(char::is_whitespace) {
                self.blank(Span::new(at, at + 1));
            }
        }
    }

    /// Blanks each of `keywords` found in `[from, to)`.
    fn blank_keywords(&mut self, from: u32, to: u32, keywords: &[&str]) {
        if to <= from || to as usize > self.source.len() {
            return;
        }
        let window = &self.source[from as usize..to as usize];
        for keyword in keywords {
            let mut search = 0usize;
            while let Some(index) = window[search..].find(keyword) {
                let at = search + index;
                let before_ok = at == 0
                    || !window[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$');
                let after = at + keyword.len();
                let after_ok = window[after..]
                    .chars()
                    .next()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '$'));
                if before_ok && after_ok {
                    self.blank(Span::new(from + at as u32, from + after as u32));
                }
                search = after;
            }
        }
    }

    fn refuse(&mut self, construct: &'static str, span: Span) {
        if self.not_erasable.is_none() {
            self.not_erasable = Some((construct, span.start));
        }
    }
}

/// The modifiers that are TypeScript-only on a class member. `static` and
/// `accessor` are deliberately absent: both are JavaScript.
const CLASS_MEMBER_MODIFIERS: [&str; 6] = [
    "public",
    "private",
    "protected",
    "readonly",
    "abstract",
    "override",
];

impl<'a> Visit<'a> for Eraser<'_> {
    fn visit_ts_type_annotation(&mut self, it: &TSTypeAnnotation<'a>) {
        // The span starts at the `:`, so the whole annotation goes.
        self.blank(it.span);
    }

    fn visit_ts_type_parameter_declaration(&mut self, it: &TSTypeParameterDeclaration<'a>) {
        self.blank(it.span);
    }

    fn visit_ts_type_parameter_instantiation(&mut self, it: &TSTypeParameterInstantiation<'a>) {
        self.blank(it.span);
    }

    fn visit_ts_type_alias_declaration(&mut self, it: &TSTypeAliasDeclaration<'a>) {
        self.blank(it.span);
    }

    fn visit_ts_interface_declaration(&mut self, it: &TSInterfaceDeclaration<'a>) {
        self.blank(it.span);
    }

    fn visit_ts_as_expression(&mut self, it: &TSAsExpression<'a>) {
        self.blank_tail(it.span, it.expression.span());
        self.visit_expression(&it.expression);
    }

    fn visit_ts_satisfies_expression(&mut self, it: &TSSatisfiesExpression<'a>) {
        self.blank_tail(it.span, it.expression.span());
        self.visit_expression(&it.expression);
    }

    fn visit_ts_non_null_expression(&mut self, it: &TSNonNullExpression<'a>) {
        self.blank_tail(it.span, it.expression.span());
        self.visit_expression(&it.expression);
    }

    fn visit_ts_instantiation_expression(&mut self, it: &TSInstantiationExpression<'a>) {
        self.blank_tail(it.span, it.expression.span());
        self.visit_expression(&it.expression);
    }

    fn visit_ts_type_assertion(&mut self, it: &TSTypeAssertion<'a>) {
        // `<T>expr`: the assertion is everything before the expression.
        if it.expression.span().start > it.span.start {
            self.blank(Span::new(it.span.start, it.expression.span().start));
        }
        self.visit_expression(&it.expression);
    }

    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if it.definite {
            self.blank_marker(it.id.span().end, '!');
        }
        walk::walk_variable_declarator(self, it);
    }

    fn visit_formal_parameter(&mut self, it: &FormalParameter<'a>) {
        if it.accessibility.is_some() || it.readonly || it.r#override {
            // `constructor(private x: T)` declares a field as a side effect
            // of the parameter list; erasing the modifier would silently
            // drop the assignment.
            self.refuse("parameter property", it.span);
            return;
        }
        if it.optional {
            self.blank_marker(it.pattern.span().end, '?');
        }
        walk::walk_formal_parameter(self, it);
    }

    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if it.body.is_none() {
            // An overload signature: declaration only, no code.
            self.blank(it.span);
            return;
        }
        walk::walk_function(self, it, flags);
    }

    fn visit_class(&mut self, it: &Class<'a>) {
        if it.r#abstract {
            self.blank_keywords(it.span.start, it.body.span.start, &["abstract"]);
        }
        if let (Some(first), Some(last)) = (it.implements.first(), it.implements.last()) {
            let keyword = self.source[..first.span.start as usize].rfind("implements");
            let from = keyword.map_or(first.span.start, |at| at as u32);
            self.blank(Span::new(from, last.span.end));
        }
        walk::walk_class(self, it);
    }

    fn visit_property_definition(&mut self, it: &PropertyDefinition<'a>) {
        if it.declare {
            self.blank(it.span);
            return;
        }
        self.blank_keywords(it.span.start, it.key.span().start, &CLASS_MEMBER_MODIFIERS);
        if it.optional {
            self.blank_marker(it.key.span().end, '?');
        }
        if it.definite {
            self.blank_marker(it.key.span().end, '!');
        }
        walk::walk_property_definition(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'a>) {
        self.blank_keywords(it.span.start, it.key.span().start, &CLASS_MEMBER_MODIFIERS);
        if it.optional {
            self.blank_marker(it.key.span().end, '?');
        }
        walk::walk_method_definition(self, it);
    }

    fn visit_accessor_property(&mut self, it: &AccessorProperty<'a>) {
        self.blank_keywords(it.span.start, it.key.span().start, &CLASS_MEMBER_MODIFIERS);
        walk::walk_accessor_property(self, it);
    }

    fn visit_ts_enum_declaration(&mut self, it: &TSEnumDeclaration<'a>) {
        self.refuse("enum", it.span);
    }

    fn visit_ts_namespace_declaration(&mut self, it: &TSNamespaceDeclaration<'a>) {
        self.refuse("namespace", it.span);
    }

    fn visit_ts_global_declaration(&mut self, it: &TSGlobalDeclaration<'a>) {
        self.refuse("declare global", it.span);
    }

    fn visit_ts_external_module_declaration(&mut self, it: &TSExternalModuleDeclaration<'a>) {
        self.refuse("declare module", it.span);
    }

    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) {
        self.refuse("import =", it.span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model's own lines back out of the generated program: the
    /// prologue dropped, the trailing `e()` dropped, and each line's capture
    /// splice — which is always appended at the end — cut off again.
    fn body_of(javascript: &str) -> String {
        javascript
            .lines()
            .skip(1)
            .take_while(|line| *line != ";__pane_cell.e();")
            .map(|line| line.split(";__pane_cell.s(").next().unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_type_annotation_is_erased_where_it_stood() {
        let source = "const n: number = 1;\nconst s: string = \"x\";\n";
        let compiled = compile(source, 1).unwrap();
        let body = body_of(&compiled.javascript);
        assert_eq!(
            body, "const n         = 1;\nconst s         = \"x\";",
            "{body:?}"
        );
        assert_eq!(compiled.declared, vec!["n", "s"]);
    }

    #[test]
    fn every_erasure_keeps_the_line_and_the_column() {
        let source = "const a = 1;\nfunction f(x: number, y?: string): void {\n  return;\n}\nconst b = f as unknown;\n";
        let compiled = compile(source, 3).unwrap();
        for (original, erased) in source.lines().zip(body_of(&compiled.javascript).lines()) {
            assert_eq!(
                original.chars().count(),
                erased.chars().count(),
                "a line changed width: {original:?} -> {erased:?}"
            );
        }
    }

    #[test]
    fn a_destructuring_declaration_names_every_binding_it_makes() {
        let compiled = compile("const { a, b: c } = obj;\nconst [d, ...e] = arr;\n", 1).unwrap();
        assert_eq!(compiled.declared, vec!["a", "c", "d", "e"]);
    }

    #[test]
    fn functions_classes_and_bare_assignments_are_top_level_bindings() {
        let compiled = compile("function f() {}\nclass K {}\nlet v = 1;\nw = 2;\n", 1).unwrap();
        assert_eq!(compiled.declared, vec!["f", "K", "v", "w"]);
    }

    #[test]
    fn an_interface_and_a_type_alias_leave_nothing_behind() {
        let compiled = compile(
            "interface I { a: number }\ntype T = I | null;\nconst x = 1;\n",
            1,
        )
        .unwrap();
        let body = body_of(&compiled.javascript);
        assert!(!body.contains("interface"), "{body:?}");
        assert!(!body.contains("type T"), "{body:?}");
        assert_eq!(compiled.declared, vec!["x"]);
    }

    #[test]
    fn generics_and_assertions_are_erased() {
        let compiled = compile(
            "const m = new Map<string, number>();\nconst y = (m as any)!;\n",
            1,
        )
        .unwrap();
        let body = body_of(&compiled.javascript);
        assert!(body.contains("new Map"), "{body:?}");
        assert!(!body.contains("<string, number>"), "{body:?}");
        assert!(!body.contains(" as any"), "{body:?}");
        assert!(!body.contains("!;"), "{body:?}");
    }

    #[test]
    fn an_enum_is_refused_rather_than_mis_erased() {
        let error = compile("enum E { A, B }\n", 1).unwrap_err();
        assert!(matches!(
            error,
            CellError::NotErasable {
                construct: "enum",
                line: 1,
                ..
            }
        ));
    }

    #[test]
    fn a_namespace_and_a_parameter_property_are_refused() {
        assert!(matches!(
            compile("namespace N { export const x = 1; }\n", 1).unwrap_err(),
            CellError::NotErasable {
                construct: "namespace",
                ..
            }
        ));
        assert!(matches!(
            compile("class K { constructor(private x: number) {} }\n", 1).unwrap_err(),
            CellError::NotErasable {
                construct: "parameter property",
                ..
            }
        ));
    }

    #[test]
    fn a_binding_may_not_take_a_host_functions_name() {
        let error = compile("const read = 1;\n", 1).unwrap_err();
        assert!(
            matches!(error, CellError::ShadowsHostFunction { ref name } if name == "read"),
            "{error:?}"
        );
        // Every declared host function is guarded, and `console` is not.
        for name in HOST_FUNCTIONS {
            assert!(
                compile(&format!("const {name} = 1;\n"), 1).is_err(),
                "`{name}` may be shadowed"
            );
        }
        assert!(compile("const console = 1;\n", 1).is_ok());
    }

    #[test]
    fn a_runtime_reserved_name_is_refused() {
        let error = compile("const __pane_cell = 1;\n", 1).unwrap_err();
        assert!(matches!(error, CellError::ReservedName { .. }), "{error:?}");
    }

    #[test]
    fn a_parse_failure_reports_the_models_own_position() {
        let error = compile("const a = 1;\nconst = ;\n", 1).unwrap_err();
        match error {
            CellError::Parse { line, .. } => assert_eq!(line, 2),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn the_prologue_is_exactly_one_line() {
        let compiled = compile("const a = 1;\n", 1).unwrap();
        let first = compiled.javascript.lines().next().unwrap();
        assert_eq!(first, "(async function(__pane_cell){");
        assert_eq!(LINE_OFFSET, 1);
        assert!(
            compiled
                .javascript
                .lines()
                .nth(1)
                .unwrap()
                .starts_with("const a = 1;"),
            "{}",
            compiled.javascript
        );
    }

    /// The capture goes at the end of the line the declaration finishes on,
    /// so every column before it is the model's own.
    #[test]
    fn a_capture_is_appended_where_the_declaration_ends() {
        let compiled = compile("const a = 1;\nlet b = 2;\n", 1).unwrap();
        let lines: Vec<&str> = compiled.javascript.lines().collect();
        assert_eq!(lines[1], "const a = 1;;__pane_cell.s(\"a\",a);");
        assert_eq!(lines[2], "let b = 2;;__pane_cell.s(\"b\",b);");
        assert_eq!(compiled.declared, vec!["a", "b"]);
    }

    /// A binding made before a throw has already been captured when the
    /// throw happens — `runtime-contract.md` §5's third item, as a property
    /// of where the call sits rather than of a `finally`.
    #[test]
    fn a_capture_precedes_a_later_statement_that_could_throw() {
        let compiled = compile("const before = 1;\nthrow new Error(\"x\");\n", 1).unwrap();
        let capture = compiled.javascript.find("s(\"before\"").unwrap();
        let throw = compiled.javascript.find("throw new Error").unwrap();
        assert!(capture < throw, "{}", compiled.javascript);
    }
}
