//! A bounded observer for the top-level `model` string in a relayed request.
//!
//! [`Observed`] validates JSON incrementally from the same borrowed bytes its
//! caller sends upstream and returns those bytes untouched. It retains only a
//! bounded top-level key or model literal; unrelated strings are validated and
//! immediately discarded.

use std::io::Read;
use std::sync::{Arc, Mutex};

pub(super) const MAX_MODEL_BYTES: usize = 256;
const MAX_CAPTURE_BYTES: usize = MAX_MODEL_BYTES * 6 + 2;
const MAX_DEPTH: usize = 64;

pub(super) fn bounded(model: String) -> Option<String> {
    (model.len() <= MAX_MODEL_BYTES).then_some(model)
}

pub(super) fn observe<R>(inner: R, expected: u64) -> (Observed<R>, Observation) {
    let scanner = Arc::new(Mutex::new(Scanner::default()));
    (
        Observed {
            inner,
            scanner: Arc::clone(&scanner),
        },
        Observation { scanner, expected },
    )
}

pub(super) struct Observed<R> {
    inner: R,
    scanner: Arc<Mutex<Scanner>>,
}

impl<R: Read> Read for Observed<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(bytes)?;
        self.scanner
            .lock()
            .expect("request model observer poisoned")
            .feed(&bytes[..read]);
        Ok(read)
    }
}

pub(super) struct Observation {
    scanner: Arc<Mutex<Scanner>>,
    expected: u64,
}

impl Observation {
    pub(super) fn model(&self) -> Option<String> {
        self.scanner
            .lock()
            .expect("request model observer poisoned")
            .model(self.expected)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectPhase {
    KeyOrEnd { allow_end: bool },
    Colon,
    Value,
    CommaOrEnd,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArrayPhase {
    ValueOrEnd { allow_end: bool },
    CommaOrEnd,
}

enum Context {
    Object {
        phase: ObjectPhase,
        root: bool,
        current_is_model: bool,
    },
    Array {
        phase: ArrayPhase,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringRole {
    TopKey,
    OtherKey,
    ModelValue,
    OtherValue,
}

#[derive(Clone, Copy)]
enum NumberState {
    Minus,
    Zero,
    Integer,
    Dot,
    Fraction,
    Exponent,
    ExponentSign,
    ExponentDigits,
}

impl NumberState {
    fn accepting(self) -> bool {
        matches!(
            self,
            Self::Zero | Self::Integer | Self::Fraction | Self::ExponentDigits
        )
    }
}

struct LiteralState {
    expected: &'static [u8],
    matched: usize,
}

#[derive(Default)]
struct Scanner {
    stack: Vec<Context>,
    started: bool,
    done: bool,
    string: Option<StringRole>,
    string_bytes: Vec<u8>,
    capture_overflow: bool,
    escaped: bool,
    unicode_left: u8,
    utf8_left: u8,
    utf8_min: u8,
    utf8_max: u8,
    number: Option<NumberState>,
    literal: Option<LiteralState>,
    model_keys: u8,
    candidate: Option<String>,
    consumed: u64,
    ambiguous: bool,
    invalid: bool,
}

impl Scanner {
    fn feed(&mut self, bytes: &[u8]) {
        self.consumed = self.consumed.saturating_add(bytes.len() as u64);
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn model(&self, expected: u64) -> Option<String> {
        (!self.invalid
            && !self.ambiguous
            && self.done
            && self.stack.is_empty()
            && self.string.is_none()
            && self.number.is_none()
            && self.literal.is_none()
            && self.consumed == expected
            && self.model_keys == 1)
            .then(|| self.candidate.clone())
            .flatten()
    }

    fn byte(&mut self, byte: u8) {
        if self.invalid {
            return;
        }
        if self.string.is_some() {
            self.string_byte(byte);
            return;
        }
        if self.number.is_some() {
            self.number_byte(byte);
            return;
        }
        if self.literal.is_some() {
            self.literal_byte(byte);
            return;
        }
        if is_json_whitespace(byte) {
            return;
        }
        if self.done {
            self.invalid = true;
            return;
        }
        if !self.started {
            if byte == b'{' {
                self.started = true;
                self.push_object(true);
            } else {
                self.invalid = true;
            }
            return;
        }

        enum Action {
            Key(bool),
            Colon,
            Value(bool),
            ObjectCommaOrEnd,
            ArrayValue(bool),
            ArrayCommaOrEnd,
        }
        let action = match self.stack.last() {
            Some(Context::Object {
                phase: ObjectPhase::KeyOrEnd { allow_end },
                ..
            }) => Action::Key(*allow_end),
            Some(Context::Object {
                phase: ObjectPhase::Colon,
                ..
            }) => Action::Colon,
            Some(Context::Object {
                phase: ObjectPhase::Value,
                root,
                current_is_model,
            }) => Action::Value(*root && *current_is_model),
            Some(Context::Object {
                phase: ObjectPhase::CommaOrEnd,
                ..
            }) => Action::ObjectCommaOrEnd,
            Some(Context::Array {
                phase: ArrayPhase::ValueOrEnd { allow_end },
            }) => Action::ArrayValue(*allow_end),
            Some(Context::Array {
                phase: ArrayPhase::CommaOrEnd,
            }) => Action::ArrayCommaOrEnd,
            None => {
                self.invalid = true;
                return;
            }
        };

        match action {
            Action::Key(allow_end) if byte == b'}' && allow_end => self.close_object(),
            Action::Key(_) if byte == b'"' => {
                let root = matches!(self.stack.last(), Some(Context::Object { root: true, .. }));
                self.start_string(if root {
                    StringRole::TopKey
                } else {
                    StringRole::OtherKey
                });
            }
            Action::Colon if byte == b':' => {
                if let Some(Context::Object { phase, .. }) = self.stack.last_mut() {
                    *phase = ObjectPhase::Value;
                }
            }
            Action::Value(capture_model) => self.start_value(byte, capture_model),
            Action::ObjectCommaOrEnd if byte == b',' => {
                if let Some(Context::Object { phase, .. }) = self.stack.last_mut() {
                    *phase = ObjectPhase::KeyOrEnd { allow_end: false };
                }
            }
            Action::ObjectCommaOrEnd if byte == b'}' => self.close_object(),
            Action::ArrayValue(allow_end) if byte == b']' && allow_end => self.close_array(),
            Action::ArrayValue(_) => self.start_value(byte, false),
            Action::ArrayCommaOrEnd if byte == b',' => {
                if let Some(Context::Array { phase }) = self.stack.last_mut() {
                    *phase = ArrayPhase::ValueOrEnd { allow_end: false };
                }
            }
            Action::ArrayCommaOrEnd if byte == b']' => self.close_array(),
            _ => self.invalid = true,
        }
    }

    fn start_value(&mut self, byte: u8, capture_model: bool) {
        if capture_model && byte != b'"' {
            self.candidate = None;
        }
        match byte {
            b'"' => self.start_string(if capture_model {
                StringRole::ModelValue
            } else {
                StringRole::OtherValue
            }),
            b'{' => self.push_object(false),
            b'[' => self.push_array(),
            b't' => self.literal = Some(LiteralState::new(b"true")),
            b'f' => self.literal = Some(LiteralState::new(b"false")),
            b'n' => self.literal = Some(LiteralState::new(b"null")),
            b'-' => self.number = Some(NumberState::Minus),
            b'0' => self.number = Some(NumberState::Zero),
            b'1'..=b'9' => self.number = Some(NumberState::Integer),
            _ => self.invalid = true,
        }
    }

    fn push_object(&mut self, root: bool) {
        if self.stack.len() >= MAX_DEPTH {
            self.invalid = true;
            return;
        }
        self.stack.push(Context::Object {
            phase: ObjectPhase::KeyOrEnd { allow_end: true },
            root,
            current_is_model: false,
        });
    }

    fn push_array(&mut self) {
        if self.stack.len() >= MAX_DEPTH {
            self.invalid = true;
            return;
        }
        self.stack.push(Context::Array {
            phase: ArrayPhase::ValueOrEnd { allow_end: true },
        });
    }

    fn close_object(&mut self) {
        if !matches!(self.stack.pop(), Some(Context::Object { .. })) {
            self.invalid = true;
            return;
        }
        self.container_complete();
    }

    fn close_array(&mut self) {
        if !matches!(self.stack.pop(), Some(Context::Array { .. })) {
            self.invalid = true;
            return;
        }
        self.container_complete();
    }

    fn container_complete(&mut self) {
        if self.stack.is_empty() {
            self.done = true;
        } else {
            self.value_complete();
        }
    }

    fn value_complete(&mut self) {
        match self.stack.last_mut() {
            Some(Context::Object { phase, .. }) if *phase == ObjectPhase::Value => {
                *phase = ObjectPhase::CommaOrEnd;
            }
            Some(Context::Array { phase }) if matches!(phase, ArrayPhase::ValueOrEnd { .. }) => {
                *phase = ArrayPhase::CommaOrEnd;
            }
            _ => self.invalid = true,
        }
    }

    fn start_string(&mut self, role: StringRole) {
        self.string = Some(role);
        self.string_bytes.clear();
        self.capture_overflow = false;
        if matches!(role, StringRole::TopKey | StringRole::ModelValue) {
            self.string_bytes.push(b'"');
        }
        self.escaped = false;
        self.unicode_left = 0;
        self.utf8_left = 0;
    }

    fn string_byte(&mut self, byte: u8) {
        let capture = matches!(
            self.string,
            Some(StringRole::TopKey | StringRole::ModelValue)
        );
        if capture {
            if self.string_bytes.len() < MAX_CAPTURE_BYTES {
                self.string_bytes.push(byte);
            } else {
                self.capture_overflow = true;
            }
        }
        if self.unicode_left > 0 {
            if byte.is_ascii_hexdigit() {
                self.unicode_left -= 1;
            } else {
                self.invalid = true;
            }
            return;
        }
        if self.escaped {
            self.escaped = false;
            if byte == b'u' {
                self.unicode_left = 4;
            } else if !matches!(byte, b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') {
                self.invalid = true;
            }
            return;
        }
        if self.utf8_left > 0 {
            if !(self.utf8_min..=self.utf8_max).contains(&byte) {
                self.invalid = true;
            } else {
                self.utf8_left -= 1;
                self.utf8_min = 0x80;
                self.utf8_max = 0xbf;
            }
            return;
        }
        match byte {
            b'\\' => self.escaped = true,
            b'"' => self.end_string(),
            0..=0x1f | 0x80..=0xc1 | 0xf5..=0xff => self.invalid = true,
            0xc2..=0xdf => self.start_utf8(1, 0x80, 0xbf),
            0xe0 => self.start_utf8(2, 0xa0, 0xbf),
            0xe1..=0xec | 0xee..=0xef => self.start_utf8(2, 0x80, 0xbf),
            0xed => self.start_utf8(2, 0x80, 0x9f),
            0xf0 => self.start_utf8(3, 0x90, 0xbf),
            0xf1..=0xf3 => self.start_utf8(3, 0x80, 0xbf),
            0xf4 => self.start_utf8(3, 0x80, 0x8f),
            _ => {}
        }
    }

    fn start_utf8(&mut self, left: u8, min: u8, max: u8) {
        self.utf8_left = left;
        self.utf8_min = min;
        self.utf8_max = max;
    }

    fn end_string(&mut self) {
        let role = self.string.take().expect("a string is open");
        let decoded = (!self.capture_overflow)
            .then(|| decode_json_string(&self.string_bytes))
            .flatten();
        match role {
            StringRole::TopKey => {
                let Some(key) = decoded else {
                    self.ambiguous = true;
                    self.set_object_colon(false);
                    return;
                };
                let is_model = key == "model";
                if is_model {
                    self.model_keys = self.model_keys.saturating_add(1);
                    if self.model_keys > 1 {
                        self.candidate = None;
                    }
                }
                self.set_object_colon(is_model);
            }
            StringRole::OtherKey => self.set_object_colon(false),
            StringRole::ModelValue => {
                self.candidate = decoded.and_then(bounded);
                self.value_complete();
            }
            StringRole::OtherValue => self.value_complete(),
        }
    }

    fn set_object_colon(&mut self, is_model: bool) {
        match self.stack.last_mut() {
            Some(Context::Object {
                phase,
                current_is_model,
                ..
            }) => {
                *phase = ObjectPhase::Colon;
                *current_is_model = is_model;
            }
            _ => self.invalid = true,
        }
    }

    fn number_byte(&mut self, byte: u8) {
        let state = self.number.expect("a number is open");
        let next = match (state, byte) {
            (NumberState::Minus, b'0') => Some(NumberState::Zero),
            (NumberState::Minus, b'1'..=b'9') => Some(NumberState::Integer),
            (NumberState::Integer, b'0'..=b'9') => Some(NumberState::Integer),
            (NumberState::Zero | NumberState::Integer, b'.') => Some(NumberState::Dot),
            (NumberState::Dot | NumberState::Fraction, b'0'..=b'9') => Some(NumberState::Fraction),
            (NumberState::Zero | NumberState::Integer | NumberState::Fraction, b'e' | b'E') => {
                Some(NumberState::Exponent)
            }
            (NumberState::Exponent, b'+' | b'-') => Some(NumberState::ExponentSign),
            (NumberState::Exponent | NumberState::ExponentSign, b'0'..=b'9') => {
                Some(NumberState::ExponentDigits)
            }
            (NumberState::ExponentDigits, b'0'..=b'9') => Some(NumberState::ExponentDigits),
            _ => None,
        };
        if let Some(next) = next {
            self.number = Some(next);
        } else if state.accepting() && is_delimiter(byte) {
            self.number = None;
            self.value_complete();
            self.byte(byte);
        } else {
            self.invalid = true;
        }
    }

    fn literal_byte(&mut self, byte: u8) {
        let literal = self.literal.as_mut().expect("a literal is open");
        if literal.matched < literal.expected.len() {
            if literal.expected[literal.matched] == byte {
                literal.matched += 1;
            } else {
                self.invalid = true;
            }
        } else if is_delimiter(byte) {
            self.literal = None;
            self.value_complete();
            self.byte(byte);
        } else {
            self.invalid = true;
        }
    }
}

impl LiteralState {
    fn new(expected: &'static [u8]) -> Self {
        Self {
            expected,
            matched: 1,
        }
    }
}

fn is_delimiter(byte: u8) -> bool {
    is_json_whitespace(byte) || matches!(byte, b',' | b'}' | b']')
}

fn is_json_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn decode_json_string(literal: &[u8]) -> Option<String> {
    if literal.first() != Some(&b'"') || literal.last() != Some(&b'"') {
        return None;
    }
    let mut decoded = Vec::with_capacity(literal.len().saturating_sub(2));
    let mut index = 1;
    while index + 1 < literal.len() {
        let byte = literal[index];
        index += 1;
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let escaped = *literal.get(index)?;
        index += 1;
        match escaped {
            b'"' | b'\\' | b'/' => decoded.push(escaped),
            b'b' => decoded.push(0x08),
            b'f' => decoded.push(0x0c),
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'u' => {
                let first = decode_hex_quad(literal, &mut index)?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if literal.get(index..index + 2)? != b"\\u" {
                        return None;
                    }
                    index += 2;
                    let second = decode_hex_quad(literal, &mut index)?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return None;
                    }
                    0x1_0000 + (((first - 0xd800) as u32) << 10) + (second - 0xdc00) as u32
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return None;
                } else {
                    first as u32
                };
                let character = char::from_u32(scalar)?;
                let mut encoded = [0; 4];
                decoded.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            }
            _ => return None,
        }
    }
    String::from_utf8(decoded).ok()
}

fn decode_hex_quad(literal: &[u8], index: &mut usize) -> Option<u16> {
    let digits = literal.get(*index..*index + 4)?;
    *index += 4;
    digits.iter().try_fold(0u16, |value, digit| {
        Some((value << 4) | (*digit as char).to_digit(16)? as u16)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OneByte {
        bytes: std::io::Cursor<Vec<u8>>,
        reads: usize,
    }

    impl Read for OneByte {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            let limit = out.len().min(1);
            self.bytes.read(&mut out[..limit])
        }
    }

    struct ErrorAfterPrefix(std::io::Cursor<Vec<u8>>);

    impl Read for ErrorAfterPrefix {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.0.position() < self.0.get_ref().len() as u64 {
                self.0.read(out)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "planted transport failure",
                ))
            }
        }
    }

    fn observed(bytes: &[u8]) -> (Vec<u8>, Option<String>) {
        let (mut reader, observation) =
            observe(std::io::Cursor::new(bytes.to_vec()), bytes.len() as u64);
        let mut relayed = Vec::new();
        reader.read_to_end(&mut relayed).unwrap();
        (relayed, observation.model())
    }

    #[test]
    fn one_byte_reads_observe_a_fragmented_model_without_changing_a_byte() {
        let bytes = br#"{"model":"gpt-\u0035.6-luna","x":[true,null,-1.2e+3]}"#.to_vec();
        let source = OneByte {
            bytes: std::io::Cursor::new(bytes.clone()),
            reads: 0,
        };
        let (mut reader, observation) = observe(source, bytes.len() as u64);
        let mut relayed = Vec::new();
        reader.read_to_end(&mut relayed).unwrap();
        assert_eq!(relayed, bytes);
        assert_eq!(observation.model().as_deref(), Some("gpt-5.6-luna"));
        assert!(reader.inner.reads > relayed.len());
    }

    #[test]
    fn malformed_nested_primitive_and_trailing_comma_bodies_invent_nothing() {
        let cases: &[&[u8]] = &[
            br#"{"model":"luna","junk":xxx}"#,
            br#"{"model":"luna",}"#,
            br#"{"model":"luna","junk":[1,]}"#,
            br#"{"model":"luna","junk":{"x":true,}}"#,
            br#"{"model":"luna","junk":01}"#,
            br#"{"model":"luna","junk":1e}"#,
            br#"{"model":"luna"} trailing"#,
            b"{\"model\":\"luna\"}\x0b",
            b"{\"model\":\"luna\"}\x0c",
        ];
        for bytes in cases {
            let (relayed, model) = observed(bytes);
            assert_eq!(&relayed, bytes);
            assert_eq!(model, None, "body: {}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn incomplete_declared_body_never_claims_the_valid_prefix_model() {
        let prefix = br#"{"model":"luna"}"#;
        let expected = prefix.len() as u64 + 8;

        let (mut short, observation) = observe(std::io::Cursor::new(prefix), expected);
        let mut relayed = Vec::new();
        short.read_to_end(&mut relayed).unwrap();
        assert_eq!(relayed, prefix);
        assert_eq!(observation.model(), None);

        let (mut failed, observation) = observe(
            ErrorAfterPrefix(std::io::Cursor::new(prefix.to_vec())),
            expected,
        );
        let mut relayed = Vec::new();
        assert_eq!(
            failed.read_to_end(&mut relayed).unwrap_err().kind(),
            std::io::ErrorKind::ConnectionReset
        );
        assert_eq!(relayed, prefix);
        assert_eq!(observation.model(), None);
    }

    #[test]
    fn fully_escaped_models_use_the_decoded_256_byte_bound() {
        let accepted = format!(r#"{{"model":"{}"}}"#, "\\u0061".repeat(MAX_MODEL_BYTES));
        let rejected = format!(r#"{{"model":"{}"}}"#, "\\u0061".repeat(MAX_MODEL_BYTES + 1));
        let (_, model) = observed(accepted.as_bytes());
        assert_eq!(model.as_deref(), Some("a".repeat(MAX_MODEL_BYTES).as_str()));
        let (_, model) = observed(rejected.as_bytes());
        assert_eq!(model, None);
    }

    #[test]
    fn deterministic_mutations_never_claim_a_model_for_json_the_reference_parser_rejects() {
        let valid = [
            br#"{"model":"luna","x":[true,null,-1.2e+3]}"#.as_slice(),
            br#"{"ignored":"s\u0065cret","model":"l\u0075na","nested":{"a":[1,2,3]}}"#.as_slice(),
        ];
        let replacements = [b'{', b'}', b'[', b']', b',', b':', 0x0b, 0x0c];
        let mut rejected = 0;
        for body in valid {
            for index in 0..body.len() {
                let mut deleted = body.to_vec();
                deleted.remove(index);
                if serde_json::from_slice::<serde_json::Value>(&deleted).is_err() {
                    let (relayed, model) = observed(&deleted);
                    assert_eq!(relayed, deleted);
                    assert_eq!(model, None);
                    rejected += 1;
                }
                for replacement in replacements {
                    let mut replaced = body.to_vec();
                    replaced[index] = replacement;
                    if serde_json::from_slice::<serde_json::Value>(&replaced).is_err() {
                        let (relayed, model) = observed(&replaced);
                        assert_eq!(relayed, replaced);
                        assert_eq!(model, None);
                        rejected += 1;
                    }
                }
            }
        }
        assert!(
            rejected > 500,
            "mutation corpus did not exercise enough invalid JSON"
        );
    }

    #[test]
    fn nested_duplicate_oversized_and_malformed_models_invent_nothing() {
        let cases = [
            (br#"{"outer":{"model":"nested"}}"#.to_vec(), None),
            (br#"{"model":"one","model":"two"}"#.to_vec(), None),
            (
                format!(r#"{{"model":"{}"}}"#, "x".repeat(MAX_MODEL_BYTES + 1)).into_bytes(),
                None,
            ),
            (br#"{"model":12}"#.to_vec(), None),
            (br#"{"model":"bad\q"}"#.to_vec(), None),
            (br#"{"model":"unfinished}"#.to_vec(), None),
        ];
        for (bytes, expected) in cases {
            let (relayed, model) = observed(&bytes);
            assert_eq!(relayed, bytes);
            assert_eq!(model.as_deref(), expected);
        }
    }

    #[test]
    fn unrelated_strings_are_never_retained_and_large_valid_values_do_not_hide_the_model() {
        let mut scanner = Scanner::default();
        scanner.feed(br#"{"ignored":""#);
        scanner.feed(&vec![b's'; 4096]);
        assert_eq!(scanner.string, Some(StringRole::OtherValue));
        assert!(scanner.string_bytes.is_empty());
        scanner.feed(br#"","tail":[[],[],{}],"model":"luna"}"#);
        assert_eq!(scanner.model(scanner.consumed).as_deref(), Some("luna"));
        assert!(scanner.string_bytes.len() <= MAX_CAPTURE_BYTES);
    }
}
