use crate::Result;

pub(super) fn first_value(raw: &[u8]) -> Result<String> {
    // Decoder.Decode ignores later values, including their syntax and depth failures.
    crate::codec::normalize_json(&raw[..first_value_end(raw)])
}

fn first_value_end(raw: &[u8]) -> usize {
    let Some(start) = raw
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
    else {
        return raw.len();
    };
    let remaining = &raw[start..];
    for literal in [b"null".as_slice(), b"true", b"false"] {
        if remaining.starts_with(literal) {
            return start + literal.len();
        }
    }
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    let container = matches!(raw[start], b'{' | b'[');
    let string = raw[start] == b'"';
    if !container && !string {
        return raw.len();
    }
    for (offset, &byte) in raw[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                if string {
                    return start + offset + 1;
                }
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 10000 {
                        return start + offset + 1;
                    }
                }
                b'}' | b']' => {
                    let Some(next) = depth.checked_sub(1) else {
                        return start + offset + 1;
                    };
                    depth = next;
                    if depth == 0 {
                        return start + offset + 1;
                    }
                }
                _ => {}
            }
        }
    }
    raw.len()
}
