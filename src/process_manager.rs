use crate::{
    download_ffmpeg::download_ffmpeg, download_yt_dlp::download_yt_dlp, stream_errors::StreamError,
    CHANNELS, CREATE_NO_WINDOW, DEFAULT_BUFFER_SIZE, SAMPLE_RATE,
};
use command_group::{CommandGroup, GroupChild};
use gdnative::godot_print;

use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const TIMEOUT_DURATION: Duration = Duration::from_secs(20);
type FFmpegStreamResult = Result<(Sender<Vec<u8>>, Receiver<Vec<f32>>), StreamError>;

pub struct ProcessManager {
    root_path: PathBuf,
    ffmpeg_process: Option<GroupChild>,
    ytdlp_process: Option<GroupChild>,
}

impl ProcessManager {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            ffmpeg_process: None,
            ytdlp_process: None,
        }
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

    pub fn download_dependencies(&mut self) -> Result<(), StreamError> {
        // check if root path is set
        if self.root_path.as_path().exists() {
            return Ok(());
        }
        // download yt-dlp
        download_yt_dlp(self.root_path.as_path())
            .map_err(|e| StreamError::YtDlpDownloadError(e.to_string()))?;
        // download ffmpeg
        download_ffmpeg(self.root_path.as_path())
            .map_err(|e| StreamError::FfmpegDownloadError(e.to_string()))?;
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
