use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const MAX_APPEND_CHUNK_BYTES: u64 = 2 * 1024 * 1024;
const PARTIAL_LINE_HOLD_LIMIT: usize = 64 * 1024;

/// Case-insensitive ASCII substring search without heap allocation.
#[inline]
pub fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Parsed severity level for a sing-box log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Other,
}

impl LogLevel {
    pub fn classify(line: &str) -> Self {
        if contains_ignore_ascii_case(line, "FATAL")
            || contains_ignore_ascii_case(line, "ERROR")
            || contains_ignore_ascii_case(line, "ERR ")
        {
            Self::Error
        } else if contains_ignore_ascii_case(line, "WARN") {
            Self::Warn
        } else if contains_ignore_ascii_case(line, "INFO") {
            Self::Info
        } else if contains_ignore_ascii_case(line, "DEBUG")
            || contains_ignore_ascii_case(line, "TRACE")
        {
            Self::Debug
        } else {
            Self::Other
        }
    }
}

/// Return the byte offset in `file` where the final `lines` lines begin, scanning
/// backwards in 4 KiB blocks so large log files are never loaded in full.
pub fn last_lines_offset(file: &mut File, lines: usize) -> std::io::Result<u64> {
    if lines == 0 {
        return Ok(0);
    }
    let size = file.metadata()?.len();
    if size == 0 {
        return Ok(0);
    }

    let mut count = 0usize;
    let mut buf = [0u8; 4096];
    let mut pos = size;
    while pos > 0 {
        let chunk = (pos as usize).min(buf.len());
        pos -= chunk as u64;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..chunk])?;

        for i in (0..chunk).rev() {
            if buf[i] == b'\n' {
                count += 1;
                if count > lines {
                    return Ok(pos + i as u64 + 1);
                }
            }
        }
    }
    Ok(0)
}

/// Read the last `max_lines` from `path`, returning `(lines, new_byte_offset)`.
///
/// If `path` does not exist yet, returns an empty `Vec` and offset `0`.
pub fn read_tail_lines(path: &Path, max_lines: usize) -> std::io::Result<(Vec<String>, u64)> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(err) => return Err(err),
    };
    let len = file.metadata()?.len();
    let start = last_lines_offset(&mut file, max_lines)?;
    file.seek(SeekFrom::Start(start))?;

    let mut raw = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let lines = text.lines().map(ToString::to_string).collect::<Vec<_>>();
    Ok((lines, len))
}

/// Read newly appended lines starting at `offset`, handling log rotation, large catch-up
/// jumps, and partial unterminated trailing writes cleanly.
pub fn read_appended_lines(path: &Path, offset: u64) -> std::io::Result<(Vec<String>, u64, bool)> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), 0, false));
        }
        Err(err) => return Err(err),
    };
    let len = file.metadata()?.len();
    let (mut start, mut truncated) = if len < offset {
        (0, true)
    } else {
        (offset, false)
    };
    if len == start {
        return Ok((Vec::new(), len, truncated));
    }
    if len - start > MAX_APPEND_CHUNK_BYTES {
        start = last_lines_offset(&mut file, 2_000)?;
        truncated = true;
    }

    file.seek(SeekFrom::Start(start))?;
    let mut raw = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut raw)?;

    let (consume_len, next_offset) = match raw.iter().rposition(|&b| b == b'\n') {
        Some(pos) => (pos + 1, start + pos as u64 + 1),
        None => {
            // Wait for the trailing newline unless a single line exceeds PARTIAL_LINE_HOLD_LIMIT.
            if raw.len() < PARTIAL_LINE_HOLD_LIMIT {
                return Ok((Vec::new(), start, truncated));
            }
            (raw.len(), len)
        }
    };

    let text = String::from_utf8_lossy(&raw[..consume_len]);
    let lines = text.lines().map(ToString::to_string).collect();
    Ok((lines, next_offset, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn last_lines_offset_scans_multi_chunk_log_accurately() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        for i in 1..=500 {
            writeln!(
                tmp,
                "INFO [2026-11-18] line number {i:04} padding padding padding"
            )
            .unwrap();
        }
        tmp.flush().unwrap();

        let (lines, offset) = read_tail_lines(tmp.path(), 5).unwrap();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("0496"));
        assert!(lines[4].contains("0500"));

        writeln!(tmp, "ERROR [2026-11-18] line 0501").unwrap();
        tmp.flush().unwrap();
        let (appended, new_offset, truncated) = read_appended_lines(tmp.path(), offset).unwrap();
        assert!(!truncated);
        assert!(new_offset > offset);
        assert_eq!(appended.len(), 1);
        assert_eq!(LogLevel::classify(&appended[0]), LogLevel::Error);
    }

    #[test]
    fn read_appended_lines_waits_for_partial_line_completion() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        write!(tmp, "INFO complete\nWARN half-").unwrap();
        tmp.flush().unwrap();

        let (lines1, off1, truncated1) = read_appended_lines(tmp.path(), 0).unwrap();
        assert!(!truncated1);
        assert_eq!(lines1, vec!["INFO complete"]);

        // Calling again before the newline holds the offset steady.
        let (lines_mid, off_mid, _) = read_appended_lines(tmp.path(), off1).unwrap();
        assert!(lines_mid.is_empty());
        assert_eq!(off_mid, off1);

        writeln!(tmp, "written").unwrap();
        tmp.flush().unwrap();
        let (lines2, off2, _) = read_appended_lines(tmp.path(), off_mid).unwrap();
        assert_eq!(lines2, vec!["WARN half-written"]);
        assert!(off2 > off_mid);
    }
}
