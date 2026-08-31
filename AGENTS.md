# 🤖 AGENTS.md — OmniRec Architecture & AI Agent Developer Guide

> This document is designed for AI Coding Agents and Human Developers working on **OmniRec**.  
> It provides deep architectural context, IPC command contracts, cross-platform media pipelines, DSP specifications, and development rules.

---

## 🧭 System Overview

**OmniRec** is a high-performance, cross-platform media suite built with **Tauri v2** (Rust) and **React 19 + TypeScript**.

```
┌────────────────────────────────────────────────────────────────────────┐
│                        React 19 Frontend (Vite)                        │
│  상단 탭은 작업 흐름 단위 5개. 세부 화면은 각 탭 안의 서브 탭.          │
│   record  → RecordStudio   (AudioRecorder · ScreenRecorder)            │
│   script  → ScriptStudio   (TtsBatchRunner · ScriptLibrary · TtsRec.)  │
│   subtitle→ SubtitleStudio (SubtitleGenerator · SubtitleBatchRunner)   │
│   files   → FileStudio     (HistoryList · MediaJoiner · AudioConverter)│
│   settings→ SettingsView                                              │
│  공용: TabBar · TypecastSessionCard · TypecastDiagnosticsLog ·         │
│        SubtitleOptionsPanel · AudioVisualizer                          │
│  별도 창: SelectionOverlay · MiniController                            │
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
  (`region` 은 **가상 데스크톱 전역 물리 좌표**다. 그 좌표가 나온 모니터 정보는 커맨드가
   `AppState.last_selection_screen` 에서 꺼내 함께 넘긴다 — 아래 "Screen Capture Driver Matrix" 참고)
- `start_audio_record(settings: Settings, file_name_prefix: Option<String>, show_mini_controller: Option<bool>, exact_file_name: Option<bool>)` ➔ `Result<String, String>` (`exact_file_name: true` 는 타임스탬프 없이 `file_name_prefix` 그대로를 파일명으로 쓴다 — 대본 & TTS 녹음 전용)
- `resolve_script_recording_targets(settings: Settings, file_name_prefixes: Vec<String>)` ➔ `Vec<ScriptRecordingTarget>`
  (대본 제목들이 저장될 실제 경로와 존재 여부. 덮어쓰기 확인 + 제목 충돌 검사 둘 다 이걸 쓴다. 녹음은 시작하지 않는다)
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

### 5. Subtitle Generator (Script-to-Sub & Local AI Whisper)
- `generate_subtitles(task: SubtitleGenerateTask)` ➔ `Result<SubtitleGenerateResult, String>` (VAD + DP 엔진)
- `save_subtitle_file(path: String, content: String)` ➔ `Result<(), String>`
- `read_script_file(path: String)` ➔ `Result<String, String>`
- `extract_audio_pcm_16k(path: String)` ➔ `ArrayBuffer` (raw f32le 바이트)
  프론트엔드는 `new Float32Array(buf)` 로 **사본 없이** 본다. 예전에 `Vec<f32>` 를 반환해 Tauri 가
  JSON `number[]` 로 직렬화하던 것을 `tauri::ipc::Response` 로 바꿨다 — 1시간 오디오가 5,760만
  샘플이라 JSON 문자열 수백 MB + 사본 3벌이 되어 전사 시작 전에 앱이 죽었다. 사본을 다시 만들지 말 것
  (NaN/Inf 세정은 제자리에서 한다).

`generateSubtitles()`(`services/subtitleGeneration.ts`)의 결과 계약 두 가지를 지킬 것:
- **`saveFailures: SubtitleSaveFailure[]`** — 자동 저장을 요청했는데 안 만들어진 포맷 목록.
  비어 있지 않으면 '완료'가 아니다. 두 화면(`SubtitleGenerator`, `SubtitleBatchRunner`)이 모두
  이걸 읽어 사용자에게 보여준다. 저장 실패를 `console.error` 로만 남기던 시절에는 파일이 없는데
  성공으로 보였다.
- **`signal?: AbortSignal`** — 취소는 성공도 실패도 아니다. 일괄 생성의 중단 버튼은 이 시그널을
  끊어 **진행 중인 항목까지** 중단하고(예전에는 항목 사이에서만 멈춰, 진행 중 항목이 끝까지 돌아
  '완료'로 보고되고 파일까지 썼다), 호출자는 `isSubtitleCancelled(err)` 로 '건너뜀'과 '실패'를
  구분한다.

### 6. Script Library & Typecast TTS Recording

> 자막 생성 파이프라인은 `src/services/subtitleGeneration.ts` 한 곳에 있다.
> `SubtitleGenerator`(단건 편집 화면)와 `SubtitleBatchRunner`(대본 일괄 생성)가 같은 함수를 쓴다.
> 엔진을 고칠 때 두 곳에 나눠 쓰지 말 것.
- `list_scripts()` ➔ `Vec<ScriptItem>` (최근 수정순)
- `save_script(draft: ScriptDraft)` ➔ `Result<ScriptItem, String>` (id 없으면 신규 생성)
- `delete_script(id: String)` ➔ `Result<(), String>`
- `duplicate_script(id: String)` ➔ `Result<ScriptItem, String>`
- `import_script_file(path: String)` ➔ `Result<ScriptItem, String>`
- `export_script_file(id: String, path: String)` ➔ `Result<(), String>`
- `attach_script_recording(id: String, recorded_path: String)` ➔ `Result<ScriptItem, String>`
- `open_typecast_browser(url: Option<String>)` ➔ `Result<(), String>`
- `close_typecast_browser()` / `focus_typecast_browser()` ➔ `Result<(), String>`
- `navigate_typecast_browser(url: String)` ➔ `Result<(), String>`
- `typecast_go_back()` / `typecast_reload()` ➔ `Result<(), String>`
- `clear_typecast_session()` ➔ `Result<(), String>` (쿠키/스토리지 전체 삭제 → 재로그인)
- `get_typecast_browser_state()` ➔ `TypecastBrowserState`
- `mark_typecast_login(email: Option<String>)` ➔ `Result<Settings, String>`
- `push_script_to_typecast(text: String)` ➔ `Result<bool, String>` (클립보드 복사 + 편집기 자동 입력 시도)
- `notify_typecast(message: String, tone: Option<String>)` ➔ `Result<(), String>`
- `typecast_prepare_script(text: String, copy_to_clipboard: Option<bool>)` ➔ `Result<(), String>`
  (편집기 자동 입력, 결과는 `typecast_step` 으로 보고. 클립보드 복사는 기본 true — 자동 입력 실패 시 수동 붙여넣기 폴백.
   자동 일괄 녹음만 false: 무인 실행이라 붙여넣을 사람이 없는데 대본마다 복사하면 사용자 클립보드를 대본 수만큼 덮어쓴다)
- `typecast_play()` / `typecast_stop_playback()` ➔ `Result<(), String>`
- `typecast_probe()` ➔ `Result<(), String>` (편집기 · 재생 버튼 탐색 진단)
  > 자동화 선택자는 `TypecastController::apply_automation_options()` 가 조작 직전마다 다시 심는다
  > (`window.__omnirecSetOptions`). 페이지가 다시 로드되면 주입 스크립트가 기본값으로 돌아가므로
  > 한 번만 심어 두면 안 된다.
- `copy_text_to_clipboard(text: String)` ➔ `Result<(), String>`
- `get_last_recorded_path()` ➔ `Option<String>`

### 7. History & Files
- `list_history_files()` ➔ `Vec<HistoryItem>`
- `delete_history_file(path: String)` ➔ `Result<(), String>`
- `rename_history_file(old_path: String, new_name: String)` ➔ `Result<String, String>`
- `read_audio_file(path: String)` ➔ `Result<Vec<u8>, String>`
- `open_in_explorer(path: String)` ➔ `Result<(), String>`
- `open_with_default_player(path: String)` ➔ `Result<(), String>`

### 8. Window & Overlay Controls
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
| `auto_stop_triggered` | `Option<String>` | Emitted when silence duration exceeds threshold and recorder auto-stops. Payload is the saved output file path. 수동 정지가 먼저 전이를 가져간 경우에는 발행하지 않는다(중복 처리 방지). |
| `recording_failed` | `String` | 캡처/인코딩이 죽어 녹음이 중단됐다(장치 제거 · 미지원 포맷 · FFmpeg 파이프 오류 · 비정상 종료 코드). 프론트엔드는 상태를 idle 로 되돌리고 사용자에게 알린다. **이 이벤트 없이 조용히 넘어가면 UI 는 계속 "녹음 중"이고 빈 파일이 정상 결과로 히스토리에 올라간다.** |
| `typecast_navigation` | `TypecastNavigationPayload` | Typecast 브라우저 창의 URL 이 바뀔 때마다 발행(로그인 여부 추정 포함). |
| `typecast_browser_closed` | `None` | Typecast 브라우저 창이 닫혔을 때 발행. |
| `typecast_popup_intercepted` | `TypecastPopupPayload` | 새 팝업/탭이 열렸을 때 발행(진단용 — 팝업 자체는 건드리지 않는다, 실제 브라우저의 네이티브 로그인 팝업). |
| `typecast_step` | `TypecastStepPayload` | 페이지 자동화 단계 보고(`prepared`, `prepare-failed`, `playing`, `play-failed`, `stopped`, `probe`, `media-play`, `media-ended`, `media-pause`). |
| `typecast_debug` | `TypecastDebugPayload` | 연동 진단 로그(네비게이션 · 네이티브 팝업 · 브리지 메시지). |

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
   - 일시정지 사유는 **수동/자동을 분리 보관**하고 실효 일시정지 = 둘의 OR 이다. 하나로 합치면
     무음 자동 재개가 사용자의 수동 일시정지를 되돌린다(컨트롤러는 Paused 인데 파일에는 계속 기록).
6. **FFmpeg Audio Pipe**:
   - Mixed audio is streamed as raw `f32le` stereo interleaved PCM to FFmpeg process `pipe:0` (stdin).
   - **실시간 캡처 콜백 → 워커 채널은 유계다.** 소비자(FFmpeg 파이프)가 막히면 무한 채널은
     메모리를 무한히 먹는다. 가장 오래된 프레임을 버리고 버린 수를 세어 진단에 남긴다.
7. **`TimelineMode` — 일시정지의 의미가 녹음 종류마다 다르다** (`audio/engine.rs`):
   - `SkipPaused`(오디오 전용): 일시정지한 시간은 결과 파일에 **담지 않는다**. 무음 자동
     일시정지로 무음을 걷어내는 기능이 이 의미에 의존한다.
   - `WallClock`(화면 녹화): 일시정지 구간과 캡처가 멈춘 구간을 **무음으로 메운다**. 화면
     녹화는 `-shortest` 로 종료하는데(비디오 입력은 스스로 끝나지 않아 이게 유일한 크로스
     플랫폼 graceful stop 레버다), 오디오가 벽시계보다 짧으면 그만큼 영상 뒤가 잘린다.
   - 두 모드를 하나로 합치지 말 것 — 한쪽을 고치면 다른 쪽이 깨진다.
8. **실패는 반드시 위로 올린다**:
   - 장치 없음 · 기본 설정 읽기 실패 · 미지원 샘플 포맷(cpal `U16` 등) · `build_input_stream`/
     `play()` 실패 · 오디오 전용인데 소스가 하나도 없음 → `AudioCaptureEngine::start()` 가 `Err`.
     cpal `Stream` 은 `!Send` 라 워커 안에서만 만들 수 있으므로, 워커가 준비 결과를 채널로
     돌려주고 `start()` 가 그것을 기다린다.
   - 실행 중 발생한 치명적 실패(스트림 에러 콜백 · FFmpeg stdin 쓰기 실패)는
     `AudioEngineEvent::Fatal(String)` 으로 **세션당 정확히 한 번** 보고한다.
   - `stop()` 계열에 무한 대기가 없어야 한다. 워커가 커널 파이프 쓰기에 막혀 있으면 정지
     플래그를 볼 수 없으므로, 완료 플래그를 **데드라인 폴링**한 뒤에만 `join` 하고, stdin 이
     닫히지 않았으면 기다리지 않고 FFmpeg 을 kill 해 파이프를 깨워 수거한다. `join()` 을
     플래그 확인 없이 부르는 코드로 되돌리지 말 것 — 동기 커맨드였다면 앱이 통째로 멈춘다.

---

## 🎞️ FFmpeg Process & Platform Dispatch (`src-tauri/src/recorder/`)

### Screen Capture Driver Matrix
- **Windows**: `-f gdigrab -framerate {fps} -draw_mouse 1 [-offset_x {x} -offset_y {y} -video_size {w}x{h}] -i desktop`
- **macOS**: `-f avfoundation -framerate {fps} -capture_cursor 1 -i "1:none" [-vf crop={w}:{h}:{x}:{y}]`
- **Linux**: `-f x11grab -framerate {fps} -draw_mouse 1 [-video_size {w}x{h} -i :0.0+{x},{y}]`

**`RectRegion` 은 가상 데스크톱 전역 물리 좌표다** (모니터 로컬이 아니다).
오버레이가 `SelectionScreenInfo.physical_x/y`(모니터 원점)를 더해서 보내고, `start_screen_record`
가 그 좌표가 나온 모니터 정보(`AppState.last_selection_screen`)를 함께 `RecorderController::start_screen`
→ `ScreenRecorderSession::start(settings, region, screen, tx)` 로 넘긴다.
- `gdigrab -offset_x/-offset_y` 와 `x11grab :0.0+x,y` 는 전역 좌표를 그대로 쓴다.
- `avfoundation` 의 `crop` 은 **캡처 대상 디스플레이 로컬 좌표**라 모니터 원점을 뺀다. macOS 는
  `-i "1:none"` 으로 주 디스플레이만 캡처하므로, 원점이 (0,0) 이 아닌 모니터(=보조 모니터) 선택은
  조용히 엉뚱한 영역을 녹화하지 않도록 **에러로 거부한다**.
- 원점은 음수일 수 있다(주 모니터 왼쪽/위 모니터). 프론트·백엔드 어디서도 0 으로 클램프하지 말 것.
- `(w/2)*2` 짝수 강제는 유지한다(libx264 + yuv420p).

### `RecorderController` 잠금 규칙 (`src-tauri/src/recorder/mod.rs`)

**어떤 잠금이든 잡은 채로 `status_snapshot()` / `emit_status()` 를 부르지 말 것.**
`parking_lot::Mutex` 는 재진입이 불가능하다. `pause()`/`resume()` 이 `status` 가드를 들고 상태
스냅샷을 만들다 자기 자신을 영구히 기다렸고, `pause_record` 는 동기 커맨드(= Tauri 메인 스레드)라
**앱 전체와 모든 웹뷰가 함께 얼었다**(초기 커밋부터 있던 버그). 전이는 잠금 안에서, 알림은 잠금을
놓은 뒤에. 회귀 테스트: `recorder::tests::pause_and_resume_do_not_deadlock_with_an_app_handle_attached`
(`AppHandle` 이 `None` 이면 `emit_status()` 가 조기 반환해 재현되지 않으므로 mock 앱 핸들이 필수다).

**잠금 순서는 항상 `status` → `session`.** 두 잠금을 동시에 잡는 곳이 시작 경로와 정지 경로 둘인데,
순서를 뒤집으면 ABBA 교착이 난다.

**시작은 상태가 정확히 `Idle` 일 때만 허용한다.** 세션 유무만 보면 정지 처리가 끝나기 전
(`Stopping`)에 새 녹음이 끼어들고, 뒤늦게 끝난 정지 경로가 살아 있는 세션의 공유 상태를 Idle 로
지운다(그 뒤 정지는 이전 결과 경로를 반환하고 새 FFmpeg 은 고아가 된다).

**상태 스냅샷을 만드는 곳은 `SharedState::status_snapshot()` 하나다.** 예전에는 커맨드·틱커·자동
일시정지/재개/종료가 각자 `RecordingStatus` 를 만들어, 자동 일시정지 payload 의 `duration_secs`
가 `0.0` 으로 하드코딩돼 타이머가 0 으로 튀었다.

**정지·자동종료·치명적 실패는 `SharedState::finish_active_session()` 한 곳을 쓴다.** 성공이든
실패든 상태는 반드시 `Idle` 로 되돌아간다 — 여기서 에러로 조기 반환하면 `Stopping` 에 박혀 이후
어떤 녹음도 시작할 수 없다.

**FFmpeg 종료 코드를 확인하지 않고 "출력 파일이 존재한다"로 성공을 판정하지 말 것.** FFmpeg 은
인코딩 시작 전에 목적지를 만들므로 비정상 종료 후에도 파일이 남는다(실측: `-ac 3` + libmp3lame →
exit 234, 0바이트 파일 생성, `progress=end` 까지 찍힘). stderr 는 파이프로 잡아 꼬리 20줄만
링버퍼로 보관하고(안 읽으면 파이프가 차서 FFmpeg 이 멈춘다. `\r` 도 줄 구분자로 처리 — 진행 로그는
`\n` 이 없다) 실패 메시지에 싣는다. 단, **녹음기는 크기가 있는 부분 파일을 지우지 않는다**
— 재현 불가한 사용자의 유일한 사본이다. 변환기/병합기는 입력이 남아 있으니 삭제한다.

### Multi-Window Setup (`tauri.conf.json`)
- `main`: Primary application window.
- `selection-overlay`: Transparent, fullscreen, frameless window for drag-to-crop region selection.
- `mini-controller`: Compact, frameless, always-on-top floating toolbar displayed during active screen recordings, and during TTS 낭독 녹음 (`start_audio_record` 의 `show_mini_controller` 옵션).

Typecast 는 Tauri 창이 아니다 — `typecast-browser`/`typecast-popup` 이라는 이름의 앱 창은 없다.

### Typecast 자동화 — 실제 Chrome + CDP (`src-tauri/src/tts/mod.rs`)

Typecast(`studio.typecast.ai`)는 앱 내장 웹뷰가 아니라 **사용자가 실제로 보는 별도의
Google Chrome 프로세스**를 Chrome DevTools Protocol(CDP, `chromiumoxide` 크레이트)로 띄우고
제어한다. WKWebView 로 임베드하던 이전 구현에서 이쪽으로 바꾼 이유:

1. WKWebView 는 창이 가려지거나 최소화되면 배터리 절약을 위해 그 프로세스를 스로틀링/서스펜드해,
   재생 중인 오디오가 정지 버튼을 누르지 않았는데도 멈추는 사고가 있었다.
2. WKWebView 는 사용자 제스처와 분리된 `window.open` 을 차단해, Typecast 의 소셜 로그인
   (팝업 + `window.opener.postMessage`)을 흉내 내려면 프록시 window 객체 + opener 스텁으로
   된 400줄 넘는 우회 코드가 필요했다.
3. 실제 Chrome 은 이 둘 다 겪지 않는다. **팝업 로그인은 아무 코드 없이 그대로 동작한다** —
   진짜 `window.opener` 관계가 유지되므로 Typecast 자신의 `postMessage`/`popup.close()` 코드가
   손대지 않아도 정상 동작한다.

**세션·프로필**: `SettingsManager::typecast_chrome_profile_dir()` (`~/.omnirec/typecast-chrome-profile`)
가 로그인 쿠키를 영구 보관하는 앱 전용 Chrome 프로필이다. 사용자의 평소 개인 Chrome 프로필과는
**절대 공유하지 않는다** — Chrome 136+ 는 기본 프로필에 대한 원격 디버깅 자체를 거부하기도 하고,
자동화가 사용자의 실제 로그인 세션에 손대는 것도 피해야 한다. Chrome 실행 파일은
`SettingsManager::find_chrome()` 이 OS별 기본 설치 위치를 자동 탐색하며, 안 잡히면
설정의 `custom_chrome_path` 로 직접 지정할 수 있다(TtsBatchRunner 고급 설정 패널).

**세션 상태(`TypecastCdpState`)**: `AsyncMutex<Option<CdpSession>>` 하나로 앱 전체에 세션을
최대 하나만 유지한다(단일 Typecast 탭 모델). `CdpSession` 은 `Browser`, `main_page: Page`,
그리고 백그라운드로 계속 폴링해야 하는 태스크 3~4개(핸들러 루프 · 브리지 바인딩 이벤트 ·
네비게이션 이벤트 · 팝업 감지)를 들고 있다. `Page` 는 `Clone`(내부 `Arc`)이라 명령마다
짧게 락을 잡고 클론해서 쓰고 바로 락을 놓는다 — `Browser` 는 `Clone` 이 아니라서 `close()`/`wait()`
처럼 소유권이 필요한 조작만 세션을 통째로 `take()` 한 뒤에 한다.

**세션이 죽었는지는 객체가 아니라 응답으로 판정한다.** 사용자가 Chrome 창을 직접 닫거나
프로세스가 죽어도 `TypecastCdpState` 에는 `CdpSession` 객체가 그대로 남는다. 이걸 "열려 있음"
으로 취급하면 다음 `open()` 이 죽은 페이지에 `bring_to_front()` 를 걸고, CDP 응답이 올 리 없어
커맨드 타임아웃까지 멈췄다가 **창은 뜨지도 않은 채** 끝난다(실측 증상: 한 번 녹음한 뒤 창을
닫고 다시 시작하면 브라우저가 안 뜬다). 세 겹으로 막는다.

1. `handler_task` 가 이벤트 스트림 종료(= 연결 끊김)를 감지하면 `discard_dead_session()` 으로
   세션을 **Rust 쪽에서도** 버린다. 프론트엔드의 `typecast_browser_closed` 처리만으로는
   Rust 상태가 남는다.
2. `open()`/`state()` 는 `live_main_page()` 로 짧은 CDP 왕복(5초)을 먼저 던져 살아 있는지
   확인하고, 응답이 없으면 세션을 버린 뒤 새로 띄운다.
3. `CdpSession::shutdown()` 의 `close()`/`wait()` 도 시간 제한을 둔다 — 이미 죽은 브라우저를
   닫으려다 여기서 멈추면 다음 `open()` 이 상태 잠금을 못 얻는다.

**상태 잠금(`TypecastCdpState`)을 CDP `await` 너머로 들고 있지 말 것.** 죽은 세션의 응답을
기다리는 동안 잠금을 붙들면, 연결이 끊겨 세션을 정리하려는 handler 태스크가 그 잠금을 못 얻어
서로 영원히 기다린다. `Page` 를 클론해 잠금을 놓은 **뒤에** 왕복할 것. `Browser` 소유가 필요한
조작(`clear_cookies` 등)은 세션을 통째로 `take()` 한 뒤 잠금을 놓고 작업하고, 끝나면 슬롯이
비어 있으면 되돌려 놓는다(그 사이 누가 새 세션을 설치했으면 우리 것을 shutdown 한다).

**세션에는 세대(`CdpSession::id`)가 있고, 정리는 세대가 일치할 때만 한다.** `discard_dead_session`
이 슬롯에 있는 것을 무조건 `take()` 하던 시절에는, 뒤늦게 끝난 옛 handler 태스크가 **갓 만든 새
세션을 지우고 그 백그라운드 태스크 4개를 abort** 했다. `open`/`close`/`clear_session` 전이 자체는
별도 `transition` 잠금으로 직렬화한다 — 잠금 순서는 `transition` → `session` 이고, `transition` 은
CDP `await` 를 넘어도 되지만 `session` 은 절대 안 된다.

**`open()` 이 세션을 설치할 때 슬롯이 차 있으면 이전 것을 반드시 shutdown 한다.** 그냥 덮어쓰면
브라우저 프로세스와 태스크 4개가 누출된다.

**`navigate_and_wait()` 는 새 문서가 커밋됐는지 확인해야 한다.** `location.replace()` 직후에는
아직 옛 문서가 살아 있어 `readyState === 'complete'` 가 첫 폴링에서 통과한다 — 이동 전에 심은
표식(`window.__omnirecNavToken`)이 사라졌는지까지 봐야 한다. 개별 `evaluate` 도 남은 데드라인으로
감쌀 것(그러지 않으면 왕복 하나가 45초 예산을 넘어 무한정 멈춘다). `page.goto()`/`Page.navigate`
는 여전히 금지(하드코딩 30초 타임아웃).

**재접속한 브라우저는 검증하고 쓴다.** `Browser::connect` 성공은 "살아 있다"는 뜻이 아니다.
`Browser.version()` 왕복을 **Handler 를 직접 굴리며**(아직 폴링 태스크가 없으므로) 유계 시간 안에
확인하고, 실패하면 락 파일 정리 → 재실행 경로로 이어간다(`close()` 를 부르지 말 것 — 응답 없는
브라우저에서 멈춘다).

**브리지 페이로드는 유계다.** 바인딩은 원격 페이지에 노출돼 있다. 16KB 넘는 메시지는 파싱 전에
버리고, emit 하는 step 이름/상세도 UTF-8 경계에서 잘라 상한을 둔다.

**페이지 → 앱 브리지**: CDP `Runtime.addBinding("__omnirecBridge")` 하나로 단순화됐다. 원격
오리진이라 Tauri IPC 가 막혀 있던 WKWebView 시절에는 `document.title` 변경 + 커스텀 스킴
네비게이션이라는 이중 채널과 일련번호 중복 제거가 필요했지만, CDP 바인딩은 페이지 JS 가 직접
호출하는 진짜 함수라 그런 우회가 필요 없다. 바인딩은 최상위 실행 컨텍스트에서만 보장되므로,
주입 스크립트의 `report()` 는 최상위 프레임에서만 `window.__omnirecBridge` 를 직접 부르고
서브프레임은 여전히 `postMessage` 로 top 에 중계한다(이 부분은 그대로 유지했다).

**팝업 감지는 진단용일 뿐, 팝업 자체를 조작하지 않는다.** `Browser::event_listener::<EventTargetCreated>()`
로 새 탭/창이 뜨는 것만 관찰해 `typecast_debug`/`typecast_popup_intercepted` 를 보고한다.
`window.open` 오버라이드, opener 스텁, `popup.close()` 가로채기 같은 코드를 다시 넣지 말 것 —
진짜 브라우저에서는 전부 불필요하고, 오히려 사이트 동작을 방해할 위험만 늘린다.

**포커스는 두 단계다.** `Page::bring_to_front()` 는 Chrome **탭**만 활성화할 뿐, 다른 앱 뒤에
있는 Chrome **창**을 OS 레벨로 끌어올리지는 못한다. 그래서 `activate_chrome_app()` 이
`open -a "Google Chrome"` 을 셸아웃해 앱 자체를 최전면으로 올린 뒤 `bring_to_front()` 로 탭을
맞춘다(macOS 전용, 다른 OS 는 no-op).

**자동재생 정책**은 Chrome 실행 인자 `--autoplay-policy=no-user-gesture-required` 로 끈다
(WKWebView 시절 `mediaTypesRequiringUserActionForPlayback = None` 과 같은 목적).

**`page.goto()`(`Page.navigate`)를 쓰지 말 것 — 하드코딩된 30초 타임아웃이 있다.**
chromiumoxide 0.9.1 은 `Page.navigate` 요청에 `FrameNavigationRequest::new()` 가 항상
`REQUEST_TIMEOUT`(30초) 상수를 그대로 박아 쓴다 — `BrowserConfig::request_timeout()` 빌더로
늘릴 수 없다(그 설정은 다른 일반 커맨드 왕복에만 적용됨). Typecast 처럼 분석 스크립트 등
3rd-party 리소스가 낀 무거운 SPA 는 `load` 이벤트가 30초를 넘기기 쉬워, 실제로 스모크
테스트(`tts::tests::real_chrome_cdp_round_trip`, 아래)에서 `studio.typecast.ai/sign-in` 이동이
매번 정확히 30초에 `CdpError::Timeout` 으로 실패하는 것을 확인했다. `TypecastController::navigate_and_wait()`
가 우회로다 — `Page.navigate` CDP 커맨드 자체를 보내지 않고 `location.replace()` JS 로 이동시킨
뒤 `document.readyState` 를 우리가 직접 폴링해 타임아웃도 우리가 정한다(45초). `open()` /
`navigate()` / `clear_session()` 모두 이 헬퍼를 쓴다 — 새 네비게이션 경로를 추가할 때
`page.goto()` 를 다시 쓰지 말 것.

**`CdpSession::shutdown()` 의 순서를 바꾸지 말 것.** `browser.close()` 는 CDP 요청/응답
왕복이라 `handler_task`(연결을 실제로 읽는 폴링 루프)가 계속 돌고 있어야 응답을 받는다.
`handler_task.abort()` 를 `close()`/`wait()` **보다 먼저** 부르면 응답을 영원히 못 받아
`close_typecast_browser` 커맨드가 멈춘다 — 실제로 스모크 테스트에서 이 순서로 짰다가
재현했다. 항상 `close()` → `wait()` → (그 다음에) 태스크 정리 순서를 지킬 것.

**같은 프로필로 두 번 실행하면 `SingletonLock` 충돌이 난다.** Chrome 프로필 디렉터리는
한 번에 프로세스 하나만 열 수 있다. 앱이 재시작/충돌하는 사이에도 이전 Chrome 이 살아있거나,
비정상 종료로 락 파일만 남으면 `Browser::launch` 가 `Failed to create .../SingletonLock:
File exists` 로 실패한다(실측 확인 — 사용자가 그대로 겪음). `TypecastController::launch_with_recovery()`
가 이 경우를 처리한다: (1) `DevToolsActivePort` 파일에서 포트를 읽어 이미 떠 있는 Chrome 에
먼저 재접속을 시도하고, (2) 그것도 안 되면 죽은 프로세스가 남긴 락으로 보고
`SingletonLock`/`SingletonCookie`/`SingletonSocket` 을 지운 뒤 한 번 더 실행한다. `open()` 에서
`Browser::launch` 를 직접 부르지 말고 반드시 이 헬퍼를 거칠 것. 스모크 테스트:
`tts::tests::recovers_from_live_singleton_lock`(`#[ignore]`).

**`commands.rs` 의 모든 `typecast_*` 커맨드는 `with_typecast_timeout()` 으로 감싸져 있다.**
chromiumoxide 의 CDP 요청/응답이 (드물지만) 영원히 안 돌아오는 경우가 있다 — 이러면 그
커맨드를 부른 `await` 가 무한정 멈추고, `TtsBatchRunner` 의 for-loop 전체가 그 자리에서
멈춰 그 대본의 `onStopRecord()` 조차 실행되지 못한다(실제 증상: 대본 3개 중 마지막에서
멈추고 녹음 종료 처리도 안 됨). 새 `typecast_*` 커맨드를 추가할 때도 반드시 이 타임아웃으로
감쌀 것 — 프론트엔드의 기존 실패 처리(건너뛰기/중단 선택)가 그 위에서 동작한다.

**실제 Chrome + CDP 스모크 테스트**: `tts::tests::real_chrome_cdp_round_trip` (`#[ignore]`, 일반
`cargo test` 에는 안 돎). 이 머신에 설치된 실제 Chrome 을 띄우고, 프로덕션과 똑같은 순서로
바인딩·초기화 스크립트를 등록한 뒤 `studio.typecast.ai/sign-in` 으로 이동해 실제
`MAIN_INIT_SCRIPT` 가 주입됐는지, `__omnirecProbe()` → `step:probe:` 브리지 왕복이 되는지
확인한다. 로그인은 필요 없다.
`cargo test --manifest-path src-tauri/Cargo.toml --lib tts::tests::real_chrome_cdp_round_trip -- --ignored --nocapture`
로 수동 실행.

**로그인 포함 종단 테스트**: `tts::tests::real_login_and_prepare_flow` (`#[ignore]`). **프로덕션과
같은 프로필**(`typecast_chrome_profile_dir`)을 그대로 써서 실제 대본 입력(`__omnirecPrepare`)과
재생(`__omnirecPlay`) 까지 확인한다. 처음 뜬 Chrome 창에서 사용자가 직접 로그인한 뒤 테스트가
떠 있는 터미널에서 Enter 를 눌러야 진행된다(URL 만으로는 로그인 여부를 정확히 판단할 수 없어
URL 휴리스틱 대신 사람 확인을 기다린다 — `hub send` 로 표준입력에 아무 텍스트나 보내면 됨).
같은 프로필을 재사용하므로 한 번 로그인해 두면 다음 실행부터는 바로 Enter 를 보내도 된다.
`cargo test --manifest-path src-tauri/Cargo.toml --lib tts::tests::real_login_and_prepare_flow -- --ignored --nocapture`
로 수동 실행. Chrome 자동화 관련 코드(특히 `MAIN_INIT_SCRIPT`, `doPrepare`, `doPlay`)를 고칠 때마다
이 테스트로 먼저 확인할 것 — 이 문서의 Slate 관련 함정들은 전부 이 테스트로 실측한 것이다.

#### 자동 일괄 녹음 파이프라인 (`TtsBatchRunner`)

대본마다 아래 상태 기계를 순서대로 돌린다. 오케스트레이션은 프론트엔드가 맡고, Rust 는 페이지 조작과 녹음만 담당한다.

```
preflight (루프 전에 한 번)
         resolve_script_recording_targets → ① 제목이 같은 파일로 저장되는 대본이 있으면
           시작 거부(뒤 대본이 앞 대본 결과를 조용히 덮어쓴다) ② 이미 있는 파일은
           한 번에 모아서 덮어쓰기 확인
         get_typecast_browser_state → 창이 열릴 때까지 1초 간격 확인 + looks_signed_in 확인
prepare  typecast_prepare_script  → step:prepared / step:prepare-failed (10s 타임아웃)
         (대본 전체를 한 번에 넣는다. 입력 후 선택 해제 + 캐럿을 본문 맨 앞으로 —
          Typecast 는 커서 위치부터 낭독하므로 이 처리가 없으면 소리가 나지 않는다)
settle   EDITOR_SETTLE_MS(2초) 대기 — `step:prepared` 는 DOM 에 글자가 들어간 것만 확인한다.
         React + Slate 는 그 뒤에도 내부 상태 반영 · 재생 버튼 활성화를 이어서 하므로 바로
         재생을 누르면 이전 대본이 읽히거나 재생이 먹지 않는다. **녹음 시작 전에** 기다려
         이 대기 시간이 결과 파일 앞부분의 무음으로 들어가지 않게 한다.
record   start_audio_record       → 저장 경로 확보 (auto_stop 은 끄고 직접 판정)
play     focus_typecast_browser → typecast_play → step:playing / step:play-failed (12s)
speak    audio_vu_meter 구독      → sys_level > 임계값이면 시작, 무음 N초 지속되면 종료
         (단락/화자 전환으로 오디오가 재생성되는 동안의 무음은 `media-ended` 신호로,
          Typecast 사이트가 스스로 재생을 멈추는 오동작은 `media-pause` 신호로 들어온다.
          둘 다 재생 버튼을 다시 누르는 등 추가 개입 없이 `segmentGapMs`(무음 판정의
          2배, 최소 8초) 만큼 종료 판정만 미룬다 — TtsBatchRunner.tsx)
save     stop_record → attach_script_recording → 다음 대본 (gap 초 대기)
```

**단계 보고 구독은 상시로 두고 일련번호로 "이 시점 이후"를 판정할 것.** `invoke()` 를 보낸
**뒤에** `listen('typecast_step')` 을 거는 방식으로 되돌리지 말 것 — `listen()` 등록 자체가
비동기 왕복이라 그사이 도착한 보고를 놓친다. 특히 페이지 스크립트가 **동기적으로** 내는
실패 보고(`fillEditor` 예외 → `step:prepare-failed:입력 중 오류 …`)는 항상 유실돼, 진짜 원인
대신 10초 뒤 "페이지 응답 시간 초과"만 보였다. `markSteps()` 로 `invoke` **전에** 기준점을
찍고, `waitForStep(…, since)` 이 이미 도착한 로그부터 훑은 뒤 구독한다.
**구독 등록 자체도 기다려야 한다** — 상시 구독이라도 `listen()` 의 프라미스가 아직 안 끝났으면
네이티브 리스너가 없어 동기 보고를 놓친다. `ensureListenersReady()` 로 첫 Typecast `invoke`
전에 `typecast_step` · `audio_vu_meter` 등록 완료를 `await` 한다.

**녹음이 조용히 죽는 경우도, 하드캡 도달도 성공이 아니다.** `audio_vu_meter` 가 3초 이상 끊기면
(FFmpeg 종료 · 오디오 스트림 오류 · 바깥에서 정지) 녹음 세션이 사라진 것이다. 그리고
`hardCapMs` 도달은 **낭독이 끝났다는 신호가 아니라 판정에 실패했다는 신호**이므로 `ok: false` 로
처리한다 — 예전에는 `ok: true` 로 보고해 잘린 파일을 대본에 연결하고 "완료"로 표시했다. 실패
항목의 녹음 파일 경로는 상세에 남겨 사용자가 직접 확인·복구할 수 있게 하되, 자동으로 대본에
연결하지 않는다.

**정지가 실패하면 녹음 소유권을 놓지 말 것.** `recordingActiveRef` 를 `await onStopRecord()`
**전에** 내리면, 정지가 거부됐는데 플래그는 false 라 이후 모든 정리가 no-op 이 되고 살아 있는
녹음이 계속 파일을 쓴다. 정지가 성공했을 때만 소유권을 놓고, 실패하면
`invoke('get_recording_status')` 로 실제 상태를 확인해 아직 녹음 중이면 소유권을 유지한다.
중복 정지는 in-flight 프라미스 하나로 직렬화한다.

**`attach_script_recording` 실패를 '완료'로 표시하지 말 것.** 예전에는 `console.error` 만 남기고
상태를 `done` 으로 만들어, 라이브러리에 연결도 안 된 결과가 성공으로 보였다.

**모달을 루프 안에서 띄우지 말 것.** 덮어쓰기 확인을 대본마다 하면 Chrome 창 뒤에서 배치가
멈춘 것처럼 보이고 대본 수만큼 모달이 반복된다. 시작 전에 한 번에 확인하고
(`skipOverwriteCheck: true`), 녹음 시작/저장 실패는 `throwOnError` / `onStopRecord({silent:true})`
로 받아 `alert` 대신 항목 옆에 인라인으로 보여준다.

**빠져나오는 모든 경로에서 큐와 녹음을 정리할 것.** `runBatch` 는 `try/finally` 로 감싸
남은 항목을 '건너뜀'으로 마무리하고 `stopRecordingIfActive()` 를 부른다(같은 플래그를
중단 버튼도 쓰므로 정지가 두 번 나가지 않는다). 사용자가 중단해 끝난 항목은 '실패'가 아니라
'건너뜀'이다(`StepResult.aborted`).

Typecast 의 3,000자 제한은 **문단(줄) 하나의 최대 길이**이지 한 번에 넣을 수 있는 전체 길이가 아니다. 대본은 통째로 넣는다. 분할 로직을 다시 넣지 말 것.

**단락 전환 · 사이트 오동작으로 인한 무음을 낭독 종료로 오판하지 않도록 할 것. 동시에 Typecast
사이트 동작에 대한 개입은 최소로 유지할 것.** 무음 원인은 두 가지다.
(1) Typecast 는 화자/단락이 바뀔 때 다음 오디오를 새로 생성하느라 잠깐 재생이 끊긴다 — 정상 동작.
(2) Typecast 사이트가 낭독 도중 재생을 스스로 멈추는 오동작이 있다 — `ended` 없이 `pause` 만 온다.
둘 다 이 무음이 `tts_auto_stop_seconds` 를 넘기면 `waitForSpeechCycle` 이 낭독이 끝난 것으로 오판해
대본 중간에서 녹음을 끝내고 다음 대본으로 넘어간다. `hookMedia` 가 `play`/`ended`/`pause` 를 구분해
보고하고(`step:media-play` / `step:media-ended` / `step:media-pause`), 배치 러너는 둘 다 재생 버튼을
다시 누르는 등의 추가 조작 없이 `segmentGapMs` 만큼 종료 판정만 미루고 재생이 스스로 돌아오길
기다린다. **낭독 도중에 재생 버튼을 자동으로 재클릭해 "이어가기"를 시도하는 방식은 쓰지 않는다.**
이미 재생 중인데 또 클릭하면 토글 버튼이 꺼져버려 오히려 우리가 재생을 멈추는 원인이 될 수 있다
(정지 버튼을 누르지 않았는데 소리가 멈추는 증상의 실제 원인이었다). 우리가 `doStop` 으로 직접
멈춘 pause 는 `intentionalStop` 플래그로 걸러 오동작으로 보고하지 않는다. 이 유예 없이 `silenceMs` 자체를 늘리는
식으로 고치면 진짜 낭독 종료 판정도 그만큼 느려지므로 반드시 구분해서 다룰 것.

**입력 직후 캐럿을 반드시 맨 앞으로 되돌릴 것.** `execCommand('insertText')` 는 캐럿을 본문 끝에 남기는데, Typecast 는 커서 위치부터 낭독하므로 그대로 재생하면 아무 소리도 나지 않거나 마지막 부분만 읽는다. `collapseCaretToStart()` 를 입력 직후와 재생 직전 두 번 호출한다.

#### Typecast 편집기는 Slate.js 다

DOM 구조가 평범한 `contenteditable` 이 아니라 아래처럼 생겼다. 자동화 코드를 고칠 때 이 구조를 전제로 할 것.

```html
<div role="textbox" data-slate-editor="true" contenteditable="true">
  <div data-slate-node="element">            <!-- 단락 하나 -->
    <div contenteditable="false">            <!-- 화자 선택 버튼 (편집 불가) -->
      <button class="actor-selector">…필재…</button>
    </div>
    <p><span data-slate-node="text"><span data-slate-leaf="true">
      <span data-slate-string="true">실제 대본 텍스트</span>
    </span></span></p>
  </div>
  …단락 반복…
</div>
```

여기서 나오는 세 가지 함정:

1. **`textContent` 로 본문을 읽으면 안 된다.** 단락마다 붙은 화자 이름("필재")이 섞여 들어온다. `readEditor()` 는 `[data-slate-string="true"]` 만 모아 읽는다.
2. **캐럿을 첫 텍스트 노드에 놓으면 안 된다.** 첫 텍스트 노드는 `contenteditable="false"` 인 화자 버튼 안에 있다. `collapseCaretToStart()` 는 첫 `[data-slate-string="true"]` 노드를 찾아 그 앞에 놓고, 없으면 편집 불가 서브트리를 걸러내는 TreeWalker 로 되돌아간다.
3. **빈 줄은 빈 단락이 된다.** 소리 없는 단락과 화자 선택 UI만 늘어나므로 `cleanScript()` 로 미리 걷어낸다(프론트엔드 `scriptChunks.ts` 에도 같은 정리 로직이 있다).

4. **`execCommand('selectAll')` 은 문단 하나만 선택할 수 있다.** 그 상태로 붙여넣으면 첫 문단만 새 대본으로 바뀌고 이전 대본의 나머지 문단이 그대로 남는다. `selectAllIn()` 은 첫 `[data-slate-string]` 텍스트 노드부터 마지막 노드까지 범위를 직접 만들고, `doPrepare()` 는 붙여넣기 **전에** `clearEditor()` 로 편집기를 완전히 비운다(최대 4회 반복).
5. **`execCommand('delete')` 로 지우지 말 것 — Slate 의 "문단 최소 1개" 불변식이 깨진다.** 실측: 전체 선택 후 `execCommand('delete')` 를 반복하면 문단이 화자 선택 버튼까지 통째로 사라져 `[data-slate-node="element"]` 가 0개가 되는 경우가 있었다. 이 상태가 되면 Slate 가 이 DOM 과 완전히 어긋나 이후 어떤 `execCommand`(`insertText` 포함)로도 복구되지 않고 — 붙여넣기가 조용히 사라진다("입력 확인 실패"). `execCommand` 는 Slate 의 `onKeyDown` 컨트롤을 거치지 않고 DOM 을 직접 바꾸기 때문이다. `clearEditor()` 는 대신 진짜 `KeyboardEvent('keydown', { key: 'Backspace', ... })` 를 보낸다 — Slate 가 이 키를 자기 핸들러에서 가로채 자신의 delete 트랜잭션으로 처리하므로 불변식이 유지된다. **`execCommand` 기반 삭제/삽입으로 되돌리지 말 것.**

**Typecast 는 프로젝트 기반이다 — `typecast_editor_url` 이 에디터가 아니라 프로젝트 목록으로 갈 수 있다.**
실측(2026-08): 기본 URL(`studio.typecast.ai/text-to-speech`)이 에디터가 아니라 "새 프로젝트 / 대본
가져오기 / 새 폴더" 가 있는 프로젝트 목록 화면으로 감. `findEditor()`/`findPlayButton()` 이 못 찾으면
(즉 `editor`/`button` 이 `null`) `doPrepare()`/`doPlay()` 가 `enterFirstProjectAndRetry()` 를 호출해
목록의 **첫 번째(가장 최근) 프로젝트** 링크(`a[href*="/text-to-speech/"]`)를 클릭하고 2초 뒤 **한 번만**
재시도한다. **새 프로젝트를 만들지 않는다** — 사용자가 이미 갖고 있는 프로젝트를 그대로 쓴다(재시도
횟수를 `attempt < 1` 로 제한해 무한 재귀도 막는다). 이 로직을 지울 때는 목록 화면 자체가 없어졌는지
먼저 확인할 것 — 사이트가 다시 개편되면 이 부분이 가장 먼저 깨진다.

입력 검증은 **정규화 후 완전 일치**다. "앞 40자가 어딘가 있다 + 기대보다 20자 넘게 길지 않다"는
예전 휴리스틱에는 **하한이 없어**, 3,000자 대본에서 앞 50자만 들어가도 `step:prepared` 를 내고
앞부분만 낭독한 파일이 정상 완료로 처리됐다. 불일치하면 기대/실제 길이 · 첫 불일치 인덱스 ·
양쪽 발췌를 실어 원인을 3분류(앞부분만 들어감 / 이전 대본 잔류 / 내용 어긋남)해 보고한다.
`normalize()` 가 흡수하는 것은 공백·개행(Slate 단락 렌더링), 제로폭 스캐폴딩 문자, 한글 NFC
통일 셋뿐이다 — 관대한 오차 허용으로 되돌리지 말 것.

편집기 입력은 **`paste`(ClipboardEvent) 를 먼저** 시도한다. Slate 는 자체 paste 핸들러에서 줄바꿈을 단락으로 나눠 주지만 `insertText` 는 한 단락에 몰아넣을 수 있어서다. 편집기가 처리했는지는 `event.defaultPrevented` 로 판단하고, 실패하면 다시 전체 선택 후 `insertText` → `textContent` 순으로 물러난다. 어떤 방법이 통했는지는 `step:prepared` 에 단락 수와 함께 실어 보낸다.

#### 합성 클릭의 두 가지 함정

1. **`clickLikeUser()` 한 번 호출은 `click` 을 정확히 한 번만 내보내야 한다.** 마우스 시퀀스를 dispatch 한 뒤 `el.click()` 까지 부르면 클릭이 두 번 나가 재생 → 정지로 토글된다. `clickLikeUser()` 는 `pointerover … mouseup` 뒤에 `click` 을 **정확히 한 번만** 발생시킨다.
2. **합성 클릭은 사용자 제스처로 인정되지 않는다.** 브라우저의 자동재생 정책에 걸려 `audio.play()`
   가 거부될 수 있으므로, Chrome 실행 인자 `--autoplay-policy=no-user-gesture-required` 로 이
   제한을 끈다(`BrowserConfig::builder().arg(...)`, `TypecastController::open`).

#### 재생 버튼은 안정화 후 정확히 한 번 누른다

`step:prepared` 는 편집기 DOM 의 내용 일치만 보장한다. Typecast 의 React + Slate 내부 상태와
합성 준비가 뒤따라 반영될 시간을 주기 위해 배치 러너는 녹음 시작 전에 2초간 기다린다. 그 뒤
`clickPlayOnce()` 가 재생 버튼을 **정확히 한 번만** 누른다.

반복 클릭으로 되돌리지 말 것. Typecast 가 Web Audio 로 재생하면 `<audio>`/`<video>` 상태만으로
실제 재생 여부를 알 수 없고, 합성 준비 중 재생/정지 토글을 연속으로 바꾸면 화면만 재생 상태인 채
소리가 시작되지 않을 수 있다. 낭독 도중 재클릭도 같은 이유로 금지한다.

클릭 전달 여부는 `step:playing` 에 `클릭 1회(전달 1)` 형태로 남는다. **소리가 안 나면 먼저
진단 로그의 `play=` 가 진짜 재생 버튼을 가리키는지 확인할 것** — 구조 선택자가 밀려 엉뚱한
버튼을 잡고 있으면 한 번의 클릭도 해롭다.

붙여넣기 직후에는 재생 버튼이 잠시 비활성일 수 있어 `doPlay()` 가 최대 5초간 활성화를 기다린다.
클릭이 버튼까지 전달됐는지는 임시 capture 리스너로 확인하므로, 실패 시 "버튼을 못 찾음 / 클릭이
안 닿음 / 닿았는데 반응 없음"을 구분할 수 있다.

**재생 버튼은 아이콘만 있어 라벨 휴리스틱으로 잡히지 않는다.** 하단 플레이어 바의 구조 선택자를 `DEFAULT_PLAY_SELECTORS` 에 정확한 것부터 느슨한 것 순으로 내장해 두고, 그 다음에 라벨 탐색을 시도한다. 어떤 경로로 찾았는지(`playSource`)를 `step:playing` / `step:probe` 에 함께 실어 보내므로, 사이트가 개편되면 진단 로그의 `play=` 항목을 보고 목록 앞쪽에 새 선택자를 추가하면 된다. 사용자 지정 선택자(`typecast_play_selector`)가 있으면 항상 먼저 시도한다.

#### 시스템 오디오만으로 낭독을 캡처한다

Typecast 는 이제 별도의 실제 Chrome 프로세스라 `MacSystemAudioCapture` 의
`excludesCurrentProcessAudio`(자기 앱 소리 제외) 설정과 무관하다 — Chrome 은 OmniRec 과
다른 프로세스이므로 시스템 오디오 루프백에 항상 포함된다. **TTS 녹음의 `settingsOverride`
에 `system_audio_include_own_app` 를 넣지 말 것** — WKWebView 시절 자기 앱 소리를 캡처에
포함시키려고 켰던 값인데, 지금은 아무 효과가 없고 오히려 "왜 이 플래그가 필요한가"라는
혼란만 남긴다.

TTS 녹음의 `settingsOverride` 는 **꼭 필요한 것만** 덮어쓴다. 무음 자동 일시정지 · 노이즈 게이트 · 80Hz Low-cut 은 환경 설정 값을 그대로 따라야 한다(사용자가 설정한 무음 처리가 결과 파일에 반영되어야 하므로). 예외는 `auto_stop_enabled: false` 하나뿐인데, 녹음을 언제 끝낼지는 일괄 러너가 직접 판정해 다음 대본으로 넘어가는 시점을 관리해야 하기 때문이다. 오디오 엔진은 일시정지 중에도 VU 레벨을 계속 갱신하므로 auto-pause 가 켜져 있어도 소리 기반 종료 판정은 정상 동작한다.

일괄 처리 화면의 진행 표시줄에 시스템 오디오 레벨(dB)을 실시간으로 띄워, 소리가 실제로 캡처되고 있는지 바로 보이게 해 두었다.

**녹음이 끝났다고 메인 창을 무조건 앞으로 끌어오지 말 것.** 종료 처리는
`commands::finish_recording_windows()` 하나로 모았고, 메인 창은 **실제로 최소화·숨김
상태일 때만** 되돌린다(`restore_main_window_if_hidden`). 화면 녹화는 시작할 때 메인 창을
최소화하니 되돌려야 하지만, 대본 자동 녹음은 최소화하지 않는다 — 그때도 `set_focus()` 를
부르면 대본 하나가 끝날 때마다 Typecast Chrome 창에서 포커스를 빼앗는다.

**연속 녹음에서 상태 틱커가 겹치지 않게 할 것.** `spawn_status_ticker` 는 세대 번호
(`ticker_generation`)를 보고 돈다. `status != Idle` 만 보고 돌면, 한 녹음이 끝난 뒤 50ms
슬립에서 깨어나기 전에 다음 녹음이 시작될 때 이전 틱커가 죽지 않고 남아 `audio_vu_meter`
를 중복으로 쏘는 스레드가 대본 수만큼 쌓인다.

**낭독 시작/종료 판정을 DOM 이 아니라 시스템 오디오 레벨로 하는 것이 핵심이다.** Typecast 가 미디어 엘리먼트로 재생하든 Web Audio 로 재생하든 동작하고, 사이트 개편에도 영향을 받지 않는다. DOM 에 의존하는 부분은 **편집기 입력**과 **재생 버튼 클릭** 둘뿐이며, 각각 휴리스틱 + 사용자 지정 CSS 선택자(`typecast_editor_selector` / `typecast_play_selector`)로 대응한다. 자동화 코드를 고칠 때 이 경계를 유지할 것.

모든 경로는 `typecast_debug` 이벤트로 기록되어 TTS 탭의 **연동 진단 로그**에 표시된다.

---

## 🧪 Development & Testing Workflow

### Checking Rust Backend
```bash
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
cargo test --manifest-path src-tauri/Cargo.toml
```

장치·권한이 필요한 스모크 테스트는 `#[ignore]` 로 두고 수동 실행한다.
```bash
# 실제 마이크 + 실제 FFmpeg 로 녹음 → 일시정지 → 재개 → 정지, 결과 길이까지 검증
cargo test --manifest-path src-tauri/Cargo.toml --lib recorder::smoke_tests -- --ignored --nocapture
```
`tauri` 는 `[dev-dependencies]` 에서 `test` 피처로 한 번 더 선언돼 있다 — 녹음 상태 이벤트 경로
(예전에 앱 전체를 얼렸던 자기 교착)를 테스트하려면 실제 `AppHandle` 이 필요해
`tauri::test::mock_app()` 을 쓴다. `RecorderController<R = tauri::Wry>` 의 런타임 타입 매개변수는
그 목적뿐이며 프로덕션 코드는 그대로 `RecorderController` 로 쓴다.

### Checking Frontend Types & Build
```bash
npm run build
```

### Running in Local Dev
```bash
npm run tauri dev
```

---

## 🧱 중복을 만들지 않기 위한 규칙

같은 일을 두 곳에서 하지 말 것. 아래는 이미 한 곳으로 모아 둔 것들이다.

| 하는 일 | 유일한 위치 |
|---|---|
| 자막 생성 파이프라인 | `services/subtitleGeneration.ts` |
| 자막 생성 옵션 폼 | `components/SubtitleOptionsPanel.tsx` |
| Typecast 로그인 · 창 제어 | `components/TypecastSessionCard.tsx` |
| Typecast 연동 진단 로그 | 같은 파일의 `TypecastDiagnosticsLog` |
| 탭 바 마크업 | `components/TabBar.tsx` |
| 시간 · 파일 크기 · 길이 표시 | `utils/format.ts` |
| 대본 정리 · 분할 | `utils/scriptChunks.ts` |
| 녹음 상태 스냅샷 | `recorder::SharedState::status_snapshot()` |
| 녹음 종료 · 상태 되돌리기 | `recorder::SharedState::finish_active_session()` |
| 설정 · 대본 파일 원자적 쓰기 | `SettingsManager::write_atomic()` (script 저장소도 이걸 쓴다) |
| 손상 파일 격리 | `SettingsManager::quarantine_corrupt_file()` |
| 녹음 파일명 규칙 | `AudioRecorderSession::resolve_output_path()` |

**설정 값은 `Settings` 하나가 단일 출처다.** 화면이 값을 따로 들고 있다가 되돌려 저장하는 미러 상태를 만들지 말 것(자막 옵션이 그렇게 돼 있어 두 화면이 어긋날 수 있었다). 새 설정을 추가하면 `SettingsView` 에도 노출해 한곳에서 전체를 볼 수 있게 한다.

## 💡 Guidelines for Future Agents
1. **Never block the Tauri main thread**: Always use `tokio::task::spawn_blocking` for FFmpeg execution, conversions, and heavy disk I/O.
   동기(`fn`) 커맨드는 Tauri 가 **메인 스레드**에서 실행한다 — 녹화/녹음 시작·종료
   (`start_screen_record` / `start_audio_record` / `stop_record`)도 FFmpeg 스폰, cpal ·
   ScreenCaptureKit 스트림 준비, 종료 대기(최대 3초) 때문에 `async fn` + `spawn_blocking` 이다.
   되돌려 동기 커맨드로 만들면 그 시간 동안 UI 와 웹뷰 JS 가 통째로 멈추고, 대본 자동 일괄
   녹음에서는 대본마다 멈춰 중단 버튼조차 먹지 않는다.
2. **Graceful Subprocess Termination**: When stopping or canceling, close stdin / send EOF or send kill signals cleanly to avoid orphaned FFmpeg processes.
3. **Cross-Platform Compatibility**: Always gate OS-specific APIs (`windows::Win32`, Windows Registry, Windows Explorer, macOS Finder, macOS Cocoa) behind `#[cfg(target_os = "...")]`.
4. **Typecast 자동 입력은 best-effort**: 외부 사이트 DOM 은 언제든 바뀐다. `__omnirecFillScript` 가 실패해도 클립보드 복사는 항상 선행되므로 사용자가 붙여넣기로 이어갈 수 있어야 한다. 자동 입력 실패를 오류로 처리하지 말 것.
5. **자격 증명은 저장하지 않는다**: Typecast 비밀번호/토큰을 파일에 쓰지 않는다. 로그인 지속성은 웹뷰의 영구 쿠키 저장소로만 처리하고, `settings.json` 에는 표시용 이메일과 마지막 로그인 시각만 남긴다.
6. **Even Dimension Constraint**: `libx264` with `yuv420p` requires width and height to be divisible by 2 (`(w / 2) * 2`). Always enforce this for custom screen crop regions.
7. **실패를 성공으로 보고하지 말 것** — 이 저장소에서 반복적으로 나온 사고 유형이다.
   - 자식 프로세스는 **종료 코드**로 판정한다. "출력 파일이 존재한다"는 근거가 아니다.
   - `let _ = ...` 로 결과를 버리는 코드를 새로 만들지 말 것. 정말 무해한 경우에만 쓰고 왜
     무해한지 주석을 남긴다.
   - 실패했는데 `Ok` 로 돌아가는 경로, 잘렸을 수 있는 결과를 '완료'로 표시하는 경로,
     저장이 안 됐는데 저장됐다고 하는 경로는 버그다.
8. **사용자 파일을 파괴하지 말 것.**
   - 설정·대본 JSON 은 파싱 실패 시 `.bad-<타임스탬프>` 로 **보존**하고 기본값을 그 위에 쓰지
     않는다(예전엔 필드 하나가 빠진 옛 파일 하나로 전 설정이 초기화됐다). `Settings` 의 컨테이너
     수준 `#[serde(default)]` 를 제거하지 말 것 — 그게 없으면 새 필드를 추가하는 순간 같은 사고가 난다.
   - 파일 쓰기는 temp → `sync_all()` → `rename` 으로 원자 교체한다. `fs::write` 는 in-place 절단이다.
   - read-modify-write 는 잠금 안에서 전체를 감싼다(Tauri 커맨드는 동시 실행된다).
   - 녹음 실패 시 **크기가 있는 부분 파일은 지우지 않는다**. 재현 불가한 사용자의 유일한 사본이다.
