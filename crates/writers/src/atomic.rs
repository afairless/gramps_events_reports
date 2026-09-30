//! Atomic temp-file → rename write machinery (plan §7.4).
//!
//! Every writer renders its payload into a uniquely named temp file *in
//! the destination directory*, then renames it over the destination only
//! after the payload is fully written and flushed. An aborted or failed
//! export therefore never leaves a partial file at the final path — POSIX
//! `rename(2)` over an existing destination is atomic. On failure the temp
//! file is removed and the error propagated; a pre-existing destination is
//! left untouched.

use std::fs::{self, File};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::WriterError;

/// Per-process temp-name counter. Combined with the PID in the temp file
/// name it makes the path unique among concurrent writers in this process
/// (and across processes with different PIDs); a stale temp file from a
/// crashed run is simply unused.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `dest` atomically: run `write` against a temp file sitting next
/// to `dest`, then rename it over `dest` on success. On error the temp
/// file is removed and the error returned; an existing `dest` is preserved.
pub(crate) fn write_atomically(
    dest: &Path,
    write: impl FnOnce(File) -> Result<(), WriterError>,
) -> Result<(), WriterError> {
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let file_name = dest.file_name().ok_or_else(|| {
        WriterError::Other(format!(
            "destination `{}` does not name a file",
            dest.display()
        ))
    })?;
    let temp_path = parent.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    // create_new refuses to clobber an existing (stale) temp file, which
    // is exactly what we want for a unique-name collision.
    let file = File::create_new(&temp_path).map_err(|e| WriterError::io(&temp_path, e))?;

    match write(file) {
        Ok(()) => fs::rename(&temp_path, dest).map_err(|e| WriterError::io(dest, e)),
        Err(err) => {
            // Best-effort cleanup; the primary failure is the write error.
            let _ = fs::remove_file(&temp_path);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::write_atomically;
    use crate::error::WriterError;
    use std::fs;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn dest(td: &TempDir) -> PathBuf {
        td.path().join("out.csv")
    }

    fn temp_leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn success_creates_dest_and_leaves_no_temp_file() {
        let td = TempDir::new().unwrap();
        let d = dest(&td);
        write_atomically(&d, |file| {
            let mut file = file;
            file.write_all(b"hello").unwrap();
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(&d).unwrap(), b"hello");
        assert!(
            temp_leftovers(td.path()).is_empty(),
            "temp files left behind"
        );
    }

    #[test]
    fn failure_removes_temp_and_preserves_existing_dest() {
        let td = TempDir::new().unwrap();
        let d = dest(&td);
        fs::write(&d, b"old").unwrap();
        let err =
            write_atomically(&d, |_file| Err(WriterError::Other("boom".to_string()))).unwrap_err();
        assert_eq!(err.to_string(), "boom");
        assert_eq!(
            fs::read(&d).unwrap(),
            b"old",
            "destination must survive a failed write"
        );
        assert!(
            temp_leftovers(td.path()).is_empty(),
            "temp file must be cleaned up"
        );
    }

    #[test]
    fn failure_removes_temp_when_no_dest_existed() {
        let td = TempDir::new().unwrap();
        let d = dest(&td);
        let _ = write_atomically(&d, |_file| Err(WriterError::Other("boom".to_string())));
        assert!(!d.exists());
        assert!(temp_leftovers(td.path()).is_empty());
    }

    #[test]
    fn second_write_replaces_the_first() {
        let td = TempDir::new().unwrap();
        let d = dest(&td);
        write_atomically(&d, |file| {
            let mut file = file;
            file.write_all(b"one").unwrap();
            Ok(())
        })
        .unwrap();
        write_atomically(&d, |file| {
            let mut file = file;
            file.write_all(b"two").unwrap();
            Ok(())
        })
        .unwrap();
        // rename(2) is atomic-over-replace on POSIX: only the new content
        // is ever visible at the destination, and no temp file lingers.
        assert_eq!(fs::read(&d).unwrap(), b"two");
        assert!(temp_leftovers(td.path()).is_empty());
    }

    #[test]
    fn missing_parent_directory_errors_and_leaves_nothing() {
        let td = TempDir::new().unwrap();
        let d = td.path().join("no/such/dir/out.csv");
        let err = write_atomically(&d, |_file| Ok(())).unwrap_err();
        assert!(
            matches!(err, WriterError::Io { .. }),
            "expected an io error, got {err:?}"
        );
        assert!(!d.exists());
    }

    #[test]
    fn destination_without_file_name_is_rejected() {
        let err = write_atomically(Path::new("/"), |_file| Ok(())).unwrap_err();
        assert!(matches!(err, WriterError::Other(_)), "got {err:?}");
    }

    #[test]
    fn bare_file_name_writes_into_the_current_directory() {
        let unique = format!("writers-atomic-{}.csv", std::process::id());
        let path = Path::new(&unique);
        write_atomically(path, |file| {
            let mut file = file;
            file.write_all(b"cwd").unwrap();
            Ok(())
        })
        .unwrap();
        assert!(
            fs::remove_file(path).is_ok(),
            "temp artifact must be ours to remove"
        );
    }
}
