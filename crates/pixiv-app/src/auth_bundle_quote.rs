use unicode_general_category::{GeneralCategory as C, get_general_category};

pub(crate) fn quote(value: &str) -> String {
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{7}' => output.push_str("\\a"),
            '\u{8}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{b}' => output.push_str("\\v"),
            '\u{c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            ch if ch < ' ' || ch == '\u{7f}' => output.push_str(&format!("\\x{:02x}", ch as u32)),
            ch if printable(ch) => output.push(ch),
            ch if ch as u32 <= 0xffff => output.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => output.push_str(&format!("\\U{:08x}", ch as u32)),
        }
    }
    output.push('"');
    output
}
fn printable(ch: char) -> bool {
    let code = ch as u32;
    ch == ' '
        || GO_17_ADDITIONS
            .iter()
            .any(|&(start, end)| start <= code && code <= end)
        || !matches!(
            get_general_category(ch),
            C::Control
                | C::Format
                | C::Surrogate
                | C::PrivateUse
                | C::Unassigned
                | C::LineSeparator
                | C::ParagraphSeparator
                | C::SpaceSeparator
        )
}
// The cached category table is Unicode 16; the frozen Go 1.27.1 codec uses Unicode 17.
const GO_17_ADDITIONS: &[(u32, u32)] = &[
    (0x88f, 0x88f),
    (0xc5c, 0xc5c),
    (0xcdc, 0xcdc),
    (0x1acf, 0x1add),
    (0x1ae0, 0x1aeb),
    (0x20c1, 0x20c1),
    (0x2b96, 0x2b96),
    (0xa7ce, 0xa7cf),
    (0xa7d2, 0xa7d2),
    (0xa7d4, 0xa7d4),
    (0xa7f1, 0xa7f1),
    (0xfbc3, 0xfbd2),
    (0xfd90, 0xfd91),
    (0xfdc8, 0xfdce),
    (0x10940, 0x10959),
    (0x10ec5, 0x10ec7),
    (0x10ed0, 0x10ed8),
    (0x10efa, 0x10efb),
    (0x11b60, 0x11b67),
    (0x11db0, 0x11ddb),
    (0x11de0, 0x11de9),
    (0x16ea0, 0x16eb8),
    (0x16ebb, 0x16ed3),
    (0x16ff2, 0x16ff6),
    (0x187f8, 0x187ff),
    (0x18d09, 0x18d1e),
    (0x18d80, 0x18df2),
    (0x1ccfa, 0x1ccfc),
    (0x1ceba, 0x1ced0),
    (0x1cee0, 0x1cef0),
    (0x1e6c0, 0x1e6de),
    (0x1e6e0, 0x1e6f5),
    (0x1e6fe, 0x1e6ff),
    (0x1f6d8, 0x1f6d8),
    (0x1f777, 0x1f77a),
    (0x1f8d0, 0x1f8d8),
    (0x1fa54, 0x1fa57),
    (0x1fa8a, 0x1fa8a),
    (0x1fa8e, 0x1fa8e),
    (0x1fac8, 0x1fac8),
    (0x1facd, 0x1facd),
    (0x1faea, 0x1faea),
    (0x1faef, 0x1faef),
    (0x1fbfa, 0x1fbfa),
    (0x2b73a, 0x2b73f),
    (0x2cea2, 0x2cead),
    (0x323b0, 0x33479),
];
