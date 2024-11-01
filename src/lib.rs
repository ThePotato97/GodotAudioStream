use gdnative::api::{AudioStreamGeneratorPlayback, AudioStreamPlayer};
use gdnative::prelude::*;
use std::collections::VecDeque;
use std::f32::consts::E;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use url::Url;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(NativeClass)]
#[inherit(Node)]
struct YTStream {
    playback: Option<Ref<AudioStreamGeneratorPlayback>>,
    player: Option<Ref<AudioStreamPlayer>>,
    audio_data_rx: Option<Receiver<Vec<f32>>>,
    ffmpeg_tx: Option<Sender<Vec<u8>>>,
    audio_buffer: VecDeque<f32>,
    buffer_size: i32,
    buffer_threshold: usize,
    current_url: Option<String>, // Store the current URL
    is_paused: Arc<AtomicBool>,
}

fn sanitize_url(input: &str) -> Result<String, &'static str> {
    // Attempt to parse the URL
    match Url::parse(input) {
        Ok(url) => {
            // You can further validate the scheme if needed
            if url.scheme() == "http" || url.scheme() == "https" {
                // Return the URL as a string
                Ok(url.to_string())
            } else {
                Err("Invalid URL scheme")
            }
        }
        Err(_) => Err("Invalid URL"),
    }
}

#[methods]
impl YTStream {
    fn new(_owner: &Node) -> Self {
        godot_print!("YTStream new");
        YTStream {
            playback: None,
            player: None,
            buffer_size: 4096,
            audio_data_rx: None,
            ffmpeg_tx: None,
            audio_buffer: VecDeque::new(),
            buffer_threshold: 44100 * 5, // Buffer 5 seconds of audio
            current_url: None,
            is_paused: Arc::new(AtomicBool::new(false)),
        }
    }
    #[method]
    fn set_audio_stream(
        &mut self,
        #[base] _owner: &Node,
        stream: Ref<AudioStreamGeneratorPlayback>,
    ) {
        self.playback = Some(stream)
    }

    #[method]
    fn set_audio_player(&mut self, #[base] _owner: &Node, stream: Ref<AudioStreamPlayer>) {
        self.player = Some(stream)
    }

    #[method]
    fn _ready(&mut self, #[base] _owner: &Node) {
        godot_print!("YTStream ready");
        self._start_ffmpeg();
    }

    #[method]
    fn pause(&mut self, #[base] _owner: &Node) {
        self.is_paused.store(true, Ordering::Relaxed);
        if let Some(player) = self.player.as_ref() {
            unsafe {
                player.assume_safe().stop();
            }
        }
    }

    #[method]
    fn resume(&mut self, #[base] _owner: &Node) {
        self.is_paused.store(false, Ordering::Relaxed);
        if let Some(player) = self.player.as_ref() {
            unsafe {
                player.assume_safe().play(0.0);
            }
        }
    }

    // #[method]
    // fn stop(&mut self, #[base] _owner: &Node) {
    //     //Kill Child processes
    //     // this is arc and mutex, so we need to lock it
    //     if let Some(mut yt_dlp) = self.yt_dlp.lock().unwrap().take() {
    //         let _ = yt_dlp.kill(); // attempt to kill the process
    //         let _ = yt_dlp.wait(); //  wait for it to exit.
    //     }
    //     // this is arc and mutex, so we need to lock it
    //     if let Some(mut ffmpeg) = self.ffmpeg.lock().unwrap().take() {
    //         let _ = ffmpeg.kill(); // attempt to kill the process
    //         let _ = ffmpeg.wait(); //  wait for it to exit.
    //     }

    //     self.is_paused.store(false, Ordering::Relaxed);
    //     self.audio_data_rx = None;
    //     self.audio_buffer.clear();
    //     self.current_url = None;

    //     if let Some(player) = self.player.as_ref() {
    //         unsafe {
    //             player.assume_safe().stop();
    //         }
    //     }
    // }

    #[method]
    fn _start_ffmpeg(&mut self) {
        let (tx, rx) = channel::<Vec<f32>>();
        self.audio_data_rx = Some(rx);

        let (ffmpeg_tx, ffmpeg_rx) = channel::<Vec<u8>>();
        self.ffmpeg_tx = Some(ffmpeg_tx);

        let is_paused_clone = Arc::clone(&self.is_paused);

        godot_print!("Starting ffmpeg");
        thread::spawn(move || {
            let mut ffmpeg = Command::new("ffmpeg")
                .args([
                    "-i",
                    "pipe:0",
                    "-f",
                    "f32le", // 32-bit float PCM
                    "-acodec",
                    "pcm_f32le",
                    "-ar",
                    "44100", // Sample rate
                    "-ac",
                    "2", // Stereo
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW) // CREATE_NO_WINDOW
                .spawn()
                .expect("Failed to start ffmpeg");

            let mut ffmpeg_stdout = ffmpeg.stdout.take().unwrap();
            let mut ffmpeg_stdin = ffmpeg.stdin.take().unwrap();

            // Read PCM data from ffmpeg
            let mut buffer = vec![0u8; 4096 * 4]; // Space for 4096 f32 samples

            thread::spawn(move || loop {
                if is_paused_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(10)); // Check pause state periodically
                    continue;
                }

                if let Ok(raw_audio) = ffmpeg_rx.recv() {
                    godot_print!("Received {} bytes from ffmpeg_rx", raw_audio.len());
                    if ffmpeg_stdin.write_all(&raw_audio).is_err() {
                        godot_error!("Failed to write to ffmpeg");
                        break;
                    }
                }
            });

            thread::spawn(move || loop {
                match ffmpeg_stdout.read(&mut buffer) {
                    Ok(n) if n == 0 => {
                        godot_print!("ffmpeg_stdout.read() returned 0");
                    }
                    Ok(n) => {
                        // Convert bytes to f32
                        let samples = unsafe {
                            std::slice::from_raw_parts(buffer.as_ptr() as *const f32, n / 4)
                                .to_vec()
                        };

                        if tx.send(samples).is_err() {
                            // Handle send errors (e.g., receiver disconnected)
                            godot_error!("Error sending samples to audio thread");
                            break;
                        }
                    }
                    Err(e) => {
                        godot_error!("Error reading from ffmpeg: {}", e); // Handle errors!
                    }
                }
            });
        });
    }

    #[method]
    fn play_youtube_audio(&mut self, #[base] _owner: &Node, url: String) -> bool {
        if self.playback.is_none() {
            godot_print!("AudioStreamGeneratorPlayback not initialized");
            return false;
        }

        let sanitized_url = match sanitize_url(&url) {
            Ok(url) => url,
            Err(err) => {
                godot_error!("Invalid URL: {}", err);
                return false;
            }
        };

        self.current_url = Some(sanitized_url.clone());
        // self.stop(owner);

        let ffmpeg_tx_clone = self.ffmpeg_tx.clone();
        godot_print!("Playing audio from: {}", sanitized_url);

        godot_print!("Starting audio thread");
        thread::spawn(move || {
            // Start yt-dlp process
            let mut yt_dlp: Child = Command::new("yt-dlp")
                .args(["-f", "bestaudio/best", "-o", "-", &sanitized_url])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .expect("Failed to start yt-dlp");

            let mut yt_dlp_output = yt_dlp.stdout.take().unwrap();

            let mut buffer = vec![0u8; 4096 * 4]; // Space for 4096 f32 samples

            while let Ok(bytes_read) = yt_dlp_output.read(&mut buffer) {
                if bytes_read == 0 {
                    break;
                }
                godot_print!("Read {} bytes from yt-dlp", bytes_read);
                if let Some(tx) = ffmpeg_tx_clone.as_ref() {
                    if tx.send(buffer[0..bytes_read].to_vec()).is_err() {
                        godot_error!("Error sending data to ffmpeg thread");
                        break;
                    }
                }
            }
        });
        true
    }

    #[method]
    fn _process(&mut self, #[base] _owner: &Node, _delta: f64) {
        if let Some(player) = self.player.as_ref() {
            if let Some(playback) = self.playback.as_ref() {
                if let Some(ref rx) = self.audio_data_rx {
                    // 1. Fill the buffer
                    loop {
                        match rx.try_recv() {
                            Ok(samples) => {
                                self.audio_buffer.extend(samples);
                            }
                            Err(TryRecvError::Empty) => break, // No more data for now
                            Err(TryRecvError::Disconnected) => {
                                // Handle disconnection
                                break;
                            }
                        }
                    }

                    let is_currently_playing = unsafe { player.assume_safe().is_playing() };

                    // 2. Start playback if the buffer is full enough
                    if !is_currently_playing && self.audio_buffer.len() >= self.buffer_threshold {
                        unsafe {
                            player.assume_safe().play(0.0);
                        }
                    }

                    // 3. Push data to playback if playing and buffer is not empty
                    while unsafe { player.assume_safe().is_playing() }
                        && !self.audio_buffer.is_empty()
                    {
                        let mut frames = PoolArray::<Vector2>::new();
                        let mut samples_consumed = 0;

                        // Consume up to 'buffer_size' samples from the buffer
                        while samples_consumed < self.buffer_size && !self.audio_buffer.is_empty() {
                            let left = self.audio_buffer.pop_front().unwrap_or(0.0);
                            let right = self.audio_buffer.pop_front().unwrap_or(left); // Mono handling
                            frames.push(Vector2::new(left, right));
                            samples_consumed += 2;
                        }

                        unsafe {
                            playback.assume_safe().push_buffer(frames);
                        }
                    }

                    // 4. Pause playback if the buffer is too low (optional)
                    // You can add a check here to pause playback if
                    // self.audio_buffer.len() falls below a certain minimum threshold.
                }
            }
        }
    }
}

// boilerplate code to register panic hook
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
        // godot_error!("Backtrace:\n{:?}", Backtrace::new());
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
