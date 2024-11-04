use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Root path not set")]
    RootPathNotSet,

    #[error("Process error: {0}")]
    ProcessError(String),

    #[error("Failed to download yt-dlp")]
    YtDlpDownloadError(String),

    #[error("Failed to download ffmpeg")]
    FfmpegDownloadError(String),

    #[error("Invalid player")]
    InvalidPlayer,
}
