use std::{
    fs::File,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use gdnative::godot_print;

use crate::{CREATE_NO_WINDOW, YT_DLP_URL};

pub fn is_yt_dlp_installed(output_dir: impl AsRef<Path>) -> bool {
    output_dir.as_ref().join("yt-dlp.exe").exists()
}

pub fn download_yt_dlp(
    output_dir: impl AsRef<Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // check if yt-dlp is already downloaded
    if is_yt_dlp_installed(output_dir.as_ref()) {
        // run yt-dlp update

        let output = Command::new("yt-dlp")
            .arg("-U")
            .current_dir(output_dir.as_ref())
            .creation_flags(CREATE_NO_WINDOW) // Apply creation flags here
            .output()?;

        if !output.status.success() {
            return Err(format!(
                "Failed to update yt-dlp: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        godot_print!("yt-dlp is already downloaded");
        return Ok(output_dir.as_ref().join("yt-dlp.exe"));
    }

    let output_dir = output_dir.as_ref();

    // download yt-dlp
    let mut response = reqwest::blocking::get(YT_DLP_URL).expect("Failed to download yt-dlp");

    let mut file =
        File::create(output_dir.join("yt-dlp.exe")).expect("Failed to create yt-dlp.exe");

    std::io::copy(&mut response, &mut file).expect("Failed to copy yt-dlp");

    Ok(output_dir.join("yt-dlp.exe"))
}
