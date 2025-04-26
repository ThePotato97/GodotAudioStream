use crate::{
    download_ffmpeg::download_ffmpeg, download_yt_dlp::download_yt_dlp, stream_errors::StreamError,
    CHANNELS, CREATE_NO_WINDOW, DEFAULT_BUFFER_SIZE, SAMPLE_RATE,
};
use command_group::{CommandGroup, GroupChild};
use gdnative::{derive::ToVariant, godot_print};

use serde::Deserialize;

use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const TIMEOUT_DURATION: Duration = Duration::from_secs(20);
type FFmpegStreamResult = Result<(Sender<Vec<u8>>, Receiver<Vec<f32>>), StreamError>;

#[derive(Debug, Deserialize, Clone, ToVariant)]
pub struct YtDlpMetadata {
    pub title: String,
    pub uploader: Option<String>, // Option used if the field might be missing
    pub duration: Option<u64>,    // Duration in seconds, if applicable
    pub webpage_url: Option<String>,
    pub thumbnail: Option<String>,
}

pub struct ProcessManager {
    initialized: Arc<AtomicBool>,
    root_path: PathBuf,
    ffmpeg_process: Option<GroupChild>,
    ytdlp_process: Option<GroupChild>,
    pub ytdlp_metadata: Arc<Mutex<Option<YtDlpMetadata>>>,
}

impl ProcessManager {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            initialized: Arc::new(AtomicBool::new(false)),
            root_path,
            ffmpeg_process: None,
            ytdlp_process: None,
            ytdlp_metadata: Arc::new(Mutex::new(None)),
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    pub fn set_root_path(&mut self, path: PathBuf) {
        self.root_path = path;
    }
    pub fn start_ffmpeg(&mut self) -> FFmpegStreamResult {
        let (audio_tx, audio_rx) = channel();
        let (ffmpeg_tx, ffmpeg_rx) = channel();

        // Cleanup any existing processes
        self.cleanup();

        let ffmpeg_path = self.root_path.join("ffmpeg");

        let mut ffmpeg = Command::new(ffmpeg_path)
            .args([
                "-i",
                "pipe:0",
                "-f",
                "f32le",
                "-acodec",
                "pcm_f32le",
                "-ar",
                &SAMPLE_RATE.to_string(),
                "-ac",
                &CHANNELS.to_string(),
                "pipe:1",
            ])
            .current_dir(&self.root_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .group()
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| StreamError::ProcessError(format!("Failed to start FFmpeg: {}", e)))?;

        let ffmpeg_stdout =
            ffmpeg.inner().stdout.take().ok_or_else(|| {
                StreamError::ProcessError("Failed to get FFmpeg stdout".to_string())
            })?;
        let ffmpeg_stdin =
            ffmpeg.inner().stdin.take().ok_or_else(|| {
                StreamError::ProcessError("Failed to get FFmpeg stdin".to_string())
            })?;

        self.spawn_ffmpeg_threads(ffmpeg_stdin, ffmpeg_stdout, ffmpeg_rx, audio_tx)?;
        self.ffmpeg_process = Some(ffmpeg);

        Ok((ffmpeg_tx, audio_rx))
    }

    pub fn spawn_ffmpeg_threads(
        &self,
        mut stdin: impl Write + Send + 'static,
        mut stdout: impl Read + Send + 'static,
        rx: Receiver<Vec<u8>>,
        tx: Sender<Vec<f32>>,
    ) -> Result<(), StreamError> {
        let start = Instant::now();
        let last_received = Arc::new(AtomicU64::new(start.elapsed().as_nanos() as u64));

        let last_received_stdin = Arc::clone(&last_received);
        let last_received_stdout = Arc::clone(&last_received);
        thread::Builder::new()
            .name("ffmpeg_stdin".to_string())
            .spawn(move || {
                while let Ok(data) = rx.recv() {
                    last_received_stdin.store(start.elapsed().as_nanos() as u64, Ordering::Relaxed);

                    if stdin.write_all(&data).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| {
                StreamError::ProcessError(format!("Failed to spawn FFmpeg stdin thread: {}", e))
            })?;

        thread::Builder::new()
            .name("ffmpeg_stdout".to_string())
            .spawn(move || {
                let mut buffer = vec![0u8; DEFAULT_BUFFER_SIZE as usize * 4];

                let running: Arc<AtomicBool> = Arc::new(AtomicBool::new(true));
                let running_clone = running.clone();

                let last_received_stdout_clone = last_received_stdout.clone();

                let _timeout_thread = thread::spawn(move || {
                    while running_clone.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_secs(1));

                        let elapsed_ns = last_received_stdout_clone.load(Ordering::Relaxed);
                        let last_instant = start + Duration::from_nanos(elapsed_ns);

                        if Instant::now().duration_since(last_instant) > TIMEOUT_DURATION {
                            godot_print!("FFmpeg timed out due to inactivity.");
                            running_clone.store(false, Ordering::Relaxed);
                            break;
                        }
                    }
                });

                while running.load(Ordering::Relaxed) {
                    match stdout.read(&mut buffer) {
                        Ok(0) => {
                            godot_print!("FFmpeg closed stdout.");
                            break;
                        }
                        Ok(n) => {
                            let samples = unsafe {
                                std::slice::from_raw_parts(buffer.as_ptr() as *const f32, n / 4)
                                    .to_vec()
                            };
                            if tx.send(samples).is_err() {
                                break;
                            }
                            last_received_stdout
                                .store(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            // No data available right now, sleep a bit to prevent busy-waiting
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => {
                            godot_print!("Failed to read FFmpeg stdout.");
                            break;
                        }
                    }
                }
            })
            .map_err(|e| {
                StreamError::ProcessError(format!("Failed to spawn FFmpeg stdout thread: {}", e))
            })?;

        Ok(())
    }
    pub fn get_ytdlp_metadata(&self, url: &str) {
        let ytdlp_path: PathBuf = self.root_path.join("yt-dlp");
        let metadata_store = Arc::clone(&self.ytdlp_metadata);
        let root_path = self.root_path.clone();
        let url = url.to_string();

        thread::spawn(move || {
            let mut ytdlp = Command::new(ytdlp_path)
                .args([
                    "--dump-json",
                    "--ignore-errors",
                    "--quiet",
                    "--playlist-items",
                    "1",
                    &url,
                ])
                .current_dir(&root_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .group()
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| eprintln!("Failed to start yt-dlp: {}", e))
                .ok()?;

            let stdout = ytdlp.inner().stdout.take()?;
            let reader = BufReader::new(stdout);
            let mut metadata = String::new();

            for line in reader.lines() {
                match line {
                    Ok(chunk) => metadata.push_str(&chunk),
                    Err(e) => {
                        godot_print!("Error reading yt-dlp output: {}", e);
                        return None;
                    }
                }
            }

            // Parse the JSON output
            let parsed_metadata: YtDlpMetadata = match serde_json::from_str(&metadata) {
                Ok(data) => data,
                Err(e) => {
                    godot_print!("Failed to parse metadata as JSON: {}", e);
                    return None;
                }
            };

            // Store the parsed metadata in the Arc<Mutex<Option<YtDlpMetadata>>>
            if let Ok(mut meta_lock) = metadata_store.lock() {
                *meta_lock = Some(parsed_metadata);
            } else {
                godot_print!("Failed to acquire lock to store metadata");
            }

            Some(())
        });
    }
    pub fn start_ytdlp(
        &mut self,
        url: &str,
        ffmpeg_tx: Sender<Vec<u8>>,
    ) -> Result<(), StreamError> {
        let ytdlp_path = self.root_path.join("yt-dlp");
        let mut ytdlp = Command::new(ytdlp_path)
            .args([
                "-f",
                "bestaudio/best",
                "-o",
                "-",
                "--limit-rate",
                "300K",
                url,
            ])
            .current_dir(&self.root_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .group()
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| StreamError::ProcessError(format!("Failed to start yt-dlp: {}", e)))?;

        let mut stdout =
            ytdlp.inner().stdout.take().ok_or_else(|| {
                StreamError::ProcessError("Failed to get yt-dlp stdout".to_string())
            })?;

        thread::Builder::new()
            .name("ytdlp_stdout".to_string())
            .spawn(move || {
                let mut buffer = vec![0u8; DEFAULT_BUFFER_SIZE as usize * 4];
                while let Ok(n) = stdout.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    if ffmpeg_tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| {
                StreamError::ProcessError(format!("Failed to spawn yt-dlp stdout thread: {}", e))
            })?;

        self.ytdlp_process = Some(ytdlp);
        Ok(())
    }

    pub fn download_dependencies(&mut self, root_path: PathBuf) -> Result<(), StreamError> {
        // Check if root path is set
        godot_print!("Checking if root path is set..., {:?}", root_path);

        // Make path if it doesn't exist
        if !root_path.exists() {
            fs::create_dir_all(&root_path)?;
        }
        let initialized_clone = self.initialized.clone();

        thread::Builder::new()
            .name("download_dependencies".to_string())
            .spawn(move || {
                // Attempt to download yt-dlp
                godot_print!("Downloading yt-dlp...");
                if let Err(e) = download_yt_dlp(root_path.as_path()) {
                    godot_print!("Failed to download yt-dlp: {:?}", e);
                }
                // Attempt to download ffmpeg, regardless of yt-dlp success
                godot_print!("Downloading ffmpeg...");
                if let Err(e) = download_ffmpeg(root_path.as_path()) {
                    godot_print!("Failed to download ffmpeg: {:?}", e);
                }
                initialized_clone.store(true, Ordering::Relaxed);
            })
            .map_err(|e| {
                StreamError::ProcessError(format!(
                    "Failed to spawn download_dependencies thread: {}",
                    e
                ))
            })?;

        Ok(())
    }

    pub fn cleanup(&mut self) {
        if let Some(mut process) = self.ffmpeg_process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
        if let Some(mut process) = self.ytdlp_process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

impl Drop for ProcessManager {
    fn drop(&mut self) {
        self.cleanup();
    }
}
