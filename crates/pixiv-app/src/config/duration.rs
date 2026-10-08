pub(super) fn parse(original: &str) -> Result<i64, String> {
    let invalid = || format!("time: invalid duration {original:?}");
    let (negative, mut remaining) = match original.as_bytes().first() {
        Some(b'-') => (true, &original[1..]),
        Some(b'+') => (false, &original[1..]),
        _ => (false, original),
    };
    if remaining == "0" {
        return Ok(0);
    }
    if remaining.is_empty() {
        return Err(invalid());
    }
    let limit = 1u64 << 63;
    let mut total = 0u64;
    while !remaining.is_empty() {
        let integer_len = remaining.bytes().take_while(u8::is_ascii_digit).count();
        let mut integer = 0u64;
        for digit in remaining.bytes().take(integer_len) {
            integer = integer
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(digit - b'0')))
                .filter(|value| *value <= limit)
                .ok_or_else(invalid)?;
        }
        remaining = &remaining[integer_len..];
        let mut fraction = 0u64;
        let mut scale = 1.0f64;
        let mut fraction_len = 0;
        if let Some(rest) = remaining.strip_prefix('.') {
            fraction_len = rest.bytes().take_while(u8::is_ascii_digit).count();
            let mut overflow = false;
            for digit in rest.bytes().take(fraction_len) {
                if overflow {
                    continue;
                }
                let next = fraction
                    .checked_mul(10)
                    .and_then(|value| value.checked_add(u64::from(digit - b'0')))
                    .filter(|value| *value <= limit);
                if let Some(value) = next {
                    fraction = value;
                    scale *= 10.0;
                } else {
                    overflow = true;
                }
            }
            remaining = &rest[fraction_len..];
        }
        if integer_len == 0 && fraction_len == 0 {
            return Err(invalid());
        }
        let unit_len = remaining
            .bytes()
            .take_while(|byte| *byte != b'.' && !byte.is_ascii_digit())
            .count();
        if unit_len == 0 {
            return Err(format!("time: missing unit in duration {original:?}"));
        }
        let unit = &remaining[..unit_len];
        remaining = &remaining[unit_len..];
        let multiplier = match unit {
            "ns" => 1,
            "us" | "µs" | "μs" => 1000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => {
                return Err(format!(
                    "time: unknown unit {unit:?} in duration {original:?}"
                ));
            }
        };
        let part = integer
            .checked_mul(multiplier)
            .filter(|value| *value <= limit)
            .ok_or_else(invalid)?;
        let fraction = (fraction as f64 * (multiplier as f64 / scale)) as u64;
        total = total
            .checked_add(part)
            .and_then(|value| value.checked_add(fraction))
            .filter(|value| *value <= limit)
            .ok_or_else(invalid)?;
    }
    if negative {
        Ok((total as i64).wrapping_neg())
    } else {
        i64::try_from(total).map_err(|_| invalid())
    }
}

fn decimal(value: u64, multiplier: u64, width: usize) -> String {
    let whole = value / multiplier;
    let fraction = value % multiplier;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!(
            "{whole}.{}",
            format!("{fraction:0width$}").trim_end_matches('0')
        )
    }
}

pub(super) fn format(value: u64) -> String {
    if value == 0 {
        return "0s".into();
    }
    if value < 1000 {
        return format!("{value}ns");
    }
    if value < 1_000_000 {
        return format!("{}µs", decimal(value, 1000, 3));
    }
    if value < 1_000_000_000 {
        return format!("{}ms", decimal(value, 1_000_000, 6));
    }
    let hours = value / 3_600_000_000_000;
    let minutes = value / 60_000_000_000 % 60;
    let seconds = decimal(value % 60_000_000_000, 1_000_000_000, 9);
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}
