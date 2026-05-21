use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::Result;

use super::scan::RawEntry;

/// Read all *complete* JSONL entries from `path` starting at byte `offset`.
/// A "complete" line is one terminated by `\n`; if the file is in the middle
/// of being written, any partial trailing line is left for the next call.
///
/// Returns the parsed entries and the new byte offset (the position
/// immediately after the last complete line).
pub fn read_jsonl_since(path: &Path, offset: u64) -> Result<(Vec<RawEntry>, u64)> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();

    if offset >= file_size {
        return Ok((Vec::new(), file_size));
    }

    file.seek(SeekFrom::Start(offset))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    let mut entries = Vec::new();
    let mut complete_bytes: u64 = 0;

    for chunk in content.split_inclusive('\n') {
        if !chunk.ends_with('\n') {
            // Partial trailing line — stop counting here.
            break;
        }
        complete_bytes += chunk.len() as u64;
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<RawEntry>(trimmed) {
            entries.push(entry);
        }
    }

    Ok((entries, offset + complete_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};
    use tempfile::tempdir;

    #[test]
    fn read_from_zero_returns_all_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = "{\"type\":\"user\",\"timestamp\":\"2026-05-21T10:00:00Z\"}\n\
                       {\"type\":\"assistant\",\"timestamp\":\"2026-05-21T10:01:00Z\"}\n";
        fs::write(&path, content).unwrap();

        let (entries, offset) = read_jsonl_since(&path, 0).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(offset, content.len() as u64);
    }

    #[test]
    fn read_from_offset_returns_only_new_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.jsonl");

        let line1 = "{\"type\":\"user\",\"timestamp\":\"2026-05-21T10:00:00Z\"}\n";
        let line2 = "{\"type\":\"assistant\",\"timestamp\":\"2026-05-21T10:01:00Z\"}\n";

        fs::write(&path, line1).unwrap();
        let (entries, offset_after_first) = read_jsonl_since(&path, 0).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(offset_after_first, line1.len() as u64);

        // Append second line
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(line2.as_bytes()).unwrap();
        drop(f);

        let (entries, offset_final) = read_jsonl_since(&path, offset_after_first).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(offset_final, (line1.len() + line2.len()) as u64);
    }

    #[test]
    fn read_skips_partial_trailing_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.jsonl");

        // Two complete lines + one partial (no trailing \n)
        let complete = "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n";
        let partial = "{\"type\":\"user";
        fs::write(&path, format!("{complete}{partial}")).unwrap();

        let (entries, offset) = read_jsonl_since(&path, 0).unwrap();
        assert_eq!(entries.len(), 2);
        // Offset stops at the last \n, not at EOF
        assert_eq!(offset, complete.len() as u64);
    }

    #[test]
    fn read_when_offset_at_eof_returns_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let content = "{\"type\":\"user\"}\n";
        fs::write(&path, content).unwrap();

        let (entries, offset) = read_jsonl_since(&path, content.len() as u64).unwrap();
        assert!(entries.is_empty());
        assert_eq!(offset, content.len() as u64);
    }
}
