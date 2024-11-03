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

impl FromVariant for AudioPlayer {
    fn from_variant(variant: &Variant) -> Result<Self, FromVariantError> {
        // Check if the variant is an AudioStreamPlayer
        if let Ok(player) = variant.try_to_object::<AudioStreamPlayer>() {
            return Ok(AudioPlayer::Player2D(player));
        }

        // Then try to convert to AudioStreamPlayer3D
        if let Ok(player) = variant.try_to_object::<AudioStreamPlayer3D>() {
            return Ok(AudioPlayer::Player3D(player));
        }

        // If neither conversion worked, return an error
        Err(FromVariantError::Custom(String::from(
            "Variant is not an AudioStreamPlayer or AudioStreamPlayer3D",
        )))
    }
}

impl ToVariant for AudioPlayer {
    fn to_variant(&self) -> Variant {
        match self {
            AudioPlayer::Player2D(player) => player.to_variant(),
            AudioPlayer::Player3D(player) => player.to_variant(),
        }
    }
}
