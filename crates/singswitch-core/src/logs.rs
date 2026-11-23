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
    // Stat first and open only when there is something to read: a poll usually
    // finds an unchanged file, and opening it just to compare lengths doubled
    // the syscalls for an idle log pane.
    let len = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), 0, false));
        }
        Err(err) => return Err(err),
    };
    let (mut start, mut truncated) = if len < offset {
        (0, true)
    } else {
        (offset, false)
    };
    if len == start {
        return Ok((Vec::new(), len, truncated));
    }
    let mut file = File::open(path)?;
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
/// Dropping from the front *before* building the records avoids constructing
/// entries only to discard them.
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

/// Counts of Error and Warn records in a bounded buffer.
///
/// A live log view recounts every poll, which is the whole buffer per appended
/// line. This keeps the counts incrementally instead, and falls back to a
/// recount whenever the prefix it relied on has changed: a shorter buffer
/// (truncation) or a different first record (eviction at the cap, which does
/// not change the length).
#[derive(Debug, Default, Clone, Copy)]
pub struct LogCounts {
    errors: usize,
    warnings: usize,
    covered: usize,
    front: Option<u64>,
}

impl LogCounts {
    pub fn errors(&self) -> usize {
        self.errors
    }

    pub fn warnings(&self) -> usize {
        self.warnings
    }

    /// Bring the counts up to date with `records`, doing only the work that is
    /// still valid.
    pub fn sync<T>(
        &mut self,
        records: &[T],
        id: impl Fn(&T) -> u64,
        level: impl Fn(&T) -> LogLevel,
    ) {
        let front = records.first().map(&id);
        let comparable = records.len() >= self.covered && front == self.front;
        if comparable && front.is_some() {
            for rec in &records[self.covered..] {
                self.tally(level(rec));
            }
        } else if records.is_empty() && self.covered == 0 {
            // Nothing has ever been counted; both are already zero.
        } else {
            self.errors = 0;
            self.warnings = 0;
            for rec in records {
                self.tally(level(rec));
            }
        }
        self.covered = records.len();
        self.front = front;
    }

    fn tally(&mut self, level: LogLevel) {
        match level {
            LogLevel::Error => self.errors += 1,
            LogLevel::Warn => self.warnings += 1,
            _ => {}
        }
    }
}
