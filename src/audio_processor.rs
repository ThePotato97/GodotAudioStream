use std::collections::VecDeque;

use crate::{audio_player::AudioPlayer, stream_errors::StreamError};
use gdnative::{api::AudioStreamGeneratorPlayback, prelude::*};
use std::time::{Duration, Instant};

pub struct AudioProcessor {
    pub buffer: VecDeque<f32>,
    buffer_size: i32,
    pub samples_processed: usize,
    last_process_time: Instant,
    process_interval: Duration,
    pub buffer_threshold: usize,
}

impl AudioProcessor {
    pub fn new(buffer_size: i32, buffer_threshold: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            buffer_size,
            samples_processed: 0,
            buffer_threshold,
            last_process_time: Instant::now(),
            process_interval: Duration::from_micros(5000),
        }
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.samples_processed = 0;
    }

    pub fn process_samples(
        &mut self,
        playback: &Ref<AudioStreamGeneratorPlayback>,
        player: &AudioPlayer,
    ) -> Result<bool, StreamError> {
        let playback = unsafe { playback.assume_safe() };

        let now = Instant::now();

        if now.duration_since(self.last_process_time) < self.process_interval {
            return Ok(false);
        }
        self.last_process_time = now;

        if !player.is_playing() || self.buffer.is_empty() {
            return Ok(false);
        }

        let frames_available = playback.get_frames_available() as i32;

        if frames_available == 0 {
            return Ok(false);
        }

        // Calculate how many samples we can process
        let max_samples = std::cmp::min(
            self.buffer_size,
            frames_available * 2, // Multiply by 2 for stereo
        );

        let samples_to_process = std::cmp::min(
            max_samples as usize,
            self.buffer.len() - (self.buffer.len() % 2), // Ensure even number for stereo pairs
        );

        if samples_to_process == 0 {
            return Ok(false);
        }

        let mut frames = PoolArray::<Vector2>::new();

        // Process samples in chunks
        for _ in (0..samples_to_process).step_by(2) {
            self.samples_processed += 1;
            let left = self.buffer.pop_front().unwrap_or(0.0);
            let right = self.buffer.pop_front().unwrap_or(left);
            frames.push(Vector2::new(left, right));
        }

        playback.push_buffer(frames);

        Ok(true)
    }
}
