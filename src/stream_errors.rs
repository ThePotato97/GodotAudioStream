use std::io;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StreamError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("AudioPlayer not initialized")]
    AudioPlayerNotInitialized,

    #[error("AudioStreamGeneratorPlayback not initialized")]
    AudioStreamGeneratorPlaybackNotInitialized,

    #[error("Root path not set")]
    RootPathNotSet,

    #[error("Process error: {0}")]
    ProcessError(String),

    #[error("Channel error: {0}")]
    ChannelError(String),
}
