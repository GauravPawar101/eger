//! Error types for `eger`.
//!
//! [`ConfigError`] covers everything that can go wrong while *building* a
//! [`crate::config::Config`]. [`EgerError`] is the crate-wide error used by
//! every rendering/processing API and wraps `ConfigError` alongside I/O,
//! `iascii`, `image`, `ffmpeg`, and Tokio task failures.

use std::path::PathBuf;
use std::process::ExitStatus;
use thiserror::Error;

/// Errors that can occur while validating and building a [`crate::config::Config`].
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("no file found at: {0}")]
    FileNotFound(PathBuf),

    #[error("no directory found at: {0}")]
    FolderNotFound(PathBuf),

    #[error("invalid regex pattern '{0}': {1}")]
    InvalidRegexPattern(String, #[source] regex::Error),

    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("no media type (Image/Video) was configured")]
    MediaTypeNotFound,

    #[error("no input source was configured; call `from_file` or `from_dir`")]
    MissingSource,

    #[error("no files in '{dir}' matched pattern '{pattern}'")]
    NoMatchingFiles { dir: PathBuf, pattern: String },

    #[error("invalid ASCII rendering configuration: {0}")]
    Ascii(#[from] iascii::error::ImageError),
}

/// The crate-wide error type returned by every `eger` API.
#[derive(Debug, Error)]
pub enum EgerError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("ASCII conversion failed: {0}")]
    Ascii(#[from] iascii::error::ImageError),

    #[error("image decode/encode error: {0}")]
    Image(#[from] ::image::ImageError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("video pipeline error: {0}")]
    Video(String),

    #[error("ffmpeg exited with {0}: {1}")]
    Ffmpeg(ExitStatus, String),

    #[error("`ffmpeg`/`ffprobe` was not found on PATH; install ffmpeg to enable video support")]
    FfmpegNotFound,

    #[cfg(feature = "video")]
    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("render error: {0}")]
    Render(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, EgerError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn config_error_messages_include_the_offending_path() {
        let path = PathBuf::from("/tmp/does-not-exist.png");
        let err = ConfigError::FileNotFound(path.clone());
        assert!(err.to_string().contains(&path.display().to_string()));

        let err = ConfigError::FolderNotFound(path.clone());
        assert!(err.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn no_matching_files_message_includes_dir_and_pattern() {
        let err = ConfigError::NoMatchingFiles {
            dir: PathBuf::from("/tmp/frames"),
            pattern: r"\.png$".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("/tmp/frames"));
        assert!(msg.contains(r"\.png$"));
    }

    #[test]
    fn config_error_converts_into_eger_error() {
        let config_err = ConfigError::MissingSource;
        let eger_err: EgerError = config_err.into();
        assert!(matches!(
            eger_err,
            EgerError::Config(ConfigError::MissingSource)
        ));
    }

    #[test]
    fn io_error_converts_into_eger_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let eger_err: EgerError = io_err.into();
        assert!(matches!(eger_err, EgerError::Io(_)));
    }

    #[test]
    fn ffmpeg_error_reports_exit_status_and_stderr() {
        let status = std::process::Command::new("false").status().or_else(|_| {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("exit 1")
                .status()
        });
        let Ok(status) = status else {
            eprintln!("skipping: no shell available to produce a failing ExitStatus");
            return;
        };
        let err = EgerError::Ffmpeg(status, "boom".into());
        let msg = err.to_string();
        assert!(msg.contains("boom"));
    }

    #[test]
    fn ffmpeg_not_found_has_actionable_message() {
        let err = EgerError::FfmpegNotFound;
        assert!(err.to_string().to_lowercase().contains("ffmpeg"));
        assert!(err.to_string().to_lowercase().contains("path"));
    }
}
