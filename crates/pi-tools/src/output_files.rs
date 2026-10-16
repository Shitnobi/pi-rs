//! Output files: files pi writes so the model can use output it was not
//! shown in full (full text of truncated tool output, binary MCP resources,
//! images). Port of `src/utils/output-files.ts`.
//!
//! Output can carry private data, so only the user may read the files
//! (mode 0600), and `wx`-style creation never follows a link someone else
//! placed at the path.

use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;

/// Mode for output files: user read/write only.
const OUTPUT_FILE_MODE: u32 = 0o600;

/// A new, unused path: `<tmpdir>/<prefix>-<random hex><extension>`.
/// `extension` includes the dot.
pub fn create_output_file_path(prefix: &str, extension: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    // Node's `randomBytes(8).toString("hex")` — 16 hex chars.
    let hex: String = (0..8).map(|_| format!("{:02x}", rand_byte())).collect();
    dir.push(format!("{}-{}{}", prefix, hex, extension));
    dir
}

fn rand_byte() -> u8 {
    // Read from /dev/urandom (equivalent distribution to randomBytes; the
    // exact values are never compared by the parity harness).
    use std::io::Read;
    let mut buf = [0u8; 1];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf[0]
}

/// Write `data` to a new output file and return its path.
/// Port of `writeOutputFile` (`writeFile(path, data, { mode, flag: "wx" })`).
pub fn write_output_file(prefix: &str, extension: &str, data: &[u8]) -> io::Result<PathBuf> {
    let path = create_output_file_path(prefix, extension);
    write_exclusive(&path, data)?;
    Ok(path)
}

/// Open a new output file for streamed output.
/// Port of `createOutputFileStream` (`createWriteStream(path, { mode, flags: "wx" })`).
pub fn create_output_file_stream(prefix: &str, extension: &str) -> io::Result<(PathBuf, std::fs::File)> {
    let path = create_output_file_path(prefix, extension);
    let file = open_exclusive(&path)?;
    Ok((path, file))
}

fn open_exclusive(path: &std::path::Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OUTPUT_FILE_MODE)
        .open(path)
}

fn write_exclusive(path: &std::path::Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut file = open_exclusive(path)?;
    file.write_all(data)
}
