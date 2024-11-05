use std::{
    fs::File,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    thread::sleep,
    time::Duration,
};

use gdnative::godot_print;

use crate::{CREATE_NO_WINDOW, YT_DLP_URL};

const MAX_RETRIES: u8 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);

fn is_yt_dlp_installed(output_dir: impl AsRef<Path>) -> bool {
    output_dir.as_ref().join("yt-dlp.exe").exists()
}

pub fn download_yt_dlp(
    output_dir: impl AsRef<Path>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // check if yt-dlp is already downloaded
    godot_print!("Checking if yt-dlp is already downloaded...");
    if is_yt_dlp_installed(output_dir.as_ref()) {
        // run yt-dlp update
        godot_print!("yt-dlp is already downloaded - updating...");
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

        godot_print!("yt-dlp updated successfully");
        return Ok(output_dir.as_ref().join("yt-dlp.exe"));
    }

    let output_dir = output_dir.as_ref();

    // download yt-dlp
    let mut retries = 0;
    while retries < MAX_RETRIES {
        match reqwest::blocking::get(YT_DLP_URL) {
            Ok(mut response) => {
                let mut file = File::create(output_dir.join("yt-dlp.exe"))?;
                godot_print!("Writing yt-dlp to file...");
                std::io::copy(&mut response, &mut file)?;
                return Ok(output_dir.join("yt-dlp.exe"));
            }
            Err(e) => {
                retries += 1;
                godot_print!("Download attempt {} failed: {}", retries, e);
                if retries < MAX_RETRIES {
                    godot_print!("Retrying in {} seconds...", RETRY_DELAY.as_secs());
                    sleep(RETRY_DELAY);
                } else {
                    return Err(format!(
                        "Failed to download yt-dlp after {} attempts: {}",
                        retries, e
                    )
                    .into());
                }
            }
        }
    }
    Err("Unexpected error in download retry loop.".into())
}
