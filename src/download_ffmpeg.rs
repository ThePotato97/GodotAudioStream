use anyhow::{Context, Result};
use gdnative::godot_print;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

use crate::FFMPEG_URL;

pub fn download_ffmpeg(output_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    // check if ffmpeg is already extracted
    godot_print!("Checking if FFmpeg is already extracted");
    godot_print!("Output directory: {:?}", output_dir.as_ref());
    if output_dir.as_ref().join("ffmpeg.exe").exists()
        && output_dir.as_ref().join("ffprobe.exe").exists()
    {
        godot_print!("FFmpeg is already extracted");
        return Ok(vec![
            output_dir.as_ref().join("ffmpeg.exe"),
            output_dir.as_ref().join("ffprobe.exe"),
        ]);
    }

    // Download the file into memory
    godot_print!("Downloading FFmpeg archive into memory...");

    let buffer = reqwest::blocking::get(FFMPEG_URL)
        .context("Failed to download FFmpeg archive")?
        .bytes()
        .context("Failed to read response bytes")?;

    godot_print!("Extracting from memory buffer...");
    let output_dir = output_dir.as_ref();
    fs::create_dir_all(output_dir)?;

    // Extract to a temporary directory
    let temp_dir = tempfile::tempdir()?;

    // Extract archive from memory buffer
    sevenz_rust::decompress(std::io::Cursor::new(buffer), temp_dir.path())
        .context("Failed to extract archive")?;

    // Find the bin directory
    let bin_pattern = Regex::new(r"ffmpeg-[\d\.]+-essentials_build\\bin$")?;
    let mut bin_dir = None;

    for entry in walkdir::WalkDir::new(temp_dir.path()) {
        let entry = entry?;
        let path_str = entry.path().to_string_lossy();

        if bin_pattern.is_match(&path_str) && entry.path().is_dir() {
            bin_dir = Some(entry.path().to_path_buf());
            break;
        }
    }

    let bin_dir = bin_dir.context("Could not find bin directory in archive")?;

    // Copy executables
    let executables = ["ffmpeg.exe", "ffprobe.exe"];
    let mut extracted_paths = Vec::new();

    for exe in executables {
        let source = bin_dir.join(exe);
        let destination = output_dir.join(exe);

        if source.exists() {
            fs::copy(&source, &destination).with_context(|| format!("Failed to copy {}", exe))?;
            extracted_paths.push(destination);
        }
    }

    godot_print!("FFmpeg binaries extracted successfully!");
    Ok(extracted_paths)
}
