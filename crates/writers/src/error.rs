//! The [`WriterError`] type shared by every row writer.

use std::path::Path;

/// Errors raised by the row writers (plan §7.4).
///
/// I/O failures carry the path they occurred on so an export abort can be
/// diagnosed without re-deriving it from the call site; the serde and
/// arrow/parquet families convert automatically via `?`.
#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    /// An I/O failure while creating the temp file, flushing it, renaming
    /// it over the destination, or cleaning up after a failed write.
    #[error("io error for {path}: {source}")]
    Io {
        /// The file the failing operation concerned.
        path: String,
        /// The underlying OS error.
        source: std::io::Error,
    },

    /// CSV serialization failure (writing) or deserialization failure
    /// (parse-back in tests).
    #[error("csv error: {source}")]
    Csv {
        /// The csv crate's error.
        #[from]
        source: csv::Error,
    },

    /// JSON serialization failure.
    #[error("json error: {source}")]
    Json {
        /// The serde_json error.
        #[from]
        source: serde_json::Error,
    },

    /// Arrow array / record-batch construction failure, or a Parquet
    /// reader error surfaced through the arrow crate's error type.
    #[error("arrow error: {source}")]
    Arrow {
        /// The arrow crate's error.
        #[from]
        source: arrow::error::ArrowError,
    },

    /// Parquet writer / properties error.
    #[error("parquet error: {source}")]
    Parquet {
        /// The parquet crate's error.
        #[from]
        source: parquet::errors::ParquetError,
    },

    /// A destination path that cannot name a file (e.g. `/`), or any
    /// other writer-level failure without a more specific category.
    #[error("{0}")]
    Other(String),
}

impl WriterError {
    /// Wrap an I/O error with the path the failed operation concerned.
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WriterError;
    use std::io;

    #[test]
    fn io_variant_carries_the_path() {
        let err = WriterError::io(
            std::path::Path::new("/tmp/out.csv"),
            io::Error::other("boom"),
        );
        let msg = err.to_string();
        assert!(msg.contains("/tmp/out.csv"), "path in message: {msg}");
        assert!(msg.contains("boom"), "source in message: {msg}");
    }

    #[test]
    fn other_variant_is_a_plain_message() {
        let err = WriterError::Other("no file name".to_string());
        assert_eq!(err.to_string(), "no file name");
    }
}
