"""Verify the Go 1.27.1 / Unicode 17 quote delta against cached Unicode 16."""
import argparse
import hashlib
import json
import pathlib
import re

parser = argparse.ArgumentParser()
parser.add_argument("go_root", type=pathlib.Path)
parser.add_argument("category_tables", type=pathlib.Path)
parser.add_argument("rust_source", type=pathlib.Path)
args = parser.parse_args()
category_source = args.category_tables.read_text()
assert "Unicode version: 16.0.0." in category_source
assert 'const Version = "17.0.0"' in (args.go_root / "src/unicode/tables.go").read_text()
go_source_path = args.go_root / "src/strconv/isprint.go"
go_source = go_source_path.read_text()
old = bytearray(0x110000)
excluded = {"Control", "Format", "Surrogate", "PrivateUse", "Unassigned", "LineSeparator", "ParagraphSeparator", "SpaceSeparator"}
for start, end, category in re.findall(r"\((\d+), (\d+), GeneralCategory::(\w+)\)", category_source):
    if category not in excluded:
        start, end = int(start), int(end)
        old[start:end+1] = b"\1" * (end-start+1)
old[32] = 1
new = bytearray(0x110000)

def table(name):
    block = re.search(r"var " + name + r" = .*?\{(.*?)\}", go_source, re.S)[1]
    return [int(value, 16) for value in re.findall(r"0x[0-9a-f]+", re.sub(r"//[^\n]*", "", block))]

for name in ("isPrint16", "isPrint32"):
    values = table(name)
    for start, end in zip(values[::2], values[1::2]):
        new[start:end+1] = b"\1" * (end-start+1)
for name, offset in (("isNotPrint16", 0), ("isNotPrint32", 0x10000)):
    for value in table(name):
        new[value+offset] = 0
assert not any(before and not after for before, after in zip(old, new))
ranges = []
for code, (before, after) in enumerate(zip(old, new)):
    if after and not before:
        if ranges and ranges[-1][1]+1 == code:
            ranges[-1][1] = code
        else:
            ranges.append([code, code])
rust = args.rust_source.read_text().split("const GO_17_ADDITIONS", 1)[1]
actual = [[int(start, 16), int(end, 16)] for start, end in re.findall(r"\(0x([0-9a-f]+), 0x([0-9a-f]+)\)", rust)]
assert actual == ranges
assert len(ranges) == 47
assert sum(end-start+1 for start, end in ranges) == 4803
print(json.dumps({"scalar_slots_checked": len(old), "new_printable_ranges": len(ranges), "new_printable_codepoints": 4803, "go_isprint_source_sha256": hashlib.sha256(go_source_path.read_bytes()).hexdigest(), "cached_category_source_sha256": hashlib.sha256(args.category_tables.read_bytes()).hexdigest()}, indent=2))
