//! `unsafe` lives in src/notify.rs and nowhere else: the plug-in and wake notifications are the one FFI this
//! crate needs, and a second site would be a second place to audit.

#![cfg(test)]

use std::fs;
use std::path::Path;

#[test]
fn unsafe_appears_only_in_the_notify_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut scanned = 0;
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        scanned += 1;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let code = fs::read_to_string(&path).unwrap();
        let uses = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && l.contains("unsafe"))
            .count();
        if name == "notify.rs" {
            assert!(
                uses > 0,
                "notify.rs is the FFI boundary and should carry it"
            );
        } else {
            assert_eq!(uses, 0, "{name} uses unsafe; keep FFI in src/notify.rs");
        }
    }
    assert!(
        scanned >= 7,
        "scanned only {scanned} files: the scan lost its subject"
    );
}
