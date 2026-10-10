use chrono::NaiveDate;

pub(super) fn parse(text: &str) -> Option<(i64, u32)> {
    // General date parsers differ from Go's two layouts on fractions, offsets and weekdays.
    parse_rfc3339(text.as_bytes()).or_else(|| parse_http_date(text.as_bytes()))
}

fn parse_rfc3339(text: &[u8]) -> Option<(i64, u32)> {
    let mut parser = Parser { remaining: text };
    let year = parser.fixed_number(4)?;
    parser.literal(b"-")?;
    let month = parser.fixed_number(2)?;
    parser.literal(b"-")?;
    let day = parser.fixed_number(2)?;
    parser.literal(b"T")?;
    let clock = parser.clock()?;
    let offset = if parser.remaining.first() == Some(&b'Z') {
        parser.literal(b"Z")?;
        0
    } else {
        let sign = match parser.remaining.first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        parser.remaining = &parser.remaining[1..];
        let hour = parser.fixed_number(2)?;
        parser.literal(b":")?;
        let minute = parser.fixed_number(2)?;
        if hour > 24 || minute > 60 {
            return None;
        }
        i64::from((hour * 60 + minute) * 60) * sign
    };
    if !parser.remaining.is_empty() {
        return None;
    }
    timestamp(year, month, day, clock, offset)
}

fn parse_http_date(text: &[u8]) -> Option<(i64, u32)> {
    let mut parser = Parser { remaining: text };
    parser.name(&[b"Sun", b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat"])?;
    parser.literal(b",")?;
    parser.spaces()?;
    let day = parser.fixed_number(2)?;
    parser.spaces()?;
    let month = parser.name(&[
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ])? + 1;
    parser.spaces()?;
    let year = parser.fixed_number(4)?;
    parser.spaces()?;
    let clock = parser.clock()?;
    parser.spaces()?;
    parser.literal(b"GMT")?;
    if !parser.remaining.is_empty() {
        return None;
    }
    timestamp(year, month, day, clock, 0)
}

fn timestamp(year: u32, month: u32, day: u32, clock: Clock, offset: i64) -> Option<(i64, u32)> {
    let date = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?
        .and_hms_nano_opt(clock.hour, clock.minute, clock.second, clock.nanoseconds)?
        .and_utc();
    Some((date.timestamp() - offset, clock.nanoseconds))
}

struct Clock {
    hour: u32,
    minute: u32,
    second: u32,
    nanoseconds: u32,
}

struct Parser<'a> {
    remaining: &'a [u8],
}

impl Parser<'_> {
    fn literal(&mut self, literal: &[u8]) -> Option<()> {
        self.remaining = self.remaining.strip_prefix(literal)?;
        Some(())
    }

    fn spaces(&mut self) -> Option<()> {
        self.literal(b" ")?;
        let length = self
            .remaining
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        self.remaining = &self.remaining[length..];
        Some(())
    }

    fn fixed_number(&mut self, width: usize) -> Option<u32> {
        let digits = self.remaining.get(..width)?;
        let number = digits.iter().try_fold(0, |number, byte| {
            byte.is_ascii_digit()
                .then(|| number * 10 + u32::from(byte - b'0'))
        })?;
        self.remaining = &self.remaining[width..];
        Some(number)
    }

    fn name(&mut self, names: &[&[u8]]) -> Option<u32> {
        let index = names.iter().position(|name| {
            self.remaining
                .get(..name.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(name))
        })?;
        self.remaining = &self.remaining[names[index].len()..];
        u32::try_from(index).ok()
    }

    fn clock(&mut self) -> Option<Clock> {
        let width = if self.remaining.get(1).is_some_and(u8::is_ascii_digit) {
            2
        } else {
            1
        };
        let hour = self.fixed_number(width)?;
        self.literal(b":")?;
        let minute = self.fixed_number(2)?;
        self.literal(b":")?;
        let second = self.fixed_number(2)?;
        if hour >= 24 || minute >= 60 || second >= 60 {
            return None;
        }
        Some(Clock {
            hour,
            minute,
            second,
            nanoseconds: self.fraction()?,
        })
    }

    fn fraction(&mut self) -> Option<u32> {
        if !matches!(self.remaining.first(), Some(b'.' | b',')) {
            return Some(0);
        }
        self.remaining = &self.remaining[1..];
        let length = self
            .remaining
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if length == 0 {
            return None;
        }
        let width = length.min(9);
        let mut nanoseconds = self.remaining[..width]
            .iter()
            .fold(0, |number, byte| number * 10 + u32::from(byte - b'0'));
        for _ in width..9 {
            nanoseconds *= 10;
        }
        self.remaining = &self.remaining[length..];
        Some(nanoseconds)
    }
}
