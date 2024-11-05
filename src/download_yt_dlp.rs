use gdnative::godot_print;
use reqwest::blocking::ClientBuilder;
use std::{
    fs::File,
    io::{BufWriter, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
use anyhow::{Context, Result};   
use crate::{
    checksum::{get_checksum_multiple, verify_checksum},
    CREATE_NO_WINDOW,
};

const MAX_RETRIES: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);

const YT_DLP_BIN_NAME: &str = "yt-dlp.exe";
const YT_DLP_CHECKSUM_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const YT_DLP_DOWNLOAD_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

fn is_yt_dlp_installed(output_dir: impl AsRef<Path>) -> bool {
    output_dir.as_ref().join(YT_DLP_BIN_NAME).exists()
}

fn update_yt_dlp(output_dir: impl AsRef<Path>) -> Result<(), Box<dyn std::error::Error>> {
    godot_print!("yt-dlp is already downloaded - checking for updates...");

    let output = Command::new("yt-dlp")
        .arg("-U")
        .current_dir(output_dir.as_ref())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to update yt-dlp: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    godot_print!("yt-dlp updated successfully");
    Ok(())
}

pub fn download_yt_dlp(
    output_dir: impl AsRef<Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output_dir = output_dir.as_ref();

    let client = ClientBuilder::new()
        .use_rustls_tls()
        .build()
        .context("Failed to build reqwest client")?;

    if is_yt_dlp_installed(output_dir) {
        let checksum = get_checksum_multiple(YT_DLP_CHECKSUM_URL, YT_DLP_BIN_NAME)?;

        match verify_checksum(&output_dir.join(YT_DLP_BIN_NAME), &checksum) {
            Ok(_) => {
                update_yt_dlp(output_dir)?;
                return Ok(output_dir.join(YT_DLP_BIN_NAME));
            }
            Err(e) => {
                godot_print!("Checksum verification failed: {}", e);
                std::fs::remove_file(output_dir.join(YT_DLP_BIN_NAME))?; // Remove corrupted file
            }
        }
    }

    godot_print!("Downloading yt-dlp");

    // Download checksum first
    let checksum = get_checksum_multiple(YT_DLP_CHECKSUM_URL, YT_DLP_BIN_NAME)?;

    // Download yt-dlp
    let mut download_response = client // Use the client
        .get(YT_DLP_DOWNLOAD_URL)
        .send()
        .context("Failed to download yt-dlp")?;

    let yt_dlp_path = output_dir.join(YT_DLP_BIN_NAME);

    let mut file = BufWriter::new(File::create(&yt_dlp_path)?); // Buffered writer for efficiency
    download_response.copy_to(&mut file)?;
    file.flush()?; // Ensure all data is written
    drop(file); // Close the file

    match verify_checksum(&yt_dlp_path, &checksum) {
        Ok(_) => return Ok(yt_dlp_path),
        Err(e) => {
            godot_print!("Checksum verification failed: {}", e);
            std::fs::remove_file(&yt_dlp_path)?; // Remove corrupted file
        }
    }
    Err("Failed to download yt-dlp".into())
}
