mod audio_player;
mod audio_processor;
mod download_ffmpeg;
mod download_yt_dlp;
mod process_manager;
mod stream_errors;

use audio_player::AudioPlayer;
use audio_processor::AudioProcessor;
use gdnative::api::{AudioStreamGeneratorPlayback, AudioStreamPlayer, AudioStreamPlayer3D};
use gdnative::prelude::*;
use process_manager::ProcessManager;
use stream_errors::StreamError;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use url::Url;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const SAMPLE_RATE: u32 = 44100;
const CHANNELS: u32 = 2;
const DEFAULT_BUFFER_SIZE: i32 = 4096;
const DEFAULT_BUFFER_THRESHOLD: usize = SAMPLE_RATE as usize * 5; // 5 seconds

const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.7z";
const YT_DLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

#[derive(NativeClass)]
#[inherit(Node)]
struct YTStream {
    playback: Option<Ref<AudioStreamGeneratorPlayback>>,
    player: Option<AudioPlayer>,
    audio_processor: AudioProcessor,
    process_manager: Option<ProcessManager>,
    audio_rx: Option<Receiver<Vec<f32>>>,
    ffmpeg_tx: Option<Sender<Vec<u8>>>,
    current_url: Option<String>,
    is_paused: Arc<AtomicBool>,
}

#[methods]
impl YTStream {
    fn new(_owner: &Node) -> Self {
        Self {
            playback: None,
            player: None,
            audio_processor: AudioProcessor::new(DEFAULT_BUFFER_SIZE, DEFAULT_BUFFER_THRESHOLD),
            process_manager: None,
            audio_rx: None,
            ffmpeg_tx: None,
            current_url: None,
            is_paused: Arc::new(AtomicBool::new(false)),
        }
    }

    #[method]
    fn set_player(&mut self, #[base] _owner: &Node, player: AudioPlayer) -> bool {
        // Stop any existing playback
        if let Some(ref current_player) = &self.player {
            if let Err(e) = current_player.stop() {
                godot_error!("Failed to stop current player: {}", e);
                return false;
            }
        }

        // Set the new player
        self.player = Some(player);
        true // Successfully set the player
    }

    #[method]
    fn set_audio_stream(&mut self, playback: Ref<AudioStreamGeneratorPlayback>) {
        self.playback = Some(playback);
    }

    #[method]
    fn set_root_path(&mut self, path: GodotString) -> bool {
        let path = PathBuf::from(path.to_string());
        if !path.exists() {
            godot_error!("Root path does not exist: {:?}", path);
            return false;
        }

        self.process_manager = Some(ProcessManager::new(path));
        true
    }

    #[method]
    fn play_youtube_audio(&mut self, url: String) -> bool {
        match self.play_youtube_audio_internal(&url) {
            Ok(_) => true,
            Err(e) => {
                godot_error!("Failed to play YouTube audio: {}", e);
                false
            }
        }
    }

    fn play_youtube_audio_internal(&mut self, url: &str) -> Result<(), StreamError> {
        // Validate URL
        let url = Url::parse(url).map_err(|_| StreamError::InvalidUrl(url.to_string()))?;

        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(StreamError::InvalidUrl("Invalid URL scheme".to_string()));
        }

        self.cleanup();

        // Ensure we have required components
        let process_manager = self
            .process_manager
            .as_mut()
            .ok_or(StreamError::RootPathNotSet)?;

        // Stop any existing playback

        // Start new playback
        let (ffmpeg_tx, audio_rx) = process_manager.start_ffmpeg()?;
        process_manager.start_ytdlp(url.as_str(), ffmpeg_tx.clone())?;

        self.ffmpeg_tx = Some(ffmpeg_tx);
        self.audio_rx = Some(audio_rx);
        self.current_url = Some(url.to_string());

        Ok(())
    }

    #[method]
    fn _process(&mut self, _delta: f64) -> bool {
        if let Err(e) = self.process_internal() {
            godot_error!("Process error: {}", e);
            return false;
        }
        true
    }

    fn process_internal(&mut self) -> Result<(), StreamError> {
        let playback: &Ref<AudioStreamGeneratorPlayback> = self
            .playback
            .as_ref()
            .ok_or(StreamError::AudioStreamGeneratorPlaybackNotInitialized)?;

        let player: &AudioPlayer = self
            .player
            .as_ref()
            .ok_or(StreamError::AudioPlayerNotInitialized)?;

        if let Some(rx) = &self.audio_rx {
            // Fill buffer
            while let Ok(samples) = rx.try_recv() {
                self.audio_processor.buffer.extend(samples);
            }

            // Start playback if buffer is full enough
            if !player.is_playing()
                && self.audio_processor.buffer.len() >= self.audio_processor.buffer_threshold
            {
                player.play()?;
            }

            // Process audio
            self.audio_processor.process_samples(playback, player)?;
        }

        Ok(())
    }

    fn cleanup(&mut self) {
        if let Some(process_manager) = &mut self.process_manager {
            process_manager.cleanup();
        }

        if let Some(player) = &self.player {
            let _ = player.stop();
        }

        self.audio_processor.buffer.clear();

        // clear playback buffer
        if let Some(playback) = &self.playback {
            unsafe {
                playback.assume_safe().clear_buffer();
            }
        }

        // Drain audio receiver
        if let Some(rx) = &self.audio_rx {
            while rx.try_recv().is_ok() {}
        }
        self.current_url = None;
        self.is_paused.store(false, Ordering::Relaxed);
    }

    // TODO: Add other necessary methods (pause, resume, etc.)
}

fn init(handle: InitHandle) {
    handle.add_class::<YTStream>();
}

godot_init!(init);
