mod audio_player;
mod audio_processor;
mod checksum;
mod download_ffmpeg;
mod download_yt_dlp;
mod process_manager;
mod stream_errors;

use audio_player::AudioPlayer;
use audio_processor::AudioProcessor;
use gdnative::api::AudioStreamGeneratorPlayback;
use gdnative::prelude::*;

use process_manager::{ProcessManager, YtDlpMetadata};
use std::backtrace::Backtrace;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use stream_errors::StreamError;
use url::Url;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const SAMPLE_RATE: u32 = 44100;
const CHANNELS: u32 = 2;
const DEFAULT_BUFFER_SIZE: i32 = 4096;
#[allow(clippy::identity_op)]
const DEFAULT_BUFFER_THRESHOLD: usize = SAMPLE_RATE as usize * 1; // 1 seconds

#[derive(NativeClass)]
#[inherit(Node)]
struct YTStream {
    playback: Arc<Mutex<Option<Ref<AudioStreamGeneratorPlayback>>>>,
    player: Arc<Mutex<Option<AudioPlayer>>>,
    audio_processor: Arc<Mutex<AudioProcessor>>,
    process_manager: Option<ProcessManager>,
    audio_rx: Option<Receiver<Vec<f32>>>,
    ffmpeg_tx: Option<Sender<Vec<u8>>>,
    current_url: Option<String>,
    is_paused: Arc<AtomicBool>,
    processing_thread: Option<JoinHandle<()>>,
    should_stop: Arc<AtomicBool>,
}

#[methods]
impl YTStream {
    fn new(_owner: &Node) -> Self {
        Self {
            playback: Arc::new(Mutex::new(None)),
            player: Arc::new(Mutex::new(None)),
            audio_processor: Arc::new(Mutex::new(AudioProcessor::new(
                DEFAULT_BUFFER_SIZE,
                DEFAULT_BUFFER_THRESHOLD,
            ))),
            process_manager: None,
            audio_rx: None,
            ffmpeg_tx: None,
            current_url: None,
            processing_thread: None,
            should_stop: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
        }
    }

    #[method]
    fn set_player(&mut self, #[base] _owner: &Node, player: AudioPlayer) -> bool {
        let mut player_lock = self.player.lock().unwrap();
        if let Some(ref current_player) = *player_lock {
            if let Err(e) = current_player.stop() {
                godot_error!("Failed to stop current player: {}", e);
                return false;
            }
        }
        *player_lock = Some(player);
        true
    }

    #[method]
    fn set_audio_stream(&mut self, playback: Ref<AudioStreamGeneratorPlayback>) {
        let mut playback_lock = self.playback.lock().unwrap();
        *playback_lock = Some(playback);
    }

    #[method]
    fn set_root_path(&mut self, path: GodotString) -> bool {
        let path_string = path.to_string();
        let path_buf = PathBuf::from(&path_string);

        self.process_manager = Some(ProcessManager::new(path_buf.clone()));

        self.process_manager
            .as_mut()
            .unwrap()
            .set_root_path(path_buf.clone());

        if let Err(e) = self
            .process_manager
            .as_mut()
            .unwrap()
            .download_dependencies(path_buf)
        {
            godot_error!("Failed to download dependencies: {}", e);
            return false;
        }
        true
    }

    #[method]
    fn play_youtube_audio(&mut self, url: GodotString) -> bool {
        // Stop existing processing thread if any
        self.stop_processing_thread();

        match self.play_youtube_audio_internal(&url.to_string()) {
            Ok(_) => {
                // Start new processing thread
                self.should_stop.store(false, Ordering::Relaxed);
                self.start_processing_thread();
                true
            }
            Err(e) => {
                godot_error!("Failed to play YouTube audio: {}", e);
                false
            }
        }
    }

    #[method]
    fn get_playback_position(&self) -> f64 {
        return self.audio_processor.lock().unwrap().samples_processed as f64 / SAMPLE_RATE as f64;
    }

    #[method]
    fn get_current_playback_metadata(&self) -> Option<YtDlpMetadata> {
        let metadata = self.process_manager.as_ref().unwrap();
        if let Ok(meta_lock) = metadata.ytdlp_metadata.lock() {
            return meta_lock.clone();
        }
        None
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

        // TODO: Re-enable this once we switch to async tokio

        // // get metadata and make sure it's valid before starting playback
        // let metadata = process_manager
        //     .get_ytdlp_metadata(url.as_str())
        //     .map_err(|e| {
        //         godot_print!("Failed to get metadata: {}", e);
        //         StreamError::InvalidUrl(e.to_string())
        //     })?;

        // self.current_playback_metadata = Some(metadata.clone());

        process_manager.get_ytdlp_metadata(url.as_str());

        if !process_manager.is_initialized() {
            godot_print!("Waiting for dependencies to initialize...");
            return Ok(());
        }
        // Start new playback
        let (ffmpeg_tx, audio_rx) = process_manager.start_ffmpeg()?;
        process_manager.start_ytdlp(url.as_str(), ffmpeg_tx.clone())?;

        self.ffmpeg_tx = Some(ffmpeg_tx);
        self.audio_rx = Some(audio_rx);
        self.current_url = Some(url.to_string());

        Ok(())
    }

    #[method]
    fn stop(&mut self, #[base] _owner: &Node) -> bool {
        self.cleanup();
        true
    }

    fn stop_processing_thread(&mut self) {
        if let Some(handle) = self.processing_thread.take() {
            self.should_stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        }
    }

    fn start_processing_thread(&mut self) {
        let playback = Arc::clone(&self.playback);
        let player = Arc::clone(&self.player);
        let audio_processor = Arc::clone(&self.audio_processor);
        let audio_rx = self.audio_rx.take();
        let should_stop = Arc::clone(&self.should_stop);
        let is_paused = Arc::clone(&self.is_paused);

        self.processing_thread = Some(thread::spawn(move || loop {
            if is_paused.load(Ordering::Relaxed) {
                thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            if should_stop.load(Ordering::Relaxed) {
                break;
            }

            if let Some(ref rx) = audio_rx {
                let mut processor = audio_processor.lock().unwrap();
                let playback_lock = playback.lock().unwrap();
                let player_lock = player.lock().unwrap();

                if let (Some(playback), Some(player)) =
                    (playback_lock.as_ref(), player_lock.as_ref())
                {
                    if !player.is_safe() {
                        break;
                    }

                    // Fill buffer
                    while let Ok(samples) = rx.try_recv() {
                        processor.buffer.extend(samples);
                    }

                    // Start playback if buffer is full enough
                    if !player.is_playing() && processor.buffer.len() >= processor.buffer_threshold
                    {
                        let _ = player.play();
                    }

                    // Process audio
                    if let Err(e) = processor.process_samples(playback, player) {
                        godot_error!("Audio processing error: {}", e);
                    }
                }
            }

            thread::sleep(std::time::Duration::from_millis(1));
        }));
    }

    fn cleanup(&mut self) {
        self.stop_processing_thread();

        if let Some(process_manager) = &mut self.process_manager {
            process_manager.cleanup();
        }

        if let Some(player) = &*self.player.lock().unwrap() {
            let _ = player.stop();
        }

        if let Ok(mut processor) = self.audio_processor.lock() {
            processor.clear_buffer();
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

pub fn init_panic_hook() {
    // To enable backtrace, you will need the `backtrace` crate to be included in your cargo.toml, or
    // a version of Rust where backtrace is included in the standard library (e.g. Rust nightly as of the date of publishing)
    // use backtrace::Backtrace;
    // use std::backtrace::Backtrace;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let loc_string;
        if let Some(location) = panic_info.location() {
            loc_string = format!("file '{}' at line {}", location.file(), location.line());
        } else {
            loc_string = "unknown location".to_owned()
        }

        let error_message;
        if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            error_message = format!("[RUST] {}: panic occurred: {:?}", loc_string, s);
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            error_message = format!("[RUST] {}: panic occurred: {:?}", loc_string, s);
        } else {
            error_message = format!("[RUST] {}: unknown panic occurred", loc_string);
        }
        godot_error!("{}", error_message);
        // Uncomment the following line if backtrace crate is included as a dependency
        godot_error!("Backtrace:\n{:?}", Backtrace::capture());

        (*(old_hook.as_ref()))(panic_info);

        unsafe {
            if let Some(gd_panic_hook) =
                gdnative::api::utils::autoload::<gdnative::api::Node>("rust_panic_hook")
            {
                gd_panic_hook.call(
                    "rust_panic_hook",
                    &[GodotString::from_str(error_message).to_variant()],
                );
            }
        }
    }));
}

fn init(handle: InitHandle) {
    handle.add_class::<YTStream>();
    init_panic_hook();
}

godot_init!(init);
