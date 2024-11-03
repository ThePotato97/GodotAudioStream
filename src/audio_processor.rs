use std::collections::VecDeque;

use gdnative::{api::AudioStreamGeneratorPlayback, prelude::*};

use crate::{audio_player::AudioPlayer, stream_errors::StreamError};

pub struct AudioProcessor {
    pub buffer: VecDeque<f32>,
    buffer_size: i32,
    pub buffer_threshold: usize,
}

impl AudioProcessor {
    pub fn new(buffer_size: i32, buffer_threshold: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            buffer_size,
            buffer_threshold,
        }
    }

    pub fn process_samples(
        &mut self,
        playback: &Ref<AudioStreamGeneratorPlayback>,
        player: &AudioPlayer,
    ) -> Result<(), StreamError> {
        let playback = unsafe { playback.assume_safe() };

        while player.is_playing() && !self.buffer.is_empty() {
            let frames_available = playback.get_frames_available() as i32; // Convert to i32

            if frames_available == 0 {
                // If the buffer is full, wait and try again in the next cycle
                return Ok(());
            }

            let mut frames = PoolArray::<Vector2>::new();
            let mut samples_consumed = 0;

            while samples_consumed < self.buffer_size
                && (samples_consumed / 2) < frames_available
                && !self.buffer.is_empty()
            {
                let left = self.buffer.pop_front().unwrap_or(0.0);
                let right = self.buffer.pop_front().unwrap_or(left);
                frames.push(Vector2::new(left, right));
                samples_consumed += 2;
            }

            playback.push_buffer(frames);
        }

        Ok(())
    }
}
