use anyhow::{Context, Result};
use gdnative::godot_print;

use reqwest::blocking::ClientBuilder;
use serde::{Deserialize, Serialize};
use sevenz_rust::{Error as SzError, Password, SevenZReader};
use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::checksum::verify_checksum;

const FFMPEG_METADATA_VERSION: &str = "1.0.0";
const METADATA_FILENAME: &str = "metadata_ffmpeg.json";
const FFMPEG_EXECUTABLES: &[&str] = &["ffmpeg.exe", "ffprobe.exe"];

#[derive(Serialize, Deserialize)]
struct FFmpegMetadata {
    version: String,
}

const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.7z";

fn write_metadata(output_dir: &Path) -> Result<(), anyhow::Error> {
    let metadata = FFmpegMetadata {
        version: FFMPEG_METADATA_VERSION.to_string(),
    };

    let metadata_path = output_dir.join(METADATA_FILENAME);
    godot_print!("ffmpeg metadata path: {:?}", metadata_path);
    let mut file = fs::File::create(&metadata_path)?;
    let metadata_json = serde_json::to_string_pretty(&metadata)?;
    file.write_all(metadata_json.as_bytes())?;

    Ok(())
}

fn read_metadata(output_dir: &Path) -> Result<FFmpegMetadata, anyhow::Error> {
    let metadata_path = output_dir.join(METADATA_FILENAME);
    let metadata_json = fs::read_to_string(&metadata_path)?;
    let metadata: FFmpegMetadata = serde_json::from_str(&metadata_json)?;
    Ok(metadata)
}

fn remove_ffmpeg_executables(output_dir: &Path) -> Result<(), anyhow::Error> {
    for exe in FFMPEG_EXECUTABLES {
        let exe_path = output_dir.join(exe);
        if exe_path.exists() {
            fs::remove_file(&exe_path).context(format!("Failed to remove {}", exe))?;
        }
    }
    Ok(())
}

fn is_valid_installation(output_dir: &Path) -> bool {
    match read_metadata(output_dir) {
        Ok(metadata) => metadata.version == FFMPEG_METADATA_VERSION,
        Err(_) => false,
    }
}

fn move_temp_to_final(temp_path: &Path, final_path: &Path) -> Result<(), anyhow::Error> {
    fs::rename(temp_path, final_path)
        .context("Failed to move downloaded file to final destination")?;
    Ok(())
}

pub fn download_ffmpeg(root_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let root_dir = root_dir.as_ref();

    if !is_valid_installation(root_dir) {
        godot_print!("Installation is invalid, removing existing files");
        remove_ffmpeg_executables(root_dir)?;
    }

    let temp_dir = create_temp_dir_or_nuke(root_dir)?;
    let temp_path = temp_dir.path().to_owned();

    let client = ClientBuilder::new()
        .use_rustls_tls()
        .build()
        .context("Failed to build reqwest client")?;

    let executables = ["ffmpeg.exe", "ffprobe.exe"];
    let expected_final_paths: Vec<PathBuf> =
        executables.iter().map(|exe| root_dir.join(exe)).collect();
    let expected_paths: Vec<PathBuf> = executables
        .iter()
        .map(|exe| temp_dir.path().join(exe))
        .collect();

    // Check if FFmpeg is already extracted
    godot_print!("Checking if FFmpeg is already extracted");
    if expected_final_paths.iter().all(|path| path.exists()) {
        godot_print!("FFmpeg is already extracted");
        return Ok(expected_final_paths);
    }

    let archive_path = temp_dir.path().join("ffmpeg-release-essentials.7z");

    // Ensure the output directory exists
    fs::create_dir_all(root_dir).context("Failed to create output directory")?;

    // Download with retry
    godot_print!("Downloading FFmpeg archive...");
    download_archive(&archive_path)?;

    // Verify checksum
    godot_print!("Verifying checksum...");
    let response = client // Use the client
        .get(format!("{}.sha256", FFMPEG_URL))
        .send()
        .context("Failed to fetch checksum")?;

    let checksum = response.text()?;

    match verify_checksum(&archive_path, &checksum) {
        Ok(_) => {
            godot_print!("Checksum verified successfully");
        }
        Err(e) => {
            _cleanup(&archive_path)?;
            return Err(anyhow::anyhow!("Failed to verify checksum: {}", e));
        }
    }

    // Extract only needed executables
    godot_print!("Extracting FFmpeg executables...");
    extract_executables(&archive_path, temp_dir.path(), &executables)?;

    godot_print!("expected_paths: {:?}", expected_paths);

    // Verify extraction
    if !expected_paths.iter().all(|path| path.exists()) {
        _cleanup(&archive_path)?;
        return Err(anyhow::anyhow!(
            "Not all expected executables found after extraction"
        ));
    }

    for path in &expected_final_paths {
        move_temp_to_final(&temp_path.join(path.file_name().unwrap()), path)?;
    }

    // Remove downloaded archive
    _cleanup(&archive_path)?;

    temp_dir.close()?;
    write_metadata(root_dir).context("Failed to write metadata")?;
    godot_print!("FFmpeg binaries extracted successfully!");
    Ok(expected_final_paths)
}

fn extract_executables(archive_path: &Path, output_dir: &Path, executables: &[&str]) -> Result<()> {
    let file = fs::File::open(archive_path)?;
    let password = Password::empty();
    let file_size = file.metadata()?.len();

    let mut archive = SevenZReader::new(file, file_size, password)?;

    // Process each entry in the archive
    archive.for_each_entries(|entry, reader| {
        let entry_path = entry.name().to_string();

        // Check if this entry is one of our target executables
        if let Some(exe_name) = executables.iter().find(|&&exe| entry_path.ends_with(exe)) {
            godot_print!("Found {}", exe_name);

            // Create output file
            let output_path = output_dir.join(exe_name);
            let mut output_file = match File::create(&output_path) {
                Ok(f) => f,
                Err(e) => {
                    return Err(SzError::Other(
                        format!("Failed to create output file: {}", e).into(),
                    ))
                }
            };

            // Copy the file contents
            if let Err(e) = io::copy(reader, &mut output_file) {
                // Log the warning but don't return an error
                godot_print!(
                    "Warning while copying {} to {}: {}",
                    entry_path,
                    output_path.display(),
                    e
                );
                godot_print!("Warning: {}", e);
                // Optionally return Ok(true) to continue
            }
        }

        Ok(true) // Continue processing entries
    })?;

    Ok(())
}

fn create_temp_dir(output_dir: impl AsRef<Path>) -> Result<TempDir, anyhow::Error> {
    let output_dir = output_dir.as_ref();

    // Create the temporary directory in the same directory as the output_dir
    let temp_dir = TempDir::new_in(output_dir)
        .context("Failed to create temporary directory in output directory")?;

    Ok(temp_dir)
}

fn create_temp_dir_or_nuke(output_dir: impl AsRef<Path>) -> Result<TempDir, anyhow::Error> {
    let output_dir = output_dir.as_ref();
    let temp_dir = output_dir.join("temp");

    // If the temporary directory already exists, delete it and create a new one
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir).context("Failed to remove existing temporary directory")?;
    }
    create_temp_dir(output_dir)
}

fn download_archive(archive_path: &Path) -> Result<u64> {
    let client = ClientBuilder::new()
        .use_rustls_tls()
        .build()
        .context("Failed to build reqwest client")?;

    let mut file = fs::File::create(archive_path).context("Failed to create archive file")?;
    let mut response = client
        .get(FFMPEG_URL)
        .send()
        .context("Failed to download FFmpeg archive")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "Failed to download archive: {}",
            response.status()
        ));
    }

    let downloaded_bytes = response
        .copy_to(&mut file)
        .context("Failed to write to archive file")?;
    Ok(downloaded_bytes)
}

fn _cleanup(archive_path: &Path) -> Result<()> {
    if archive_path.exists() {
        fs::remove_file(archive_path).context("Failed to remove archive file")?;
    }
    Ok(())
}
