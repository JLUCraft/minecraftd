use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CRASH_MARKER: &str = "#@!@# Game crashed! Crash report saved to: #@!@# ";
const LOG_TAIL_BYTES: u64 = 8192;

/// Try to parse the crash report path from the game log.
pub fn parse_crash_report_path_from_log<P: AsRef<Path>>(log_path: P) -> Option<PathBuf> {
    let file = File::open(log_path).ok()?;
    let mut reader = BufReader::new(file);

    // Move to the end of the file and only read the last chunk for parsing.
    let file_size = reader.seek(SeekFrom::End(0)).ok()?;
    let start_pos = file_size.saturating_sub(LOG_TAIL_BYTES);
    reader.seek(SeekFrom::Start(start_pos)).ok()?;

    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).ok()?;
    let content = String::from_utf8_lossy(&bytes);

    // Scan backwards so the most recent crash report wins.
    for line in content.lines().rev() {
        if let Some(path) = line
            .strip_prefix(CRASH_MARKER)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            return Some(PathBuf::from(path));
        }
    }

    None
}
