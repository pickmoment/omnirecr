use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use chrono::{DateTime, Local};

use crate::merger::MergerController;
use crate::types::HistoryItem;

pub struct HistoryManager;

impl HistoryManager {
    pub fn list_files(output_dir: &str, custom_ffmpeg_path: Option<String>) -> Vec<HistoryItem> {
        let dir = Path::new(output_dir);
        if !dir.exists() || !dir.is_dir() {
            return Vec::new();
        }

        let mut file_paths = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ["mp3", "m4a", "wav", "mp4", "mov", "mkv", "webm"].contains(&ext_lower.as_str()) {
                            file_paths.push(path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }

        let probes = MergerController::probe_files(file_paths.clone(), custom_ffmpeg_path).unwrap_or_default();
        let probe_map: std::collections::HashMap<String, _> = probes
            .into_iter()
            .map(|p| (p.path.clone(), p))
            .collect();

        let mut items = Vec::new();

        for file_str in file_paths {
            let path = PathBuf::from(&file_str);
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let is_video = ["mp4", "mov", "mkv", "webm"].contains(&ext.as_str());

            let metadata = fs::metadata(&path).ok();
            let size_bytes = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let size_formatted = format_file_size(size_bytes);

            let created_at = metadata
                .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                .map(|t| {
                    let dt: DateTime<Local> = t.into();
                    dt.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|| "Unknown".to_string());

            let probe_info = probe_map.get(&file_str);
            let duration_secs = probe_info.map(|p| p.duration_secs).unwrap_or(0.0);
            let duration_formatted = format_duration(duration_secs);

            let resolution = probe_info.and_then(|p| {
                if let (Some(w), Some(h)) = (p.width, p.height) {
                    Some(format!("{}x{}", w, h))
                } else {
                    None
                }
            });

            items.push(HistoryItem {
                id: file_str.clone(),
                file_name,
                file_path: file_str,
                file_type: if is_video { "video".to_string() } else { "audio".to_string() },
                format: ext,
                size_bytes,
                size_formatted,
                duration_secs,
                duration_formatted,
                created_at,
                resolution,
            });
        }

        // Sort descending by creation date, then by file name
        items.sort_by(|a, b| {
            b.created_at.cmp(&a.created_at).then_with(|| b.file_name.cmp(&a.file_name))
        });
        items
    }

    pub fn delete_file(path_str: &str) -> Result<(), String> {
        let p = Path::new(path_str);
        if p.exists() {
            fs::remove_file(p).map_err(|e| format!("Failed to delete file: {}", e))?;
        }
        Ok(())
    }

    pub fn open_in_explorer(path_str: &str) -> Result<(), String> {
        let p = Path::new(path_str);
        if !p.exists() {
            return Err("File not found.".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            Command::new("explorer")
                .arg(format!("/select,\"{}\"", path_str))
                .spawn()
                .map_err(|e| format!("Failed to open Windows Explorer: {}", e))?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg("-R")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to reveal file in Finder: {}", e))?;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            let _ = opener::open(path_str);
        }

        Ok(())
    }

    pub fn open_with_default_player(path_str: &str) -> Result<(), String> {
        let p = Path::new(path_str);
        if !p.exists() {
            return Err("File not found.".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            Command::new("rundll32.exe")
                .arg("url.dll,FileProtocolHandler")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file: {}", e))?;
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file on macOS: {}", e))?;
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            Command::new("xdg-open")
                .arg(path_str)
                .spawn()
                .map_err(|e| format!("Failed to open media file: {}", e))?;
        }

        Ok(())
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_duration(seconds: f64) -> String {
    let s = seconds.round() as u64;
    let hrs = s / 3600;
    let mins = (s % 3600) / 60;
    let secs = s % 60;

    if hrs > 0 {
        format!("{:02}:{:02}:{:02}", hrs, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}
