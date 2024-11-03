use gdnative::api::{AudioStreamPlayer, AudioStreamPlayer3D};
use gdnative::prelude::*;

use crate::stream_errors::StreamError;

#[derive(Debug, Clone)]
pub enum AudioPlayer {
    Player2D(Ref<AudioStreamPlayer>),
    Player3D(Ref<AudioStreamPlayer3D>),
}

impl AudioPlayer {
    pub fn play(&self) -> Result<(), StreamError> {
        match self {
            AudioPlayer::Player2D(player) => {
                let player = unsafe { player.assume_safe() };
                player.play(0.0);
            }
            AudioPlayer::Player3D(player) => {
                let player = unsafe { player.assume_safe() };
                player.play(0.0);
            }
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<(), StreamError> {
        match self {
            AudioPlayer::Player2D(player) => {
                let player = unsafe { player.assume_safe() };
                player.stop();
            }
            AudioPlayer::Player3D(player) => {
                let player = unsafe { player.assume_safe() };
                player.stop();
            }
        }
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        match self {
            AudioPlayer::Player2D(player) => {
                let player = unsafe { player.assume_safe() };
                player.is_playing()
            }
            AudioPlayer::Player3D(player) => {
                let player = unsafe { player.assume_safe() };
                player.is_playing()
            }
        }
    }
}
