# 🤖 AGENTS.md — OmniRec Architecture & AI Agent Developer Guide

> This document is designed for AI Coding Agents and Human Developers working on **OmniRec**.  
> It provides deep architectural context, IPC command contracts, cross-platform media pipelines, DSP specifications, and development rules.

---

## 🧭 System Overview

**OmniRec** is a high-performance, cross-platform media suite built with **Tauri v2** (Rust) and **React 19 + TypeScript**.

```
┌────────────────────────────────────────────────────────────────────────┐
│                        React 19 Frontend (Vite)                        │
│  - ScreenRecorder  - AudioRecorder  - MediaJoiner  - AudioConverter    │
│  - HistoryList     - SettingsView   - MiniController- SelectionOverlay │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Tauri 2 IPC (invoke & events)
┌───────────────────────────────────▼────────────────────────────────────┐
│                        Tauri / Rust Core Backend                       │
│                                                                        │
│  ┌───────────────────────┐  ┌───────────────────────────────────────┐  │
│  │   RecorderController  │  │         cpal Audio Engine & DSP       │  │
│  │  - Screen (FFmpeg)    │  │  - WASAPI / CoreAudio Loopback Capture│  │
│  │  - Audio  (FFmpeg)    │  │  - NoiseGate / 80Hz HPF / Resampler   │  │
│  └───────────────────────┘  └───────────────────────────────────────┘  │
│                                                                        │
│  ┌───────────────────────┐  ┌───────────────────────────────────────┐  │
│  │    MergerController   │  │       AudioConverterController        │  │
│  │  - Lossless Direct    │  │  - WAV ➔ MP3 (libmp3lame) / M4A (AAC) │  │
│  │  - Complex Re-encode  │  │  - Real-time Progress & Speed parsing │  │
│  └───────────────────────┘  └───────────────────────────────────────┘  │
│                                                                        │
│  ┌───────────────────────┐  ┌───────────────────────────────────────┐  │
│  │    SettingsManager    │  │            HistoryManager             │  │
│  │  - ~/.omnirec/settings│  │  - ffprobe metadata caching           │  │
│  │  - Cross-OS binaries  │  │  - OS Explorer / Finder integration   │  │
│  └───────────────────────┘  └───────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 🔌 Tauri IPC API Reference (Rust Commands & Events)

### 1. Settings & FFmpeg
- `get_settings()` ➔ `Settings`
- `save_settings(settings: Settings)` ➔ `Result<(), String>`
- `check_ffmpeg_status(custom_ffmpeg_path: Option<String>)` ➔ `Result<String, String>`

### 2. Recording Management
- `start_screen_record(settings: Settings, region: Option<RectRegion>)` ➔ `Result<String, String>`
- `start_audio_record(settings: Settings)` ➔ `Result<String, String>`
- `pause_record()` ➔ `Result<(), String>`
- `resume_record()` ➔ `Result<(), String>`
- `toggle_pause_record()` ➔ `Result<(), String>`
- `stop_record()` ➔ `Result<String, String>`
- `get_recording_status()` ➔ `RecordingStatus`

### 3. Audio Format Converter
- `convert_audio_files(task: AudioConvertTaskPayload)` ➔ `Result<Vec<String>, String>`
- `cancel_conversion()` ➔ `Result<(), String>`

### 4. Media Merger / Joiner
- `probe_media_files(files: Vec<String>)` ➔ `Result<Vec<MediaProbeInfo>, String>`
- `merge_media_files(task: MergeTaskPayload)` ➔ `Result<String, String>`
- `cancel_merge()` ➔ `Result<(), String>`

### 5. History & Files
- `list_history_files()` ➔ `Vec<HistoryItem>`
- `delete_history_file(path: String)` ➔ `Result<(), String>`
- `read_audio_file(path: String)` ➔ `Result<Vec<u8>, String>`
- `open_in_explorer(path: String)` ➔ `Result<(), String>`
- `open_with_default_player(path: String)` ➔ `Result<(), String>`

### 6. Window & Overlay Controls
- `show_selection_overlay()` ➔ `Result<(), String>`
- `hide_selection_overlay()` ➔ `Result<(), String>`
- `confirm_selection_region(region: RectRegion)` ➔ `Result<(), String>`

### 📡 Real-Time Global Events emitted to Frontend
| Event Name | Payload Type | Description |
|---|---|---|
| `recording_status_change` | `RecordingStatus` | Emitted when recording state changes (idle, recording, paused, stopping). |
| `audio_vu_meter` | `AudioVUMeterPayload` | Real-time VU meter dB levels for system & mic, duration, estimated bytes. |
| `merge_progress` | `MergeProgressPayload` | Real-time percent, current/total seconds, direct-copy flag, speed. |
| `conversion_progress` | `AudioConvertProgressPayload`| Real-time file index, total files, percent, overall percent, speed. |
| `region_selected` | `RectRegion` | Selection coordinates emitted when user drags and confirms area. |
| `auto_stop_triggered` | `None` | Emitted when silence duration exceeds threshold and recorder auto-stops. |

---

## 🎛️ Audio Engine & DSP Architecture (`src-tauri/src/audio/`)

1. **Dual Stream Capture**:
   - Uses `cpal` to capture output loopback (system sounds) and default input (microphone).
   - Handles `f32` and `i16` sample formats dynamically.
2. **Linear Resampler (`StereoLinearResampler`)**:
   - Interpolates native device sample rates (e.g. 44.1kHz, 48kHz, 96kHz) to target sample rate (48kHz standard).
3. **Smart Noise Gate (`NoiseGate`)**:
   - Attack time: 2ms, Release time: 50ms, Hold time: 40ms.
   - Attenuates noise below configurable threshold (-60dB to -20dB).
4. **80Hz High-Pass (Low-cut) IIR Filter (`BiquadHighPass80Hz`)**:
   - 2nd-order Butterworth IIR filter removing rumble, table vibration, and air-con frequencies.
5. **Silence Detector (`SilenceDetector`)**:
   - Tracks RMS energy. Triggers `AudioEngineEvent::AutoPause`, `AutoResume`, or `AutoStop` based on user-configured timers.
6. **FFmpeg Audio Pipe**:
   - Mixed audio is streamed as raw `f32le` stereo interleaved PCM to FFmpeg process `pipe:0` (stdin).

---

## 🎞️ FFmpeg Process & Platform Dispatch (`src-tauri/src/recorder/`)

### Screen Capture Driver Matrix
- **Windows**: `-f gdigrab -framerate {fps} -draw_mouse 1 [-offset_x {x} -offset_y {y} -video_size {w}x{h}] -i desktop`
- **macOS**: `-f avfoundation -framerate {fps} -capture_cursor 1 -i "1:none" [-vf crop={w}:{h}:{x}:{y}]`
- **Linux**: `-f x11grab -framerate {fps} -draw_mouse 1 [-video_size {w}x{h} -i :0.0+{x},{y}]`

### Multi-Window Setup (`tauri.conf.json`)
- `main`: Primary application window.
- `selection-overlay`: Transparent, fullscreen, frameless window for drag-to-crop region selection.
- `mini-controller`: Compact, frameless, always-on-top floating toolbar displayed during active screen recordings.

---

## 🧪 Development & Testing Workflow

### Checking Rust Backend
```bash
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

### Checking Frontend Types & Build
```bash
npm run build
```

### Running in Local Dev
```bash
npm run tauri dev
```

---

## 💡 Guidelines for Future Agents
1. **Never block the Tauri main thread**: Always use `tokio::task::spawn_blocking` for FFmpeg execution, conversions, and heavy disk I/O.
2. **Graceful Subprocess Termination**: When stopping or canceling, close stdin / send EOF or send kill signals cleanly to avoid orphaned FFmpeg processes.
3. **Cross-Platform Compatibility**: Always gate OS-specific APIs (`windows::Win32`, Windows Registry, Windows Explorer, macOS Finder, macOS Cocoa) behind `#[cfg(target_os = "...")]`.
4. **Even Dimension Constraint**: `libx264` with `yuv420p` requires width and height to be divisible by 2 (`(w / 2) * 2`). Always enforce this for custom screen crop regions.
