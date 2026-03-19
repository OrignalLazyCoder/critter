use std::{io, path::PathBuf, process::Command};

use crossterm::terminal;

pub(crate) fn ensure_model_local(
    model_file_name: &str,
    model_repo_url: &str,
) -> Result<PathBuf, String> {
    println!("[1/5] Checking local GGUF model file...");
    let model_dir = PathBuf::from("models");
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| format!("failed to create model directory: {e}"))?;

    let model_path = model_dir.join(model_file_name);
    if model_path.exists() {
        println!("Model found at {}", model_path.display());
        return Ok(model_path);
    }

    println!(
        "[2/5] Model not found. Downloading {} (~400-500MB)...",
        model_file_name
    );
    println!("Downloading from Hugging Face to {}", model_path.display());

    let download_status = Command::new("curl")
        .args([
            "-L",
            "--fail",
            "--progress-bar",
            "--output",
            model_path
                .to_str()
                .ok_or_else(|| "invalid model output path".to_string())?,
            model_repo_url,
        ])
        .status()
        .map_err(|e| format!("failed to launch curl download: {e}"))?;

    if !download_status.success() {
        return Err(format!(
            "model download failed with status {download_status}"
        ));
    }

    println!("Download complete.");
    Ok(model_path)
}

pub(crate) fn check_terminal_requirements(
    min_width: u16,
    min_height: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let (w, h) = terminal::size()?;
    if w < min_width || h < min_height {
        return Err(io::Error::other(format!(
            "terminal too small — resize to {}×{}",
            min_width, min_height
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn detect_truecolor_support() -> bool {
    let color_term = std::env::var("COLORTERM").unwrap_or_default();
    let c = color_term.to_ascii_lowercase();
    c.contains("truecolor") || c.contains("24bit")
}

pub(crate) fn warn_low_color_support(supports_truecolor: bool) {
    let term = std::env::var("TERM").unwrap_or_default();
    let supports_256 = term.contains("256color");
    if !supports_truecolor && !supports_256 {
        eprintln!(
            "Warning: terminal does not advertise 256/truecolor support. Using ANSI fallback."
        );
    }
}
