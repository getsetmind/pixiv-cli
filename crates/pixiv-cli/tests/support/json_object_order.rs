use serde_json::Value;

pub fn canonicalize(input: &[u8]) -> Vec<u8> {
    serde_json::from_slice::<Value>(input).expect("valid JSON output");
    let mut parser = Parser { input, position: 0 };
    let mut output = parser.whitespace();
    output.extend(parser.value());
    output.extend(parser.whitespace());
    assert_eq!(parser.position, input.len());
    output
}

pub fn canonicalize_ndjson(input: &[u8]) -> Vec<u8> {
    input
        .split_inclusive(|byte| *byte == b'\n')
        .flat_map(canonicalize)
        .collect()
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
}
impl Parser<'_> {
    fn whitespace(&mut self) -> Vec<u8> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
        self.input[start..self.position].to_vec()
    }
    fn string(&mut self) -> Vec<u8> {
        let start = self.position;
        assert_eq!(self.input[self.position], b'"');
        self.position += 1;
        loop {
            match self.input[self.position] {
                b'\\' => self.position += 2,
                b'"' => {
                    self.position += 1;
                    break;
                }
                _ => self.position += 1,
            }
        }
        self.input[start..self.position].to_vec()
    }
    fn value(&mut self) -> Vec<u8> {
        match self.input[self.position] {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string(),
            _ => {
                let start = self.position;
                while self
                    .input
                    .get(self.position)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && !b",]}".contains(byte))
                {
                    self.position += 1;
                }
                self.input[start..self.position].to_vec()
            }
        }
    }
    fn array(&mut self) -> Vec<u8> {
        self.position += 1;
        let mut output = vec![b'['];
        output.extend(self.whitespace());
        if self.input[self.position] != b']' {
            loop {
                output.extend(self.value());
                output.extend(self.whitespace());
                if self.input[self.position] != b',' {
                    break;
                }
                self.position += 1;
                output.push(b',');
                output.extend(self.whitespace());
            }
        }
        assert_eq!(self.input[self.position], b']');
        self.position += 1;
        output.push(b']');
        output
    }
    fn object(&mut self) -> Vec<u8> {
        self.position += 1;
        let mut slots = vec![self.whitespace()];
        let mut entries = vec![];
        let suffix;
        if self.input[self.position] == b'}' {
            suffix = vec![];
        } else {
            loop {
                let mut entry = self.string();
                let key: String = serde_json::from_slice(&entry).unwrap();
                entry.extend(self.whitespace());
                assert_eq!(self.input[self.position], b':');
                self.position += 1;
                entry.push(b':');
                entry.extend(self.whitespace());
                entry.extend(self.value());
                entries.push((key, entry));
                let space = self.whitespace();
                if self.input[self.position] != b',' {
                    suffix = space;
                    break;
                }
                self.position += 1;
                let mut slot = space;
                slot.push(b',');
                slot.extend(self.whitespace());
                slots.push(slot);
            }
        }
        assert_eq!(self.input[self.position], b'}');
        self.position += 1;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut output = vec![b'{'];
        if entries.is_empty() {
            output.extend(slots.remove(0));
        }
        for (slot, (_, entry)) in slots.into_iter().zip(entries) {
            output.extend(slot);
            output.extend(entry);
        }
        output.extend(suffix);
        output.push(b'}');
        output
    }
}
