use crate::tracing::{BufferLayer, FieldValue, MESSAGE_FIELD};
use alloc::string::ToString;
use alloc::vec::Vec;
use core::ffi::{CStr, c_char};
use tracing::Level;

const OK: i32 = 0;
const INVALID_SEVERITY: i32 = 1;
const NULL_ARGUMENT: i32 = 2;
const INVALID_UTF8: i32 = 3;
const NOT_INSTALLED: i32 = 4;
const FAILED: i32 = 5;

const TARGET_FIELD: &str = "target";

#[repr(C)]
pub struct Attribute {
    key: *const c_char,
    value: *const c_char,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn dusk_logs_write(
    severity: i32,
    target: *const c_char,
    message: *const c_char,
    attributes: *const Attribute,
    attribute_count: usize,
) -> i32 {
    match unsafe { write(severity, target, message, attributes, attribute_count) } {
        Ok(()) => OK,
        Err(code) => code,
    }
}

unsafe fn write(
    severity: i32,
    target: *const c_char,
    message: *const c_char,
    attributes: *const Attribute,
    attribute_count: usize,
) -> Result<(), i32> {
    let level = match severity {
        0 => Level::TRACE,
        1 => Level::DEBUG,
        2 => Level::INFO,
        3 => Level::WARN,
        4 => Level::ERROR,
        _ => return Err(INVALID_SEVERITY),
    };

    let mut fields: Vec<(&str, FieldValue)> = Vec::with_capacity(attribute_count + 2);
    fields.push((
        MESSAGE_FIELD,
        FieldValue::Text(unsafe { text(message) }?.to_string()),
    ));
    if !target.is_null() {
        fields.push((
            TARGET_FIELD,
            FieldValue::Text(unsafe { text(target) }?.to_string()),
        ));
    }
    let attributes = match attribute_count {
        0 => &[][..],
        _ if attributes.is_null() => return Err(NULL_ARGUMENT),
        _ => unsafe { core::slice::from_raw_parts(attributes, attribute_count) },
    };
    for attribute in attributes {
        let key = unsafe { text(attribute.key) }?;
        let value = unsafe { text(attribute.value) }?;
        fields.push((key, FieldValue::Text(value.to_string())));
    }

    tracing::dispatcher::get_default(|dispatch| match dispatch.downcast_ref::<BufferLayer>() {
        Some(layer) => layer.write_log(level, &fields).map_err(|_| FAILED),
        None => Err(NOT_INSTALLED),
    })
}

unsafe fn text<'a>(pointer: *const c_char) -> Result<&'a str, i32> {
    if pointer.is_null() {
        return Err(NULL_ARGUMENT);
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| INVALID_UTF8)
}
