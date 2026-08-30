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
- `start_audio_record(settings: Settings, file_name_prefix: Option<String>, show_mini_controller: Option<bool>, exact_file_name: Option<bool>)` ➔ `Result<String, String>` (`exact_file_name: true` 는 타임스탬프 없이 `file_name_prefix` 그대로를 파일명으로 쓴다 — 대본 & TTS 녹음 전용)
- `check_script_recording_exists(settings: Settings, file_name_prefix: String)` ➔ `Result<Option<String>, String>` (대본 & TTS 녹음이 저장할 정확한 경로에 이미 파일이 있으면 그 경로를 반환 — 덮어쓰기 확인용)
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
- `extract_audio_pcm_16k(path: String)` ➔ `Result<Vec<f32>, String>`

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
- `typecast_prepare_script(text: String)` ➔ `Result<(), String>` (편집기 자동 입력, 결과는 `typecast_step` 으로 보고)
- `typecast_play()` / `typecast_stop_playback()` ➔ `Result<(), String>`
- `typecast_probe()` ➔ `Result<(), String>` (편집기 · 재생 버튼 탐색 진단)
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
| `auto_stop_triggered` | `Option<String>` | Emitted when silence duration exceeds threshold and recorder auto-stops. Payload is the saved output file path. |
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

#### 자동 일괄 녹음 파이프라인 (`TtsBatchRunner`)

대본마다 아래 상태 기계를 순서대로 돌린다. 오케스트레이션은 프론트엔드가 맡고, Rust 는 페이지 조작과 녹음만 담당한다.

```
prepare  typecast_prepare_script  → step:prepared / step:prepare-failed (10s 타임아웃)
         (대본 전체를 한 번에 넣는다. 입력 후 선택 해제 + 캐럿을 본문 맨 앞으로 —
          Typecast 는 커서 위치부터 낭독하므로 이 처리가 없으면 소리가 나지 않는다)
record   start_audio_record       → 저장 경로 확보 (auto_stop 은 끄고 직접 판정)
play     focus_typecast_browser → typecast_play → step:playing / step:play-failed (12s)
speak    audio_vu_meter 구독      → sys_level > 임계값이면 시작, 무음 N초 지속되면 종료
         (단락/화자 전환으로 오디오가 재생성되는 동안의 무음은 `media-ended` 신호로,
          Typecast 사이트가 스스로 재생을 멈추는 오동작은 `media-pause` 신호로 들어온다.
          둘 다 재생 버튼을 다시 누르는 등 추가 개입 없이 `segmentGapMs`(무음 판정의
          2배, 최소 8초) 만큼 종료 판정만 미룬다 — TtsBatchRunner.tsx)
save     stop_record → attach_script_recording → 다음 대본 (gap 초 대기)
```

Typecast 의 3,000자 제한은 **문단(줄) 하나의 최대 길이**이지 한 번에 넣을 수 있는 전체 길이가 아니다. 대본은 통째로 넣는다. 분할 로직을 다시 넣지 말 것.

**단락 전환 · 사이트 오동작으로 인한 무음을 낭독 종료로 오판하지 않도록 할 것. 동시에 Typecast
사이트 동작에 대한 개입은 최소로 유지할 것.** 무음 원인은 두 가지다.
(1) Typecast 는 화자/단락이 바뀔 때 다음 오디오를 새로 생성하느라 잠깐 재생이 끊긴다 — 정상 동작.
(2) Typecast 사이트가 낭독 도중 재생을 스스로 멈추는 오동작이 있다 — `ended` 없이 `pause` 만 온다.
둘 다 이 무음이 `tts_auto_stop_seconds` 를 넘기면 `waitForSpeechCycle` 이 낭독이 끝난 것으로 오판해
대본 중간에서 녹음을 끝내고 다음 대본으로 넘어간다. `hookMedia` 가 `play`/`ended`/`pause` 를 구분해
보고하고(`step:media-play` / `step:media-ended` / `step:media-pause`), 배치 러너는 둘 다 재생 버튼을
다시 누르는 등의 추가 조작 없이 `segmentGapMs` 만큼 종료 판정만 미루고 재생이 스스로 돌아오길
기다린다. **재생 버튼을 자동으로 재클릭해 "이어가기"를 시도하는 방식은 쓰지 않는다** — 이미 재생
중인데 또 클릭하면 토글 버튼이 꺼져버려 오히려 우리가 재생을 멈추는 원인이 될 수 있다(정지 버튼을
누르지 않았는데 소리가 멈추는 증상의 실제 원인이었다). 우리가 `doStop` 으로 직접 멈춘 pause 는
`intentionalStop` 플래그로 걸러 오동작으로 보고하지 않는다. 이 유예 없이 `silenceMs` 자체를 늘리는
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

4. **`execCommand('selectAll')` 은 문단 하나만 선택할 수 있다.** 그 상태로 붙여넣으면 첫 문단만 새 대본으로 바뀌고 이전 대본의 나머지 문단이 그대로 남는다. `selectAllIn()` 은 첫 `[data-slate-string]` 텍스트 노드부터 마지막 노드까지 범위를 직접 만들고, `doPrepare()` 는 붙여넣기 **전에** `clearEditor()` 로 편집기를 완전히 비운다(최대 4회 반복, `execCommand('delete')` → 실패 시 `beforeinput/deleteContentBackward`).

입력 검증은 "앞 40자가 들어갔는가"에 더해 **글자 수가 기대보다 20자 넘게 많으면 이전 대본이 남은 것으로 보고 실패 처리**한다. 이 잔여물 버그는 조용히 지나가면 엉뚱한 낭독이 녹음되므로 반드시 걸러야 한다.

편집기 입력은 **`paste`(ClipboardEvent) 를 먼저** 시도한다. Slate 는 자체 paste 핸들러에서 줄바꿈을 단락으로 나눠 주지만 `insertText` 는 한 단락에 몰아넣을 수 있어서다. 편집기가 처리했는지는 `event.defaultPrevented` 로 판단하고, 실패하면 다시 전체 선택 후 `insertText` → `textContent` 순으로 물러난다. 어떤 방법이 통했는지는 `step:prepared` 에 단락 수와 함께 실어 보낸다.

#### 합성 클릭의 두 가지 함정

1. **click 이벤트를 두 번 보내지 말 것.** 마우스 시퀀스를 dispatch 한 뒤 `el.click()` 까지 부르면 클릭이 두 번 나가 재생 → 정지로 토글된다. `clickLikeUser()` 는 `pointerover … mouseup` 뒤에 `click` 을 **정확히 한 번만** 발생시킨다.
2. **합성 클릭은 사용자 제스처로 인정되지 않는다.** 브라우저의 자동재생 정책에 걸려 `audio.play()`
   가 거부될 수 있으므로, Chrome 실행 인자 `--autoplay-policy=no-user-gesture-required` 로 이
   제한을 끈다(`BrowserConfig::builder().arg(...)`, `TypecastController::open`).

붙여넣기 직후에는 재생 버튼이 잠시 비활성일 수 있어 `doPlay()` 가 최대 5초간 활성화를 기다린다. 클릭이 버튼까지 전달됐는지는 임시 capture 리스너로 확인해 `step:playing` 에 `클릭전달`/`클릭미전달` 로 남기므로, 실패 시 "버튼을 못 찾음 / 클릭이 안 닿음 / 닿았는데 반응 없음"을 구분할 수 있다.

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

**낭독 시작/종료 판정을 DOM 이 아니라 시스템 오디오 레벨로 하는 것이 핵심이다.** Typecast 가 미디어 엘리먼트로 재생하든 Web Audio 로 재생하든 동작하고, 사이트 개편에도 영향을 받지 않는다. DOM 에 의존하는 부분은 **편집기 입력**과 **재생 버튼 클릭** 둘뿐이며, 각각 휴리스틱 + 사용자 지정 CSS 선택자(`typecast_editor_selector` / `typecast_play_selector`)로 대응한다. 자동화 코드를 고칠 때 이 경계를 유지할 것.

모든 경로는 `typecast_debug` 이벤트로 기록되어 TTS 탭의 **연동 진단 로그**에 표시된다.

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

**설정 값은 `Settings` 하나가 단일 출처다.** 화면이 값을 따로 들고 있다가 되돌려 저장하는 미러 상태를 만들지 말 것(자막 옵션이 그렇게 돼 있어 두 화면이 어긋날 수 있었다). 새 설정을 추가하면 `SettingsView` 에도 노출해 한곳에서 전체를 볼 수 있게 한다.

## 💡 Guidelines for Future Agents
1. **Never block the Tauri main thread**: Always use `tokio::task::spawn_blocking` for FFmpeg execution, conversions, and heavy disk I/O.
2. **Graceful Subprocess Termination**: When stopping or canceling, close stdin / send EOF or send kill signals cleanly to avoid orphaned FFmpeg processes.
3. **Cross-Platform Compatibility**: Always gate OS-specific APIs (`windows::Win32`, Windows Registry, Windows Explorer, macOS Finder, macOS Cocoa) behind `#[cfg(target_os = "...")]`.
4. **Typecast 자동 입력은 best-effort**: 외부 사이트 DOM 은 언제든 바뀐다. `__omnirecFillScript` 가 실패해도 클립보드 복사는 항상 선행되므로 사용자가 붙여넣기로 이어갈 수 있어야 한다. 자동 입력 실패를 오류로 처리하지 말 것.
5. **자격 증명은 저장하지 않는다**: Typecast 비밀번호/토큰을 파일에 쓰지 않는다. 로그인 지속성은 웹뷰의 영구 쿠키 저장소로만 처리하고, `settings.json` 에는 표시용 이메일과 마지막 로그인 시각만 남긴다.
6. **Even Dimension Constraint**: `libx264` with `yuv420p` requires width and height to be divisible by 2 (`(w / 2) * 2`). Always enforce this for custom screen crop regions.
