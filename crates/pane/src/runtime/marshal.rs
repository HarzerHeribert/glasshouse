//! From a live V8 value to the sample the model is shown — the one crossing
//! `runtime-contract.md`'s whole claim depends on.
//!
//! **Nothing here reads a payload.** An array's length is `Array::length`
//! and only four of its elements are touched; a string's length is
//! JavaScript's own `.length` and only its first
//! [`preview::STRING_HEAD_SAMPLE`] characters are copied; an object's values
//! are read for their *type* and only for the twelve keys §3 names. The cost
//! of marshalling a 122 KB grep result is therefore the cost of the four
//! lines the model sees, not of the 122 KB.
//!
//! Depth stops at one, because §3's array row renders its elements "at depth
//! 1": a nested array or object shows its shape and never its own contents,
//! so [`marshal`] never recurses past a container's immediate children.

use std::mem::MaybeUninit;

use crate::runtime::preview::{
    ARRAY_HEAD_SAMPLE, ArrayValue, ErrorValue, OBJECT_KEY_SAMPLE, ObjectValue, STRING_HEAD_SAMPLE,
    StringValue, Value,
};

/// The deepest a marshal goes: 0 is the handle itself, 1 is an element or a
/// property that only ever renders as a shape.
const MAX_DEPTH: u32 = 1;

/// Samples one live value.
pub(crate) fn marshal(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> Value {
    marshal_at(scope, value, 0)
}

fn marshal_at(scope: &mut v8::PinScope, value: v8::Local<v8::Value>, depth: u32) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    if value.is_undefined() {
        return Value::Undefined;
    }
    if value.is_boolean() {
        return Value::Boolean(value.boolean_value(scope));
    }
    if value.is_number() {
        return Value::Number(value.number_value(scope).unwrap_or(f64::NAN));
    }
    if value.is_string() {
        let string: v8::Local<v8::String> = value.try_into().expect("is_string");
        return Value::String(sample_string(scope, string));
    }
    if value.is_native_error() {
        return Value::Error(error_of(scope, value));
    }
    if value.is_array() {
        let array: v8::Local<v8::Array> = value.try_into().expect("is_array");
        return Value::Array(sample_array(scope, array, depth));
    }
    if value.is_object() {
        let object: v8::Local<v8::Object> = value.try_into().expect("is_object");
        return Value::Object(sample_object(scope, object, depth));
    }
    // A symbol or a bigint: neither has a §3 row, and both have a short
    // canonical spelling.
    Value::string(value.to_rust_string_lossy(scope))
}

/// A string's own length and at most [`STRING_HEAD_SAMPLE`] of its leading
/// characters, read with one bounded write rather than a copy.
fn sample_string(scope: &mut v8::PinScope, string: v8::Local<v8::String>) -> StringValue {
    // Four bytes is the widest a UTF-8 character gets, and `write_utf8_v2`
    // never splits one.
    let mut buffer = [MaybeUninit::<u8>::uninit(); STRING_HEAD_SAMPLE * 4];
    let mut characters = 0usize;
    let written = string.write_utf8_uninit_v2(
        scope,
        &mut buffer,
        v8::WriteFlags::kReplaceInvalidUtf8,
        Some(&mut characters),
    );
    // SAFETY: `write_utf8_uninit_v2` reports how many bytes it initialised,
    // and only that prefix is read.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), written) };
    let head = std::string::String::from_utf8_lossy(bytes).into_owned();
    StringValue::sampled(string.length().max(head.chars().count()), head)
}

fn sample_array(scope: &mut v8::PinScope, array: v8::Local<v8::Array>, depth: u32) -> ArrayValue {
    let len = array.length() as usize;
    let mut head = Vec::with_capacity(ARRAY_HEAD_SAMPLE.min(len));
    for index in 0..ARRAY_HEAD_SAMPLE.min(len) {
        let element = array
            .get_index(scope, index as u32)
            .map_or(Value::Undefined, |value| {
                marshal_at(scope, value, depth + 1)
            });
        head.push(element);
    }
    let last = if len > ARRAY_HEAD_SAMPLE {
        array
            .get_index(scope, (len - 1) as u32)
            .map(|value| marshal_at(scope, value, depth + 1))
    } else {
        None
    };
    ArrayValue::sampled(len, head, last)
}

fn sample_object(
    scope: &mut v8::PinScope,
    object: v8::Local<v8::Object>,
    depth: u32,
) -> ObjectValue {
    let Some(names) = object.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    else {
        return ObjectValue::sampled(0, Vec::new());
    };
    let key_count = names.length() as usize;
    if depth >= MAX_DEPTH {
        // At depth 1 only the shape is rendered, so no value is read at all.
        return ObjectValue::sampled(key_count, Vec::new());
    }
    let mut entries = Vec::with_capacity(OBJECT_KEY_SAMPLE.min(key_count));
    for index in 0..OBJECT_KEY_SAMPLE.min(key_count) {
        let Some(key) = names.get_index(scope, index as u32) else {
            continue;
        };
        let name = key.to_rust_string_lossy(scope);
        let value = object.get(scope, key).map_or(Value::Undefined, |value| {
            marshal_at(scope, value, depth + 1)
        });
        entries.push((name, value));
    }
    ObjectValue::sampled(key_count, entries)
}

/// An `Error` the model constructed or a tool threw, read for its class and
/// its message and for nothing else. Stack frames are attached by the
/// isolate, which is the only place that knows which script is the model's.
pub(crate) fn error_of(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> ErrorValue {
    let object: Option<v8::Local<v8::Object>> = value.try_into().ok();
    // Bounded, like every other read in this module: an `Error` the model
    // built can carry a megabyte of message, and §3 shows two hundred
    // characters of it.
    let read = |scope: &mut v8::PinScope, key: &str| -> Option<std::string::String> {
        let object = object?;
        let key = v8::String::new(scope, key)?;
        let value = object.get(scope, key.into())?;
        if value.is_undefined() {
            return None;
        }
        let text: v8::Local<v8::String> = value.to_string(scope)?;
        Some(sample_string(scope, text).head().to_string())
    };
    ErrorValue {
        class: read(scope, "name").unwrap_or_else(|| "Error".to_string()),
        message: read(scope, "message").unwrap_or_default(),
        line: None,
        column: None,
        stack: Vec::new(),
    }
}

/// A cheap upper-ish bound on what a live value costs the isolate, in bytes.
///
/// **Not a retained size.** V8 offers no per-object retained size short of a
/// heap snapshot, and taking one to answer an out-of-memory error would
/// allocate at exactly the moment there is nothing to allocate from. This
/// counts what the value itself declares — a string's length, an array's
/// length times its first element's estimate, an object's key count — which
/// is enough to rank handles for `runtime-contract.md` §2's "five largest".
pub(crate) fn size_estimate(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> u64 {
    if value.is_string() {
        let string: v8::Local<v8::String> = value.try_into().expect("is_string");
        return string.length() as u64 * 2;
    }
    if value.is_array() {
        let array: v8::Local<v8::Array> = value.try_into().expect("is_array");
        let len = array.length() as u64;
        let per_element = array
            .get_index(scope, 0)
            .map_or(64, |first| element_estimate(scope, first));
        return len.saturating_mul(per_element);
    }
    if value.is_object() {
        let object: v8::Local<v8::Object> = value.try_into().expect("is_object");
        let keys = object
            .get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
            .map_or(0, |names| names.length() as u64);
        return 32 + keys * 64;
    }
    16
}

/// One element's contribution, measured without recursing: an array of
/// arrays is not walked, because walking it would be the payload cost this
/// whole module exists to not pay.
fn element_estimate(scope: &mut v8::PinScope, value: v8::Local<v8::Value>) -> u64 {
    if value.is_string() {
        let string: v8::Local<v8::String> = value.try_into().expect("is_string");
        return string.length() as u64 * 2;
    }
    if value.is_object() {
        let object: v8::Local<v8::Object> = value.try_into().expect("is_object");
        let keys = object
            .get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
            .map_or(0, |names| names.length() as u64);
        return 32 + keys * 64;
    }
    16
}
