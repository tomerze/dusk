use serde_json::{Number, Value};

pub const LARGEST_EXACT_INTEGER: u64 = 1 << 53;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    #[error("number {0} has no exact IEEE 754 double representation")]
    InexactNumber(String),
}

pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut output = Vec::with_capacity(1024);
    write_value(&mut output, value)?;
    Ok(output)
}

fn write_value(output: &mut Vec<u8>, value: &Value) -> Result<(), CanonicalError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => output.extend_from_slice(format_number(number)?.as_bytes()),
        Value::String(text) => write_string(output, text),
        Value::Array(items) => {
            output.push(b'[');
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    output.push(b',');
                }
                write_value(output, item)?;
            }
            output.push(b']');
        }
        Value::Object(members) => {
            let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
            sorted.sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
            output.push(b'{');
            for (position, (name, member)) in sorted.into_iter().enumerate() {
                if position > 0 {
                    output.push(b',');
                }
                write_string(output, name);
                output.push(b':');
                write_value(output, member)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn write_string(output: &mut Vec<u8>, text: &str) {
    output.push(b'"');
    for character in text.chars() {
        match character {
            '"' => output.extend_from_slice(b"\\\""),
            '\\' => output.extend_from_slice(b"\\\\"),
            '\u{8}' => output.extend_from_slice(b"\\b"),
            '\u{c}' => output.extend_from_slice(b"\\f"),
            '\n' => output.extend_from_slice(b"\\n"),
            '\r' => output.extend_from_slice(b"\\r"),
            '\t' => output.extend_from_slice(b"\\t"),
            control if (control as u32) < 0x20 => {
                output.extend_from_slice(format!("\\u{:04x}", control as u32).as_bytes())
            }
            other => {
                let mut buffer = [0u8; 4];
                output.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    output.push(b'"');
}

fn format_number(number: &Number) -> Result<String, CanonicalError> {
    if let Some(unsigned) = number.as_u64() {
        if unsigned <= LARGEST_EXACT_INTEGER {
            return Ok(unsigned.to_string());
        }
        let double = unsigned as f64;
        if double >= 18446744073709551616.0 || double as u64 != unsigned {
            return Err(CanonicalError::InexactNumber(number.to_string()));
        }
        return Ok(format_double(double));
    }
    if let Some(signed) = number.as_i64() {
        if signed.unsigned_abs() <= LARGEST_EXACT_INTEGER {
            return Ok(signed.to_string());
        }
        let double = signed as f64;
        if double < -9223372036854775808.0 || double as i64 != signed {
            return Err(CanonicalError::InexactNumber(number.to_string()));
        }
        return Ok(format_double(double));
    }
    match number.as_f64() {
        Some(double) if double.is_finite() => Ok(format_double(double)),
        _ => Err(CanonicalError::InexactNumber(number.to_string())),
    }
}

pub fn format_double(double: f64) -> String {
    String::from(ryu_js::Buffer::new().format_finite(double))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_text(json: &str) -> String {
        let value: Value = serde_json::from_str(json).unwrap();
        String::from_utf8(canonical_bytes(&value).unwrap()).unwrap()
    }

    #[test]
    fn matches_the_rfc_8785_primitive_example() {
        let input = r#"{
            "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
            "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
            "literals": [null, true, false]
        }"#;
        assert_eq!(
            canonical_text(input),
            r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
        );
    }

    #[test]
    fn sorts_members_by_utf16_code_units() {
        let input = r#"{
            "€": "Euro Sign",
            "\r": "Carriage Return",
            "דּ": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "😀": "Emoji: Grinning Face",
            "\u0080": "Control",
            "ö": "Latin Small Letter O With Diaeresis"
        }"#;
        let value: Value = serde_json::from_str(input).unwrap();
        let canonical = String::from_utf8(canonical_bytes(&value).unwrap()).unwrap();
        let order: Vec<&str> = [
            "Carriage Return",
            "One",
            "Control",
            "Latin Small Letter O With Diaeresis",
            "Euro Sign",
            "Emoji: Grinning Face",
            "Hebrew Letter Dalet With Dagesh",
        ]
        .to_vec();
        let positions: Vec<usize> = order
            .iter()
            .map(|name| canonical.find(name).unwrap())
            .collect();
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(positions, sorted);
    }

    #[test]
    fn formats_doubles_like_ecmascript() {
        let vectors: [(u64, &str); 24] = [
            (0x0000000000000000, "0"),
            (0x8000000000000000, "0"),
            (0x0000000000000001, "5e-324"),
            (0x8000000000000001, "-5e-324"),
            (0x7fefffffffffffff, "1.7976931348623157e+308"),
            (0xffefffffffffffff, "-1.7976931348623157e+308"),
            (0x4340000000000000, "9007199254740992"),
            (0xc340000000000000, "-9007199254740992"),
            (0x4430000000000000, "295147905179352830000"),
            (0x44b52d02c7e14af5, "9.999999999999997e+22"),
            (0x44b52d02c7e14af6, "1e+23"),
            (0x44b52d02c7e14af7, "1.0000000000000001e+23"),
            (0x444b1ae4d6e2ef4e, "999999999999999700000"),
            (0x444b1ae4d6e2ef4f, "999999999999999900000"),
            (0x444b1ae4d6e2ef50, "1e+21"),
            (0x3eb0c6f7a0b5ed8c, "9.999999999999997e-7"),
            (0x3eb0c6f7a0b5ed8d, "0.000001"),
            (0x41b3de4355555553, "333333333.3333332"),
            (0x41b3de4355555554, "333333333.33333325"),
            (0x41b3de4355555555, "333333333.3333333"),
            (0x41b3de4355555556, "333333333.3333334"),
            (0x41b3de4355555557, "333333333.33333343"),
            (0xbecbf647612f3696, "-0.0000033333333333333333"),
            (0x43143ff3c1cb0959, "1424953923781206.2"),
        ];
        for (bits, expected) in vectors {
            assert_eq!(
                format_double(f64::from_bits(bits)),
                expected,
                "bits {bits:016x}"
            );
        }
    }

    #[test]
    fn escapes_only_what_ecmascript_escapes() {
        assert_eq!(
            canonical_text(r#"["\u0000\u001f\u007f /\b\f\t"]"#),
            "[\"\\u0000\\u001f\u{7f}\u{2028}/\\b\\f\\t\"]"
        );
    }

    #[test]
    fn writes_integers_exactly_and_refuses_inexact_ones() {
        assert_eq!(
            canonical_text("[9007199254740992, -9007199254740992, 0, -0]"),
            "[9007199254740992,-9007199254740992,0,0]"
        );
        assert_eq!(
            canonical_text("[1152921504606846976]"),
            "[1152921504606847000]"
        );
        let inexact: Value = serde_json::from_str("[9007199254740993]").unwrap();
        assert_eq!(
            canonical_bytes(&inexact),
            Err(CanonicalError::InexactNumber(String::from(
                "9007199254740993"
            )))
        );
    }

    #[test]
    fn nests_objects_and_arrays_without_whitespace() {
        assert_eq!(
            canonical_text(r#"{ "b" : [ { "d" : 1 , "c" : [ ] } ], "a" : { } }"#),
            r#"{"a":{},"b":[{"c":[],"d":1}]}"#
        );
    }
}
