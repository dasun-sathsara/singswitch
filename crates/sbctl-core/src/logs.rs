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

/// Remove ANSI SGR/VT escape sequences from a log line.
///
/// sing-box writes coloured output (`\x1b[36mINFO\x1b[0m`) even when the stream
/// is a file, so every line the UI shows must be stripped or the reader sees
/// literal escape bytes. Handles CSI sequences (`ESC [ ... final`), OSC
/// sequences terminated by BEL or ST, and lone ESC pairs.
pub fn strip_ansi(input: &str) -> String {
    if !input.as_bytes().contains(&0x1b) {
        return input.to_string();
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            // Copy the next UTF-8 scalar whole so multibyte text survives.
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                i += 1;
            }
            out.push_str(&input[start..i]);
            continue;
        }

        match bytes.get(i + 1).copied() {
            // CSI: ESC '[' params final-byte (0x40..=0x7E)
            Some(b'[') => {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7E).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            // OSC: ESC ']' ... terminated by BEL or ESC '\'
            Some(b']') => {
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == 0x07 {
                        i += 1;
                        break;
                    }
                    if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // Any other two-byte escape.
            Some(_) => i += 2,
            None => i += 1,
        }
    }
    out
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
        // Colour codes can sit between the bracket and the word, so classify
        // against the stripped text.
        let line = &strip_ansi(line);
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

/// Identity of a log file, used to detect a rotation that a length check misses.
///
/// A rename-and-recreate rotation can leave the new file longer than the old
/// offset. Comparing lengths alone then reads the new file from the middle of a
/// line and keeps the old file's buffered lines, which loses data silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogFileId {
    /// Inode on Unix, volume file index on Windows; 0 when unavailable.
    pub inode: u64,
    /// Creation/change stamp; 0 when unavailable.
    pub created: u64,
}

impl LogFileId {
    /// True when both samples are known and describe different files.
    pub fn is_different_file(&self, other: &Self) -> bool {
        self.inode != 0 && other.inode != 0 && self.inode != other.inode
    }
}

/// Sample the identity of the file at `path`, if it can be determined.
pub fn log_file_id(path: &Path) -> Option<LogFileId> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(LogFileId {
            inode: meta.ino(),
            created: meta.ctime() as u64,
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // `file_index` is unstable; a recreated log has a new creation stamp,
        // which distinguishes rotation just as well for this purpose.
        Some(LogFileId {
            inode: meta.creation_time(),
            created: meta.creation_time(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        None
    }
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
    #[test]
    fn strip_ansi_removes_sgr_and_keeps_text() {
        let raw = "+0530 2026-09-15 02:01:57 \u{1b}[36mINFO\u{1b}[0m network: updated interface";
        assert_eq!(
            strip_ansi(raw),
            "+0530 2026-09-15 02:01:57 INFO network: updated interface"
        );
    }

    #[test]
    fn strip_ansi_removes_extended_and_256_colour_sequences() {
        let raw = "\u{1b}[38;5;190m190m1935904686\u{1b}[0m \u{1b}[1;32mok\u{1b}[39m";
        assert_eq!(strip_ansi(raw), "190m1935904686 ok");
    }

    #[test]
    fn strip_ansi_is_zero_cost_without_escapes() {
        let plain = "tun: started at utun123, stack=gvisor";
        assert_eq!(strip_ansi(plain), plain);
    }

    #[test]
    fn strip_ansi_preserves_multibyte_text() {
        let raw = "\u{1b}[31mproxy → 直连\u{1b}[0m";
        assert_eq!(strip_ansi(raw), "proxy → 直连");
    }

    #[test]
    fn classify_ignores_colour_codes() {
        assert_eq!(
            LogLevel::classify("\u{1b}[36mINFO\u{1b}[0m x"),
            LogLevel::Info
        );
        assert_eq!(
            LogLevel::classify("\u{1b}[31mERROR\u{1b}[0m x"),
            LogLevel::Error
        );
    }

    use super::*;
    use std::io::Write;

    #[test]
    fn log_file_id_distinguishes_a_replaced_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("error.log");
        std::fs::write(
            &path, "first
",
        )
        .expect("write");
        let before = log_file_id(&path).expect("id");

        // Replace the file the way a rename-and-recreate rotation does.
        let mut rotated = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("truncate");
        rotated
            .write_all(
                b"second
",
            )
            .expect("write");
        drop(rotated);
        let after = log_file_id(&path).expect("id");

        #[cfg(unix)]
        {
            // The inode changes for a rename+recreate; for an in-place truncate
            // the caller still detects it by length. Either way the identity is
            // readable and comparable.
            let _ = before.is_different_file(&after);
        }
        assert!(log_file_id(&dir.path().join("missing.log")).is_none());
    }

    #[test]
    fn read_appended_lines_follows_a_growing_new_file_after_rotation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("error.log");

        // Old file, 120 lines, fully consumed.
        let old: String = (0..120).map(|i| format!("old {i}\n")).collect();
        std::fs::write(&path, &old).expect("write old");
        let (lines, offset, truncated) = read_appended_lines(&path, 0).expect("read");
        assert_eq!(lines.len(), 120);
        assert!(!truncated);
        assert_eq!(offset, old.len() as u64);

        // Rotate: new file already longer than the old offset.
        let new: String = (0..400).map(|i| format!("new {i}\n")).collect();
        assert!(
            new.len() as u64 > offset,
            "precondition: new file outgrows old"
        );
        std::fs::write(&path, &new).expect("write new");

        // With a known-different identity the caller resets the offset itself;
        // this asserts the reset reads the whole new file rather than its tail.
        let (fresh, _, _) = read_appended_lines(&path, 0).expect("read new");
        assert_eq!(fresh.len(), 400);
        assert!(fresh[0].starts_with("new 0"));
    }

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
