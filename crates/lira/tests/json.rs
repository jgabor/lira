//! Reads each file of the JSONTestSuite with Lira's JSON reader. Lira's
//! author did not write these files, so they say what JSON is.

use lira::json;
use std::fs;
use std::path::Path;

/// Each file whose name starts with `start`, with its bytes.
fn files(start: &str) -> Vec<(String, Vec<u8>)> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/JSONTestSuite/test_parsing");
    let mut files = Vec::new();
    for entry in fs::read_dir(folder).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(start) {
            files.push((name, fs::read(entry.path()).unwrap()));
        }
    }
    files.sort();
    files
}

/// Reads the bytes of a file. Lira reads only text that is UTF-8.
fn read(bytes: &[u8]) -> Option<json::Json> {
    let text = std::str::from_utf8(bytes).ok()?;
    json::read(text).ok()
}

#[test]
fn accepts_each_file_that_a_reader_must_accept() {
    let files = files("y_");
    assert_eq!(files.len(), 95);
    for (name, bytes) in files {
        let value = read(&bytes).unwrap_or_else(|| panic!("{name} is refused"));
        // What Lira writes is one line that reads as the same value.
        let line = value.to_string();
        assert!(!line.contains('\n'), "{name}");
        assert_eq!(json::read(&line).ok(), Some(value), "{name}");
    }
}

#[test]
fn refuses_each_file_that_a_reader_must_refuse() {
    let files = files("n_");
    assert_eq!(files.len(), 186);
    let accepted: Vec<String> = files
        .into_iter()
        .filter(|(_, bytes)| read(bytes).is_some())
        .map(|(name, _)| name)
        .collect();
    assert_eq!(accepted, Vec::<String>::new());
}

#[test]
fn reads_each_file_that_a_reader_can_accept_or_refuse() {
    let files = files("i_");
    assert_eq!(files.len(), 35);
    for (_, bytes) in files {
        // Either answer is right. The reader must only give one.
        let _ = read(&bytes);
    }
}
