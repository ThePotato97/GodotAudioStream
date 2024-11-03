use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use crate::{
    stream_errors::StreamError, CHANNELS, CREATE_NO_WINDOW, DEFAULT_BUFFER_SIZE, SAMPLE_RATE,
};

type FFmpegStreamResult = Result<(Sender<Vec<u8>>, Receiver<Vec<f32>>), StreamError>;

pub struct ProcessManager {
    root_path: PathBuf,
    ffmpeg_process: Option<Child>,
    ytdlp_process: Option<Child>,
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

        let mut ffmpeg = Command::new("ffmpeg")
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
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| StreamError::ProcessError(format!("Failed to start FFmpeg: {}", e)))?;

        let ffmpeg_stdout = ffmpeg
            .stdout
            .take()
            .ok_or_else(|| StreamError::ProcessError("Failed to get FFmpeg stdout".to_string()))?;
        let ffmpeg_stdin = ffmpeg
            .stdin
            .take()
            .ok_or_else(|| StreamError::ProcessError("Failed to get FFmpeg stdin".to_string()))?;

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
        thread::Builder::new()
            .name("ffmpeg_stdin".to_string())
            .spawn(move || {
                while let Ok(data) = rx.recv() {
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
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let samples = unsafe {
                                std::slice::from_raw_parts(buffer.as_ptr() as *const f32, n / 4)
                                    .to_vec()
                            };
                            if tx.send(samples).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
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
        let mut ytdlp = Command::new("yt-dlp")
            .args(["-f", "bestaudio/best", "-o", "-", url])
            .current_dir(&self.root_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| StreamError::ProcessError(format!("Failed to start yt-dlp: {}", e)))?;

        let mut stdout = ytdlp
            .stdout
            .take()
            .ok_or_else(|| StreamError::ProcessError("Failed to get yt-dlp stdout".to_string()))?;

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
