//! Node.js fs error message reproduction.
//!
//! The original tools run on Node.js and propagate raw fs errors (e.g.
//! `ENOENT: no such file or directory, open '/x'`). To keep tool error output
//! byte-identical, we reconstruct Node's error strings from io::Error values.

use std::io;

/// Node error code for an io error.
pub fn node_code(err: &io::Error) -> &'static str {
    match err.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::InvalidInput => "EINVAL",
        _ => match err.raw_os_error() {
            Some(21) => "EISDIR",
            Some(20) => "ENOTDIR",
            _ => "UNKNOWN",
        },
    }
}

/// Full Node-style error message, e.g.
/// `ENOENT: no such file or directory, open '/tmp/x'`.
/// `syscall` is the Node operation name ("open", "access", "stat", "scandir",
/// "mkdir").
pub fn node_error_message(syscall: &str, path: &str, err: &io::Error) -> String {
    let code = node_code(err);
    let reason = match code {
        "ENOENT" => "no such file or directory",
        "EACCES" => "permission denied",
        "EEXIST" => "file already exists",
        "EISDIR" => "illegal operation on a directory",
        "ENOTDIR" => "not a directory",
        "EINVAL" => "invalid argument",
        _ => "operation not permitted",
    };
    format!("{}: {}, {} '{}'", code, reason, syscall, path)
}

/// Node's `fsPromises.access(path, fs.constants.R_OK)` — Err with a
/// Node-style message on failure.
pub fn node_access(path: &str) -> Result<(), String> {
    // access(R_OK) succeeds even for write-only files owned by root etc.;
    // use faccessat via std: opening for read is close enough for the common
    // cases and exactly matches the EACCES/ENOENT messages we mirror.
    match std::fs::File::open(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(node_error_message("access", path, &err)),
    }
}

/// Node's `fsPromises.readFile` as UTF-8 string.
pub fn node_read_file(path: &str) -> Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) => Err(node_error_message("open", path, &err)),
    }
}

/// Node's `fsPromises.writeFile(path, content, "utf-8")`.
pub fn node_write_file(path: &str, content: &str) -> Result<(), String> {
    match std::fs::write(path, content.as_bytes()) {
        Ok(_) => Ok(()),
        Err(err) => Err(node_error_message("open", path, &err)),
    }
}

/// Node's `fsPromises.mkdir(dir, { recursive: true })`.
pub fn node_mkdir_recursive(dir: &str) -> Result<(), String> {
    match std::fs::create_dir_all(dir) {
        Ok(_) => Ok(()),
        Err(err) => Err(node_error_message("mkdir", dir, &err)),
    }
}

/// Node's `fsPromises.stat` (directory test). Err carries the Node message.
pub fn node_stat_is_dir(path: &str) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(md) => Ok(md.is_dir()),
        Err(err) => Err(node_error_message("stat", path, &err)),
    }
}

/// TS `getFileRevision` (coding-agent `utils/paths.ts` l36):
/// `` `${dev}:${ino}:${size}:${mtimeNs}:${ctimeNs}` `` with bigint stat
/// values (mtimeNs/ctimeNs are total nanoseconds). Failure → `None`
/// (not an error).
#[cfg(unix)]
pub fn file_revision(path: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ns = meta.mtime() as i128 * 1_000_000_000 + meta.mtime_nsec() as i128;
    let ctime_ns = meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128;
    Some(format!(
        "{}:{}:{}:{}:{}",
        meta.dev(),
        meta.ino(),
        meta.len(),
        mtime_ns,
        ctime_ns
    ))
}

#[cfg(not(unix))]
pub fn file_revision(_path: &str) -> Option<String> {
    None
}

/// Node's `fsPromises.readdir` (names only).
pub fn node_readdir(path: &str) -> Result<Vec<String>, String> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut names = Vec::new();
            for entry in entries {
                match entry {
                    Ok(e) => names.push(e.file_name().to_string_lossy().into_owned()),
                    Err(err) => return Err(node_error_message("scandir", path, &err)),
                }
            }
            Ok(names)
        }
        Err(err) => Err(node_error_message("scandir", path, &err)),
    }
}
