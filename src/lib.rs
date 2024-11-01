use gdnative::api::{AudioStreamGeneratorPlayback, AudioStreamPlayer};
use gdnative::prelude::*;
use std::collections::VecDeque;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;
use url::Url;

#[derive(NativeClass)]
#[inherit(Node)]
struct YTStream {
    playback: Option<Ref<AudioStreamGeneratorPlayback>>,
    player: Option<Ref<AudioStreamPlayer>>,
    audio_data_rx: Option<Receiver<Vec<f32>>>,
    audio_buffer: VecDeque<f32>,
    buffer_size: i32,
    buffer_threshold: usize,
    yt_dlp: Option<Child>,       // Store the yt-dlp process
    ffmpeg: Option<Child>,       // Store the ffmpeg process
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
            audio_buffer: VecDeque::new(),
            buffer_threshold: 44100 * 5, // Buffer 5 seconds of audio
            yt_dlp: None,
            ffmpeg: None,
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
        godot_print!("YTStream ready")
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

    #[method]
    fn stop(&mut self, #[base] _owner: &Node) {
        //Kill Child processes
        if let Some(mut yt_dlp) = self.yt_dlp.take() {
            let _ = yt_dlp.kill(); // attempt to kill the process
            let _ = yt_dlp.wait(); //  wait for it to exit.
        }
        if let Some(mut ffmpeg) = self.ffmpeg.take() {
            let _ = ffmpeg.kill();
            let _ = ffmpeg.wait();
        }

        self.is_paused.store(false, Ordering::Relaxed);
        self.audio_data_rx = None;
        self.audio_buffer.clear();
        self.current_url = None;

        if let Some(player) = self.player.as_ref() {
            unsafe {
                player.assume_safe().stop();
            }
        }
    }

    #[method]
    fn play_youtube_audio(&mut self, #[base] owner: &Node, url: String) -> bool {
        if self.playback.is_none() {
            godot_print!("AudioStreamGeneratorPlayback not initialized");
            return false;
        }

        let is_paused_clone = self.is_paused.clone();

        let sanitized_url = match sanitize_url(&url) {
            Ok(url) => url,
            Err(err) => {
                godot_error!("Invalid URL: {}", err);
                return false;
            }
        };

        self.stop(owner);

        let (tx, rx) = channel::<Vec<f32>>();
        self.audio_data_rx = Some(rx);

        godot_print!("Playing audio from: {}", sanitized_url);

        godot_print!("Starting audio thread");
        thread::spawn(move || {
            // Start yt-dlp process
            let mut yt_dlp = Command::new("yt-dlp")
                .args(["-f", "bestaudio/best", "-o", "-", &sanitized_url])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn()
                .expect("Failed to start yt-dlp");

            godot_print!("yt-dlp started");

            // Start ffmpeg process configured for raw PCM output

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
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn()
                .expect("Failed to start ffmpeg");

            // Set up piping between processes

            let mut yt_dlp_output = yt_dlp.stdout.take().unwrap();
            let mut ffmpeg_input = ffmpeg.stdin.take().unwrap();
            let mut ffmpeg_output = ffmpeg.stdout.take().unwrap();

            // Pipe yt-dlp to ffmpeg
            godot_print!("Starting yt-dlp to ffmpeg pipe");
            thread::spawn(move || {
                std::io::copy(&mut yt_dlp_output, &mut ffmpeg_input)
                    .expect("Failed to pipe yt-dlp to ffmpeg");
            });

            // Read PCM data from ffmpeg
            let mut buffer = vec![0u8; 4096 * 4]; // Space for 4096 f32 samples

            godot_print!("Reading PCM data from ffmpeg");

            loop {
                if is_paused_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(10)); // Check pause state periodically
                    continue;
                }
                match ffmpeg_output.read(&mut buffer) {
                    Ok(n) if n == 0 => break, // End of stream
                    Ok(n) => {
                        // Convert bytes to f32
                        let samples = unsafe {
                            std::slice::from_raw_parts(buffer.as_ptr() as *const f32, n / 4)
                                .to_vec()
                        };

                        if let Err(_) = tx.send(samples) {
                            // Handle send errors (e.g., receiver disconnected)
                            break;
                        }
                    }
                    Err(e) => {
                        godot_error!("Error reading from ffmpeg: {}", e); // Handle errors!
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
