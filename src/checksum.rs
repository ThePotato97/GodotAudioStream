use std::{fs::File, path::Path};

use sha2::{Digest, Sha256};

pub fn verify_checksum(
    file_path: &Path,
    expected_checksum: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::open(file_path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let hash = hasher.finalize();
    let calculated_checksum = format!("{:x}", hash);

    if calculated_checksum == expected_checksum {
        Ok(())
    } else {
        Err(format!(
            "Checksum mismatch! Expected: {}, Calculated: {}",
            expected_checksum, calculated_checksum
        )
        .into())
    }
}

pub fn get_checksum_multiple(
    url: &str,
    binary_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let response = reqwest::blocking::get(url)?;
    let checksum = response
        .text()?
        .lines()
        .find(|line| line.ends_with(binary_name))
        .ok_or("Invalid checksum format")?
        .split(' ')
        .next()
        .ok_or("Invalid checksum format")?
        .to_string();

    Ok(checksum)
}
