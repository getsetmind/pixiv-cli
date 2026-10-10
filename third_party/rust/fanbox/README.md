# Recovered FANBOX native dependency sources

These five package directories are exact official registry releases with the
15 file modifications recorded in [MODIFICATIONS.md](MODIFICATIONS.md). There
are 1,758 package files, no additions or removals inside a package, and 1,743
files remain byte-identical to their registry originals. Newly added sibling
provenance documents and patch files are outside those package inventories.

| Package | Files | Official `.crate` SHA256 | Declared license |
| --- | ---: | --- | --- |
| wreq 6.0.0-rc.31 | 88 | `0b100d81c46113fa7f0719e43b26b13264b38589a4f059e6ebfccb5308159336` | Apache-2.0 |
| btls 0.5.6 | 108 | `2c5e60b8c8d282c86360cab651ded04ab0335a7b5390c8d34145cbeab8cacf5f` | Apache-2.0 |
| btls-sys 0.5.6 | 1,452 | `9b1b8638a2e1c38a5ae4efa90ae57e643baec35a30d03fc5b399b893adc4954b` | MIT, plus native notices |
| wreq-proto 0.2.5 | 39 | `a43942f024bb303f1042c9aa3c87fa1d9149f507c65db6e5220a11ccdb207387` | Apache-2.0 |
| http2 0.5.20 | 71 | `92d3114be2f413b2e491e686b93a28cda30c355cffc8d091a57f8be4b1342896` | MIT |

All Cargo.toml, Cargo.toml.orig, Cargo.lock and license-file bytes remain exact.
The original sparse-index record and crates.io API checksum independently agree
with every archive hash. Captured records, exact official download URLs, full
original/current file inventories and patch hashes are stored in
[the reconstruction provenance](../../../docs/migration/provenance/fanbox-native-source-reconstruction.json).
The records retain retrieval-time information; they do not claim current
registry metadata or yank status.

## Native source identity and build-patch boundary

The btls repository revision is `4edbf5d716ba014384569ac5c631cea83827abfc`.
Its `btls-sys/deps/boringssl` Gitlink points to official BoringSSL commit
`91a66a59b6c1435120ff83e245d7719411294386`, root tree
`258beb93b3d6101c44e172b16e105d2547152dac`. The captured complete Git trees were
independently rehashed: 25 btls tree objects and 410 BoringSSL tree objects.
Every one of the 1,435 packaged native blobs still equals its official Git blob
ID and byte length. The package is a subset of the complete upstream tree.

Patch 0001 changes only one added line in btls-sys/patches/boringssl.patch:
`seeds[i - offset - 1] % (i - offset + 1)`. With `rem - 1` allocated seeds, the
first iteration now selects seed `rem - 2` rather than `rem - 1`, and the swap
range includes the current suffix element. The native source directories and
upstream build/main.rs remain unchanged. The upstream builder still copies the
native tree to OUT_DIR and applies boring-pq.patch, boringssl.patch,
boringssl-loongarch.patch and boringssl-windows.patch, then feature-gated
underscore-wildcards.patch. Its existing assume-patched/path alternatives are
unchanged. This source reconstruction did not run that builder or native code.

## Ordered source patches

Apply every entry of [patches/series](patches/series) in order, from a pristine
five-package source root. The series separates the seed-bound correction,
requested trust anchors, fallible SSL extra-data ownership, TLS/certificate
policies, atomic HTTP/2 idle shutdown, dispatch admission, logical authority,
and pool lifecycle. These are reconstructed current patches, not the lost
historical seven-patch series.

The wreq-proto sources use CRLF. Patch 0006 deliberately contains CRLF hunk
lines and LF patch headers. Treat sources and patches as byte-preserved inputs;
do not convert line endings, reformat imported sources, or use whitespace-fixing
application for this outer series. The upstream inner native build-patch
mechanism is a separate boundary.

## Reproduce from official archives without compiling sources

Obtain the five official `.crate` archives at the exact URLs in the reconstruction
manifest and place them in an archive directory. This recipe uses only already
available archives, Python, and Git. Run it from the repository root, replacing
`ARCHIVES_DIR` with that directory. It safely extracts fresh originals, verifies
every original byte, applies the ordered series with `core.autocrlf=false`, and
checks every patched byte. It makes no network request and runs no Cargo, Go or
native code. The temporary reconstruction is retained for inspection.

```sh
python3 - "$PWD" "ARCHIVES_DIR" <<'PY'
import hashlib, json, pathlib, subprocess, sys, tarfile, tempfile
repo, archives = map(pathlib.Path, sys.argv[1:3])
repo = repo.resolve()
records = repo / 'docs/migration/provenance/fanbox-native-source-records'
proof = json.loads((repo / 'docs/migration/provenance/fanbox-native-source-reconstruction.json').read_bytes())
pristine = json.loads((records / 'pristine-import-manifest.json').read_bytes())
patched = json.loads((records / 'patched-family-manifest.json').read_bytes())
patches = json.loads((records / 'patch-series-manifest.json').read_bytes())
sha = lambda data: hashlib.sha256(data).hexdigest()
work = pathlib.Path(tempfile.mkdtemp(prefix='fanbox-source-reproduction-'))
expected = {f['crate'] + '/' + row['path']: row for f in pristine['families'] for row in f['files']}
seen = set()
for package in proof['registry_archives_independently_checked_and_extracted']:
    name = package['name'] + '-' + package['version']
    archive = archives / (name + '.crate')
    assert sha(archive.read_bytes()) == package['official_checksum'], archive
    index = json.loads((repo / package['registry_index_record']['path']).read_bytes())
    api = json.loads((repo / package['crates_io_api_record']['path']).read_bytes())['version']
    assert index['cksum'] == api['checksum'] == package['official_checksum']
    with tarfile.open(archive, 'r:gz') as source:
        for member in source.getmembers():
            path = pathlib.PurePosixPath(member.name)
            assert path.parts and path.parts[0] == name
            assert not path.is_absolute() and '..' not in path.parts and '\\' not in member.name
            assert member.isdir() or member.isfile(), member.name
            if member.isdir():
                continue
            assert member.name in expected and member.name not in seen, member.name
            seen.add(member.name)
            data = source.extractfile(member).read()
            row = expected[member.name]
            assert len(data) == row['bytes'] and sha(data) == row['sha256'], member.name
            destination = work / member.name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            destination.chmod(0o644)
assert seen == set(expected)
series = (repo / 'third_party/rust/fanbox/patches/series').read_text().splitlines()
assert series == [pathlib.Path(p['path']).name for p in patches['patches']]
for patch in patches['patches']:
    source = repo / patch['path']
    assert sha(source.read_bytes()) == patch['sha256'], source
    command = ['git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn']
    subprocess.run(command + ['--check', str(source)], cwd=work, check=True)
    subprocess.run(command + [str(source)], cwd=work, check=True)
expected = {f['crate'] + '/' + row['path']: row for f in patched['families'] for row in f['files']}
actual = {p.relative_to(work).as_posix() for p in work.rglob('*') if p.is_file()}
assert actual == set(expected)
inventory = b''
for name, row in sorted(expected.items()):
    data = (work / name).read_bytes()
    assert len(data) == row['bytes'] and sha(data) == row['sha256'], name
    assert data.count(b'\r\n') == row['crlf_count'], name
    assert data.count(b'\n') - data.count(b'\r\n') == row['bare_lf_count'], name
    inventory += (sha(data) + '  ' + name + '\n').encode()
assert sha(inventory) == patched['inventory_sha256']
print('Byte-identical reconstruction:', work)
print('Files:', len(actual), 'inventory SHA256:', sha(inventory))
PY
```

The recorded independent extraction/application passed for all 1,758 files.
Patched/reconstructed inventory SHA256:
`78a081d82a021f10f80ee8ae00c36374d9cf512fbc4b425ca2d129182269fef7`.
The hash is SHA256 over sorted UTF-8 lines: file SHA256, two spaces,
family-relative POSIX path, LF. File hashes always use raw bytes.

## Licensing and validation scope

Retain the six complete supplied license files. The [license inventory](../../../docs/migration/provenance/fanbox-native-source-records/licenses.json)
records their unchanged hashes and the embedded BoringSSL notice boundary.
MODIFICATIONS.md names every current patch and changed file. The existing Go
licensebundle check covers only internal/media/ugoira/rust and does not cover
the root Rust CLI/native graph. A complete Rust CLI release dependency/license
bundle, including the newly added dependency closure, remains distribution debt.
No downstream Rust binary/release license bundle was verified for this record.

The [runtime scope record](../../../docs/migration/provenance/fanbox-native-source-records/runtime-scope.json)
archives the original first/second focused RED logs and third GREEN log, exit
codes, and six lossless third-run observations. The third approved run passed
sequential reuse/idle closure, a 31.22-second active header stall with physical
cancellation, caller deadline, and three certificate failures on Linux x86_64
through the genuine Rust native raw transport boundary. The scaffold's no-mode
return is excluded from meaningful assertions. Test/helper projection/schema
repairs are distinguished from production source changes.

This focused native evidence is not private Go Session/public SDK policy
parity, full aggregate Rust validation, or cross-platform verification. Current
source hashes collected for that run are explicitly post-run snapshots; no
at-run focused-source freeze existed. Aggregate gate and independent review
status must be taken from their separate final records.

HTTP/1, resumption/PSK, HelloRetryRequest, fragmented ClientHello, full server
flight, native no-follow redirects, concurrent Read/Close, Windows/macOS/other
architectures and historical deflate/HEADERS-EOS/nonzero Content-Length gates
remain outside this source series' proof. The safety-stopped supplemental
multiplex/upload operation and unfinished HEAD/upload boundaries supply no
evidence and were not retried or probed by this work.
