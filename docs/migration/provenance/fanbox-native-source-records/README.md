# FANBOX native source proof records

Start with [the reconstruction result](../fanbox-native-source-reconstruction.json)
and [the source recipe](../../../../third_party/rust/fanbox/README.md).

- pristine-import-manifest.json is the exact full original 1,758-file import inventory
- patched-family-manifest.json identifies every current package byte, including CRLF counts
- patch-series-manifest.json orders eight narrow patches and records before/after hashes
- registry/ preserves the raw sparse-index and crates.io API records for each exact release
- git/ preserves the btls commit/tree and official BoringSSL commit/root-tree records
- boringssl-current-git-verification.json identifies all 1,435 unchanged packaged native blobs
- official-source-recovery-manifest.json preserves the original retrieval/verification record
- licenses.json identifies six retained complete upstream license files
- runtime-scope.json distinguishes focused Linux native evidence, post-run hashes and pending gates
- runtime/ archives the two failed focused logs, final successful log/exit, and six lossless observations

Paths inside the original recovery manifest are retrieval-time sibling workspace
paths. They are preserved as historical metadata; the reconstruction result
maps the copied registry and Git records to repository-relative paths. The
Gitiles response retains its original anti-XSSI prefix and is intentionally
saved as text; parse the JSON after its first line.

The source proof consists of offline archive checks, safe re-extraction, raw
byte hashing, independent complete Git-tree rehashes, native blob verification
and ordered Git patch application. This worker executed no Cargo, Go, native
code, network operation or probe. Runtime files are observations supplied by
the separately authorized executor, archived through read-only inspection.
