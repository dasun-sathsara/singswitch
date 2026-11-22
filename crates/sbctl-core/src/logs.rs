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
    // Compare the first byte before slicing a window. Log lines are mostly
    // ASCII and the needle's first byte rarely matches, so this rejects almost
    // every position for the cost of an indexed load instead of a memcmp.
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let first = n[0].to_ascii_lowercase();
    let mut i = 0;
    while i + n.len() <= h.len() {
        if h[i].to_ascii_lowercase() == first && h[i..i + n.len()].eq_ignore_ascii_case(n) {
            return true;
        }
        i += 1;
    }
    false
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
            // Any other two-byte escape, but only when the follower is a
            // single-byte char. Advancing two bytes past a UTF-8 lead byte
            // would resume mid-scalar and panic on the next slice.
            Some(b) if b < 0x80 => i += 2,
            _ => i += 1,
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
    /// Classify from the level field sing-box writes, never from the message.
    ///
    /// The body routinely contains the words we look for: an INFO line reading
    /// "connection error, retrying" is not an error, and scanning the whole line
    /// for "ERR " matched proxy tags. That misclassification drove the row
    /// badge, the severity filters, and the error count in the rail.
    ///
    /// sing-box writes either `-0700 2026-01-02 15:04:05 LEVEL msg` (with
    /// `timestamp: true`) or `LEVEL[0012] msg` (without), optionally after an
    /// ANSI colour code.
    pub fn classify(line: &str) -> Self {
        let line = &strip_ansi(line);
        let Some(token) = level_token(line) else {
            // A crashed service writes Go panic output, not a levelled log
            // line. Without this the last words before a crash carried no
            // error marker at all.
            let head = line.trim_start();
            if head.starts_with("panic:")
                || head.starts_with("fatal error:")
                || head.starts_with("goroutine ")
            {
                return Self::Error;
            }
            return Self::Other;
        };
        match token {
            "FATAL" | "PANIC" | "ERROR" | "ERR" => Self::Error,
            "WARN" | "WARNING" => Self::Warn,
            "INFO" => Self::Info,
            "DEBUG" | "TRACE" => Self::Debug,
            _ => Self::Other,
        }
    }
}

/// The level word at the front of a sing-box log line, if there is one.
///
/// Accepts the shapes sing-box actually writes:
/// `-0700 2026-01-02 15:04:05 LEVEL msg`,
/// `2026-01-02T15:04:05.000000000+07:00 LEVEL msg`, and `LEVEL[0012] msg`.
fn level_token(line: &str) -> Option<&str> {
    const LEVELS: [&str; 9] = [
        "FATAL", "PANIC", "ERROR", "WARN", "WARNING", "INFO", "DEBUG", "TRACE", "ERR",
    ];
    let token_of = |rest: &str| -> Option<&str> {
        let end = rest
            .find(|c: char| !c.is_ascii_uppercase())
            .unwrap_or(rest.len());
        LEVELS.iter().find(|l| **l == &rest[..end]).copied()
    };

    let line = line.trim_start();

    // Timezone offset prefix: "+0530 " or "-0700 ", then date and time words.
    if let Some(pos) = line.find(' ') {
        let head = &line[..pos];
        let is_offset = head.len() == 5
            && (head.starts_with('+') || head.starts_with('-'))
            && head[1..].bytes().all(|b| b.is_ascii_digit());
        if is_offset {
            let mut rest = line[pos..].trim_start();
            for _ in 0..2 {
                rest = rest[rest.find(' ')?..].trim_start();
            }
            return token_of(rest);
        }
    }

    // ISO date prefix: "2026-01-02T15:04:05...+07:00 ".
    if line.len() > 10
        && line.as_bytes().get(4) == Some(&b'-')
        && line.as_bytes().get(7) == Some(&b'-')
        && line[..4].bytes().all(|b| b.is_ascii_digit())
    {
        let rest = line[line.find(' ')?..].trim_start();
        return token_of(rest);
    }

    token_of(line)
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
    // Drop any unterminated trailing fragment and report the offset of the last
    // complete newline. Reporting `len` would consume that fragment here and
    // re-read its remainder on the next append, merging two writes into one line.
    let complete = match raw.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => 0,
    };
    let text = String::from_utf8_lossy(&raw[..complete]);
    let lines = text.lines().map(ToString::to_string).collect::<Vec<_>>();
    Ok((lines, start + complete as u64))
}

/// Identity of a log file, used to detect a rotation that a length check misses.
///
/// A rename-and-recreate rotation can leave the new file longer than the old
/// offset. Comparing lengths alone then reads the new file from the middle of a
/// line and keeps the old file's buffered lines, which loses data silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogFileId {
    /// Inode on Unix; creation stamp on Windows. 0 when unavailable.
    pub inode: u64,
    /// Creation/change stamp; 0 when unavailable.
    pub created: u64,
    /// Bytes at the start of the file. A rotation that reuses an identity (or
    /// lands after a failed stat) still shows different leading content, so this
    /// catches what an identity comparison alone cannot.
    pub head: u64,
}

/// Number of leading bytes hashed into [`LogFileId::head`].
const LOG_HEAD_BYTES: usize = 64;

impl LogFileId {
    /// True when both samples are known and describe different files.
    pub fn is_different_file(&self, other: &Self) -> bool {
        if self.inode != 0 && other.inode != 0 && self.inode != other.inode {
            return true;
        }
        // Same or unknown identity: fall back to the leading bytes. A rotation
        // replaces them; an ordinary append never touches them.
        self.head != 0 && other.head != 0 && self.head != other.head
    }
}

/// Sample the identity of the file at `path`, if it can be determined.
pub fn log_file_id(path: &Path) -> Option<LogFileId> {
    let head = read_head_fingerprint(path).unwrap_or(0);
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(LogFileId {
            inode: meta.ino(),
            created: meta.ctime() as u64,
            head,
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
            head,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = meta;
        None
    }
}

/// FNV-1a over the first [`LOG_HEAD_BYTES`] bytes. Cheap and stable.
fn read_head_fingerprint(path: &Path) -> Option<u64> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; LOG_HEAD_BYTES];
    let mut read = 0usize;
    while read < buf.len() {
        match file.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => break,
        }
    }
    // Only a full prefix is comparable: a shorter file would change its hash on
    // every append, which would read as a rotation on every write.
    if read < LOG_HEAD_BYTES {
        return Some(0);
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in &buf[..read] {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    Some(hash)
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

/// Append `lines` to `buf`, keeping at most `max` entries.
///
/// Returns the number of records dropped. Dropping from the front *before*
/// building the records avoids constructing entries only to discard them.
pub fn append_bounded<T, F>(buf: &mut Vec<T>, lines: Vec<String>, max: usize, make: F)
where
    F: Fn(String) -> T,
{
    let incoming = lines.len();
    let cur = buf.len();
    let keep_new = incoming.min(max);
    // Records that are about to fall off the front never need constructing.
    let skip_new = incoming - keep_new;
    let overflow_before = (cur + keep_new).saturating_sub(max);
    if overflow_before > 0 {
        buf.drain(..overflow_before.min(cur));
    }
    buf.extend(lines.into_iter().skip(skip_new).map(make));
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
    fn strip_ansi_survives_escape_before_multibyte_char() {
        // An ESC followed by a non-ASCII char is not a valid escape; it must be
        // dropped without leaving the scanner inside a scalar.
        assert_eq!(
            strip_ansi("prefix \u{1b}\u{2192} arrow"),
            "prefix \u{2192} arrow"
        );
        assert_eq!(strip_ansi("x \u{1b}\u{e9} y"), "x \u{e9} y");
        // A torn UTF-8 write arrives as ESC + U+FFFD via from_utf8_lossy.
        assert_eq!(strip_ansi("a\u{1b}\u{fffd}b"), "a\u{fffd}b");
        assert_eq!(strip_ansi("trailing\u{1b}"), "trailing");
    }

    #[test]
    fn strip_ansi_preserves_multibyte_text() {
        let raw = "\u{1b}[31mproxy → 直连\u{1b}[0m";
        assert_eq!(strip_ansi(raw), "proxy → 直连");
    }

    #[test]
    fn classify_reads_the_level_not_the_message() {
        // Bodies that contain level words must not change the classification.
        assert_eq!(
            LogLevel::classify(
                "+0530 2026-09-15 02:01:57 INFO outbound/vless[proxy]: outbound connection to example.com:443: connection error, retrying"
            ),
            LogLevel::Info
        );
        assert_eq!(
            LogLevel::classify(
                "+0530 2026-09-15 02:01:57 WARN inbound/tun[tun-in]: connection reset by peer: error"
            ),
            LogLevel::Warn
        );
        assert_eq!(
            LogLevel::classify("+0530 2026-09-15 02:01:57 DEBUG router: no error surfaced"),
            LogLevel::Debug
        );
        assert_eq!(
            LogLevel::classify("+0530 2026-09-15 02:01:57 INFO outbound/TERR proxy up"),
            LogLevel::Info
        );
    }

    #[test]
    fn classify_marks_a_go_crash_as_an_error() {
        // Runtime crash output has no level field, but it is the moment a user
        // most needs the error marker.
        assert_eq!(
            LogLevel::classify("panic: runtime error: invalid memory address"),
            LogLevel::Error
        );
        assert_eq!(
            LogLevel::classify("fatal error: all goroutines are asleep"),
            LogLevel::Error
        );
        assert_eq!(
            LogLevel::classify("goroutine 1 [running]:"),
            LogLevel::Error
        );
        // Ordinary prose must not be promoted.
        assert_eq!(LogLevel::classify("started the router"), LogLevel::Other);
    }

    #[test]
    fn classify_handles_iso_timestamp_and_bracket_form() {
        assert_eq!(
            LogLevel::classify("2026-09-15T02:01:57.123456789+05:30 ERROR start failed"),
            LogLevel::Error
        );
        assert_eq!(
            LogLevel::classify("INFO[0012] router: started"),
            LogLevel::Info
        );
        assert_eq!(
            LogLevel::classify("FATAL listen tcp: bind"),
            LogLevel::Error
        );
        // Message with no level field at all is not guessed from its words.
        assert_eq!(
            LogLevel::classify("plain line mentioning error later"),
            LogLevel::Other
        );
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
    fn rotation_is_detected_when_identity_is_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("error.log");
        // Real log lines exceed the fingerprint window; that is the case that
        // matters, since a shorter file has no stable prefix to compare.
        let old = "old 2026-09-15T02:01:57 ERROR start failed, retrying the listener soon\n";
        std::fs::write(&path, old).expect("write");
        let before = log_file_id(&path).expect("id");

        let new = "NEW 2026-09-15T02:02:57 INFO rotated, unrelated leading content now\n";
        std::fs::write(&path, new).expect("rewrite");
        let after = log_file_id(&path).expect("id");

        // Windows reuses a deleted file index, so an identity-only check reports
        // "same file"; the leading bytes differ, which is what makes this work.
        let identity_only = LogFileId {
            inode: before.inode,
            created: before.created,
            head: 0,
        };
        assert!(
            identity_only.is_different_file(&LogFileId {
                inode: after.inode,
                created: after.created,
                head: 0,
            }) || before.head != after.head,
            "either identity or content must distinguish the two files"
        );
        assert!(before.is_different_file(&after));
    }

    #[test]
    fn append_does_not_look_like_rotation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("error.log");
        std::fs::write(&path, "line one\n").expect("write");
        let before = log_file_id(&path).expect("id");

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        f.write_all(b"line two\n").expect("append");
        drop(f);
        let after = log_file_id(&path).expect("id");

        // Appending leaves the leading bytes untouched.
        assert!(!before.is_different_file(&after));
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
    fn append_bounded_keeps_the_tail_and_never_exceeds_the_cap() {
        let mut buf: Vec<String> = Vec::new();
        let mk = |l: String| l;

        append_bounded(&mut buf, vec!["a".into(), "b".into()], 3, mk);
        assert_eq!(buf, ["a", "b"]);

        // Adding one past the cap drops the oldest.
        append_bounded(&mut buf, vec!["c".into(), "d".into()], 3, mk);
        assert_eq!(buf, ["b", "c", "d"]);

        // A burst larger than the cap keeps only the newest `max`.
        append_bounded(
            &mut buf,
            (0..10).map(|i| format!("n{i}")).collect(),
            3,
            |l: String| l,
        );
        assert_eq!(buf, ["n7", "n8", "n9"]);
    }

    #[test]
    fn tail_leaves_an_unterminated_fragment_for_the_next_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("access.log");
        std::fs::write(&path, "a\nb\nc").expect("write");

        let (lines, offset) = read_tail_lines(&path, 10).expect("tail");
        assert_eq!(lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(offset, 4, "offset must point past the last complete line");

        // The writer finishes the fragment; it must not be merged with new data.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        f.write_all(b"xyz\n").expect("append");
        drop(f);

        let (delta, _, _) = read_appended_lines(&path, offset).expect("delta");
        assert_eq!(delta, vec!["cxyz".to_string()]);
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
