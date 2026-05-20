use std::path::PathBuf;
use tauri::{AppHandle, Manager};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Resolve the whisper-cpp binary path and its containing directory.
/// In dev mode uses CARGO_MANIFEST_DIR (compile-time); in release uses the
/// Tauri resource directory where the binary is bundled.
fn whisper_binary(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let triple = if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "aarch64-apple-darwin" } else { "x86_64-apple-darwin" }
    } else {
        "x86_64-unknown-linux-gnu"
    };

    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let binary_name = format!("whisper-cpp-{}{}", triple, ext);

    // Dev mode: binary lives next to Cargo.toml in binaries/
    let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    let dev_path = dev_dir.join(&binary_name);
    if dev_path.exists() {
        return Ok((dev_path, dev_dir));
    }

    // Release mode: binary is in the Tauri resource directory
    if let Ok(res_dir) = app.path().resource_dir() {
        let rel_path = res_dir.join(&binary_name);
        if rel_path.exists() {
            return Ok((rel_path.clone(), res_dir));
        }
    }

    Err(format!("whisper-cpp binary not found (looked for {})", binary_name))
}

pub async fn transcribe_local(
    app: &AppHandle,
    model_path: &PathBuf,
    audio_path: &PathBuf,
) -> Result<String, String> {
    if !model_path.exists() {
        return Err("Whisper model not found. Please download a model first.".to_string());
    }

    let (binary_path, binary_dir) = whisper_binary(app)?;
    println!("[Typr] Running whisper.cpp: {:?}", binary_path);
    println!("[Typr]   model: {:?}", model_path);
    println!("[Typr]   audio: {:?}", audio_path);

    // Use std::process::Command directly so we control the working directory.
    // Setting cwd = binary_dir ensures Windows finds the co-located DLLs
    // (ggml.dll, whisper.dll, etc.) regardless of the parent process's cwd.
    let output = tokio::task::spawn_blocking({
        let binary_path = binary_path.clone();
        let binary_dir = binary_dir.clone();
        let model_path = model_path.clone();
        let audio_path = audio_path.clone();
        move || {
            let mut cmd = std::process::Command::new(&binary_path);
            cmd.current_dir(&binary_dir)
                .args([
                    "-m", model_path.to_str().unwrap_or_default(),
                    "-f", audio_path.to_str().unwrap_or_default(),
                    "--no-timestamps",
                    "-l", "en",
                    "--no-gpu",
                    "--no-flash-attn",
                ]);
            #[cfg(target_os = "windows")]
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
            cmd.output()
        }
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
    .map_err(|e| format!("Failed to launch whisper.cpp: {}", e))?;

    let exit_code = output.status.code();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("[Typr] whisper.cpp exit={:?} stdout={:?}", exit_code, stdout);
    if !stderr.is_empty() {
        println!("[Typr] whisper.cpp stderr: {}", stderr.lines().next().unwrap_or(""));
    }

    if !stdout.is_empty() {
        println!("[Typr] Transcription: {}", stdout);
        return Ok(stdout);
    }

    if exit_code != Some(0) {
        return Err(format!(
            "whisper.cpp failed (exit {:?}): {}",
            exit_code,
            stderr.lines().next().unwrap_or("no output")
        ));
    }

    Err("whisper.cpp produced no transcription".to_string())
}

pub fn model_filename(model_size: &str) -> String {
    format!("ggml-{}.bin", model_size)
}

pub fn model_download_url(model_size: &str) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model_size
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_filename() {
        assert_eq!(model_filename("small"), "ggml-small.bin");
        assert_eq!(model_filename("medium"), "ggml-medium.bin");
    }

    #[test]
    fn test_model_download_url() {
        assert_eq!(
            model_download_url("small"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
        );
    }
}
