use anyhow::{Context, Result};
use gdnative::godot_print;

use sevenz_rust::{Error as SzError, Password, SevenZReader};

use std::fs;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use crate::checksum::verify_checksum;

const FFMPEG_URL: &str = "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.7z";

pub fn download_ffmpeg(output_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let output_dir = output_dir.as_ref();

    let executables = ["ffmpeg.exe", "ffprobe.exe"];
    let expected_paths: Vec<PathBuf> = executables.iter().map(|exe| output_dir.join(exe)).collect();

    // Check if FFmpeg is already extracted
    godot_print!("Checking if FFmpeg is already extracted");
    if expected_paths.iter().all(|path| path.exists()) {
        godot_print!("FFmpeg is already extracted");
        return Ok(expected_paths);
    }

    let archive_path = output_dir.join("ffmpeg-release-essentials.7z");

    // Ensure the output directory exists
    fs::create_dir_all(output_dir).context("Failed to create output directory")?;

    // Download with retry
    godot_print!("Downloading FFmpeg archive...");
    download_archive(&archive_path)?;

    // Verify checksum
    godot_print!("Verifying checksum...");
    let checksum = reqwest::blocking::get(format!("{}.sha256", FFMPEG_URL))
        .context("Failed to download checksum")?
        .text()
        .context("Failed to read checksum")?;
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
    extract_executables(&archive_path, output_dir, &executables)?;

    // Verify extraction
    if !expected_paths.iter().all(|path| path.exists()) {
        _cleanup(&archive_path)?;
        return Err(anyhow::anyhow!(
            "Not all expected executables found after extraction"
        ));
    }

    // Remove downloaded archive
    _cleanup(&archive_path)?;

    godot_print!("FFmpeg binaries extracted successfully!");
    Ok(expected_paths)
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

fn download_archive(archive_path: &Path) -> Result<u64> {
    let mut file = fs::File::create(archive_path).context("Failed to create archive file")?;
    let mut response =
        reqwest::blocking::get(FFMPEG_URL).context("Failed to download FFmpeg archive")?;

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
