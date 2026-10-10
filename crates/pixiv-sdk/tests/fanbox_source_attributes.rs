use std::{path::Path, process::Command};

#[test]
fn pinned_fanbox_sources_and_mixed_line_ending_patches_disable_text_conversion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in [
        "third_party/rust/fanbox/wreq-proto-0.2.5/src/proto/http2.rs",
        "third_party/rust/fanbox/btls-sys-0.5.6/deps/boringssl/CMakeLists.txt",
        "third_party/rust/fanbox/patches/0006-wreq-proto-idle-admission.patch",
        "docs/migration/provenance/fanbox-native-source-records/pristine-import-manifest.json",
    ] {
        assert!(root.join(path).is_file(), "audited input exists: {path}");
        let output = Command::new("git")
            .args(["check-attr", "text", "--", path])
            .current_dir(&root)
            .output()
            .expect("owned repository Git attribute query");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            format!("{path}: text: unset"),
            "audited bytes must survive a Windows checkout"
        );
    }
}
