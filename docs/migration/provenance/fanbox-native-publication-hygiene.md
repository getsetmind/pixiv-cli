# FANBOX native import publication-hygiene audit

The exact 1,758-file import and eight ordered patches remain byte-preserved.
This documentation-only audit changed no source, tests, Cargo metadata or Git
attributes and skipped no assertion or validation gate. No Cargo, Go, native
code or network operation was executed by the audit worker.

## Actual diagnostic evidence

The parent ran `git diff --no-index --check /dev/null <path>` individually for
the package and patch inputs. Its combined original diagnostic log is
`/tmp/pixiv-fanbox-native-upstream-whitespace-audit.log`, 1,500,575 bytes,
24,601 LF display lines, SHA256
`7528fbc0283a2b3bc9415e23a1f3526641af105c63c0f2c25613888112236a7c`.
A byte-identical [archive of that log](fanbox-native-source-records/whitespace/upstream-whitespace-audit.log)
is retained alongside the [machine-readable per-path audit](fanbox-native-publication-hygiene.json).

The log contains 12,304 diagnostics across exactly 114 paths:

| Source category | Warned paths | Diagnostics |
| --- | ---: | ---: |
| Unmodified official files with CRLF | 32 | 10,091 |
| Unmodified official non-CRLF files | 72 | 945 |
| Modified upstream boringssl.patch | 1 | 75 |
| Modified CRLF wreq-proto files | 2 | 999 |
| Outer ordered patch documents | 7 | 194 |
| Total | 114 | 12,304 |

Diagnostic types are 12,239 trailing-whitespace, 58 space-before-tab in indent,
and seven blank-line-at-EOF reports. Every displayed source snippet was checked
against its exact current file/line: 12,297 snippets plus seven EOF positions,
zero unparsed log lines. The LF display log omits source-terminal CR; only that
terminal CR was removed for display matching, never from the source file.

The reported audit coverage is all 1,758 package files plus eight patches,
1,766 inputs. The combined log contains diagnostics rather than success records;
it cannot independently prove which silent inputs were checked. This audit
separately verified all 1,758 current package hashes and all eight patch hashes
against their sealed manifests. The unchanged inventory SHA256 is
`78a081d82a021f10f80ee8ae00c36374d9cf512fbc4b425ca2d129182269fef7`.

## Why these bytes are retained

All 104 unmodified warned paths were compared byte-for-byte with the official
pristine imports and their original SHA256s. They include upstream CRLF,
assembly/Perl indentation, trailing spaces, original patch context and terminal
blank lines. Rewriting them would invalidate registry provenance and native
Git-blob identity. All 1,435 packaged native blobs remain official bytes.

All 75 diagnostics in the modified `btls-sys-0.5.6/patches/boringssl.patch`
refer to unchanged original lines. Its one seed-index/inclusive-swap correction
has no whitespace diagnostic. Its upstream build-patch mechanism is preserved.

The two modified wreq-proto files preserve uniform CRLF and zero bare-LF lines:
`src/conn/http2.rs` has 290 original and 318 current CRLF lines;
`src/proto/http2/client.rs` has 677 original and 681 current CRLF lines. Their
999 diagnostics are terminal CR under Git's default whitespace interpretation.
No extra horizontal trailing spaces or tabs were added. Existing unchanged
line mappings account for 283 and 673 of those diagnostics; 35 and eight lie
on modified/added CRLF lines. This is a line-ending explanation, not a substitute
for the source patch review or real behavior tests.

The seven warning-bearing outer patches contain 56 required single-space blank
context markers and 138 preserved-CRLF hunk lines. The blank-EOF diagnostic in
patch 0004 is its valid final blank context marker. Patch 0006 intentionally
mixes LF headers with CRLF hunk lines. Stripping these bytes changes unified
patch content or reconstructed source identity. Every ordered patch has already
been independently applied, and the published recipe reconstructs all 1,758
files exactly. Patch 0001 has no no-index whitespace diagnostic.

## Publication and gate boundary

Git can store, stage and commit these exact bytes. `git diff --check` is a
separate diagnostic, not a restriction on Git object representation. Audited
official originals and valid patch documents can therefore retain their bytes
when published under the parent's existing authorization. This audit does not
authorize publication or supersede any configured external CI policy.

The actual tracked working-tree `git diff --check` was rerun read-only and
returned zero with empty stdout/stderr; its [result records](fanbox-native-source-records/whitespace/tracked-diff-check.exit.txt)
are preserved. That tracked check does not turn these untracked/no-index
imports into a zero-warning report. All diagnostics remain visible and counted;
none were deleted or suppressed. Git attributes, source files, expected values
and tests were not changed to make this report pass. Required Rust full-gate
results and their source freeze are recorded separately in
[the runtime result record](fanbox-native-source-records/runtime-scope.json).
