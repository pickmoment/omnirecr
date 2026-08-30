# 🎬 OmniRec Studio (옴니렉 스튜디오)

> **All-in-One Screen & Audio Recorder, Subtitle Generator, Media Joiner & Audio Converter**  
> 크로스플랫폼(Windows / macOS / Linux) 화면 녹화, 스튜디오급 오디오 녹음, 대본 기반 자막(SRT/VTT) 생성, 초고속 무손실 미디어 병합 및 WAV 포맷 변환을 제공하는 데스크톱 올인원 미디어 스튜디오입니다.

[![Tauri](https://img.shields.io/badge/Tauri-v2.0-24C8D8?logo=tauri&logoColor=white)](https://tauri.app/)
[![Rust](https://img.shields.io/badge/Rust-1.77%2B-DEA584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)](https://react.dev/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.0-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Tailwind CSS](https://img.shields.io/badge/TailwindCSS-v3.4-38B2AC?logo=tailwindcss&logoColor=white)](https://tailwindcss.com/)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

---

## ✨ 주요 기능 (Key Features)

### 1. 🖥️ 고성능 화면 녹화 (Screen Recording)
- **전체 화면 & 맞춤 영역 녹화**: 전체 모니터 녹화 및 직관적인 투명 오버레이 드래그를 통한 자유 영역 선택 녹화
- **하드웨어 가속 & 고화질 H.264 인코딩**: 30 FPS / 60 FPS 지원, libx264 veryfast 프리셋을 통한 초저지연 녹화
- **플로팅 미니 컨트롤러 (Mini Controller)**: 녹화 시작 시 메인 창이 자동으로 최소화되고 컴팩트한 상단 플로팅 컨트롤러로 전환
- **글로벌 단축키 지원**: 백그라운드에서도 작동하는 글로벌 핫키 (`F9`/`Ctrl+Shift+R`: 종료 및 저장, `F10`/`Ctrl+Shift+P`: 일시정지/재개)

### 2. 🎙️ 스튜디오급 오디오 녹음 (Audio Recording Studio)
- **동시 믹싱 (Dual Channel Mixing)**: 시스템 사운드(WASAPI / CoreAudio)와 마이크 입력을 독립 볼륨 게인(0%~200%)으로 실시간 믹싱
- **실시간 DSP 오디오 필터**:
  - **스마트 노이즈 게이트 (Noise Gate)**: 미세 팬 소음 및 화이트 노이즈 자동 차단 (-60dB ~ -20dB 임계값 조절)
  - **80Hz 하이패스 필터 (Low-cut Filter)**: 2차 IIR 필터를 통한 책상 진동 및 저음역 웅웅거림 제거
  - **무음 자동화 (Silence Automation)**: 무음 구간 자동 일시정지 & 재개, 장시간 무음 지속 시 자동 녹음 종료 및 안전 저장
  - **알림음 자동 차단**: 녹화/녹음 중 시스템 팝업 및 경고 알림음 자동 음소거 및 복구
- **다양한 오디오 포맷 지원**: 무손실 `WAV` (16-bit PCM), 고음질 `M4A (AAC)`, 범용 `MP3`

### 3. 📝 로컬 AI Whisper 기반 자막 생성기 (Subtitle Generator / Script-to-Sub)
- **로컬 AI Whisper ASR 음성인식 엔진 탑재**:
  - 클라우드 전송이나 별도 Python 설치 없이 앱 내에서 **100% 로컬 오프라인 Whisper AI(WASM / WebGPU ONNX)** 구동
  - `Whisper Tiny (39MB)` 및 `Whisper Base (73MB)` 모델을 통해 실제 음성의 **단어 단위 타임스탬프(Word-level Timestamps)** 정밀 실측
- **대본 유무에 따른 듀얼 워크플로우 지원**:
  - `📝 대본 + AI 싱크 모드`: 사용자가 작성한 원본 대본과 AI 전사 텍스트를 강제 정렬하여 **오타 없는 원본 대본 글자 그대로 100% 일치하는 싱크 자막** 완성
  - `🤖 대본 없는 순수 AI 자동 자막 모드 (Direct AI Transcription)`: 대본 없이 음성/영상 파일만으로 온디바이스 AI Whisper가 음성을 직접 받아쓰고 적정 길이의 자막(SRT/VTT)을 자동 생성
- **듀얼 싱크 엔진 지원**:
  - `로컬 AI Whisper 모드`: 실제 발화 단어/문장 타임코드 실측 매칭 (최고 정확도 • 싱크 밀림 0%)
  - `고속 음성 파형 VAD 모드`: 16kHz PCM 에너지 및 DP(동적 계획법) 기반 초고속 오프라인 정렬
- **인터랙티브 자막 에디터**:
  - 각 자막의 시작/종료 시간 및 자막 내용 인라인 편집
  - 자막 줄별 원클릭 스냅 (`[시작]`, `[종료]`) 및 개별 `±0.1s` 미세조정
  - 자막 전체 재생 시 **실시간 부드러운 자동 스크롤(Auto Scroll)** 연동
  - 전체 오디오 길이에 맞춘 비례 스케일 맞춤(`🎯 오디오 길이에 맞춤`) 도구
- **표준 포맷 내보내기 & 복사**: `.srt` / `.vtt` 파일 저장 및 클립보드 원클릭 복사

### 4. 📚 대본 관리 & Typecast TTS 낭독 녹음 (Script Studio)
- **대본 라이브러리 (Script Library)**:
  - 대본 작성 · 저장 · 검색 · 태그 분류 · 복제 · 삭제 (`~/.omnirec/scripts.json` 에 보관)
  - `.txt` / `.md` / `.srt` 파일 가져오기 및 텍스트 파일로 내보내기
  - 글자 수 · 줄 수 · **한국어 낭독 속도 기반 예상 낭독 시간** 자동 계산
  - 대본별 최근 녹음 결과 파일 연결 및 녹음 횟수 기록
- **Typecast 전용 Chrome 세션 (Persistent Login Session)**:
  - 앱 내장 웹뷰가 아니라 **실제 Google Chrome**을 별도 프로세스로 띄워 [typecast.ai](https://typecast.ai) 스튜디오를 자동화(Chrome DevTools Protocol)
  - 앱 전용 Chrome 프로필에 로그인 쿠키가 영구 저장되어 다음 실행부터 같은 계정으로 자동 접속(평소 쓰는 개인 Chrome 프로필과는 공유하지 않음)
  - **소셜 로그인 팝업 지원**: 실제 브라우저라 구글 · 애플 · 네이버 · 카카오 로그인 팝업이 별도 조작 없이 그대로 동작합니다
  - **연동 진단 로그**: 로그인이 진행되지 않을 때 어느 단계까지 동작했는지 확인하고 복사할 수 있습니다
  - 창 `뒤로` · `새로고침` 버튼과 `세션 초기화` 버튼으로 언제든 되돌리거나 로그아웃
  - **비밀번호는 저장하지 않습니다.** 계정 이메일은 어떤 계정인지 구분하기 위한 표시용 메모로만 사용
- **🤖 선택한 대본 자동 일괄 녹음 (Batch Automation)**:
  - 대본 목록에서 여러 개를 체크하고 `자동 처리 시작` 한 번이면 끝 — 대본마다 **편집기 입력 → 재생 → 녹음 → 저장 → 대본에 연결**을 순서대로 자동 반복
  - **소리 기반 시작/종료 판정**: 시스템 오디오 레벨로 낭독 시작을 감지하고, 설정한 시간만큼 무음이 이어지면 자동 종료 (Typecast가 어떤 방식으로 재생하든 동작)
  - 대본별 실시간 진행 상태(대기 · 입력 중 · 녹음 중 · 완료 · 실패)와 전체 진행률 표시, 언제든 중단 가능
  - 실패 시 건너뛰고 계속할지 즉시 중단할지 선택, 낭독 종료 무음 · 소리 감지 임계값 · 대본 간 간격 조절
  - `연동 테스트` 버튼으로 편집기와 재생 버튼을 제대로 찾는지 미리 확인하고, 필요하면 **CSS 선택자를 직접 지정**해 사이트 변경에 대응
- **대본 → TTS 편집기 수동 전송**:
  - 대본을 클립보드에 복사하고 Typecast 편집기 자동 입력을 시도 (실패해도 ⌘V / Ctrl+V 로 즉시 이어서 작업)
  - 요금제 글자 수 제한 대응을 위한 **500 / 1,000 / 2,000자 단위 조각 분할 전송**
- **TTS 재생 → 시스템 사운드 녹음**:
  - 준비 카운트다운(0/3/5/10초)을 Typecast 화면 위 배너로 안내한 뒤 녹음 시작
  - 시스템 사운드 전용 녹음(마이크 기본 OFF, 필요 시 함께 녹음 가능)
  - **무음 자동 종료**로 낭독이 끝나면 자동으로 저장, 항상 위에 뜨는 미니 컨트롤러로 수동 종료도 가능
  - 저장 파일 이름은 대본 제목 그대로 사용(타임스탬프 없음). 같은 이름의 파일이 있으면 시작 전에 덮어쓸지 확인
- **📄 자막 일괄 생성 (Batch Subtitle)**:
  - 대본에 연결된 녹음 파일을 골라 체크하면 **대본과 음성을 정렬해 `.srt` · `.vtt`를 한 번에 생성**
  - 원본 대본 글자 그대로 자막이 만들어지므로 오타 없는 결과
  - 로컬 AI Whisper(정확도 우선) / 고속 음성 파형 VAD(속도 우선) 엔진 선택, 분할 방식 · 한 줄 최대 글자 수 조절
  - 대본별 진행 상태와 생성된 자막 줄 수 · 길이 표시, 결과 폴더 바로 열기
- **자막 생성기 원클릭 연계**: 녹음 결과와 원본 대본을 그대로 자막 생성기로 전달해 `대본 + AI 싱크` 자막을 바로 생성

### 5. 🔄 WAV 오디오 포맷 변환기 (Audio Converter)
- **WAV ➔ MP3 / M4A 변환**: WAV 무손실 음원을 고음질 MP3(`libmp3lame`) 또는 고효율 M4A(`aac`)로 일괄/개별 변환
- **세부 인코딩 설정**:
  - 오디오 비트레이트: `128 kbps`, `192 kbps`, `256 kbps (추천)`, `320 kbps (최고 음질)`
  - 샘플링 레이트: `원본 유지`, `44.1 kHz (CD)`, `48.0 kHz (스튜디오)`
  - 오디오 채널: `원본 유지`, `Stereo (2ch)`, `Mono (1ch)`
  - 저장 위치: `원본 폴더 동일 위치`, `기본 저장 폴더`, `사용자 정의 폴더`
- **실시간 진행률 및 속도 표시**: FFmpeg 파이프 연동을 통한 퍼센트(%), 현재 변환 파일, 배속(Speed, 예: 15.2x) 표시 및 즉시 재생/폴더 열기 지원
- **자막 생성기 바로가기**: 변환 완료된 결과 파일을 클릭 한 번으로 자막 생성기로 전송

### 6. 🧩 미디어 파일 연결 & 병합 (Media Joiner)
- **무손실 직접 복사 (Direct Stream Copy Concat)**: 동일 코덱 및 규격의 동영상/오디오 파일들을 재인코딩 없이 0.1초 만에 무손실 병합
- **스마트 리인코딩 병합**: 규격이 다른 파일들도 해상도/샘플레이트 자동 일치화 후 하나의 MP4/M4A/MP3 파일로 결합
- **드래그 & 드롭 순서 변경**: 파일 순서 상하 이동 및 실시간 메타데이터(ffprobe) 분석

### 7. 📁 히스토리 & 파일 관리자 (History & File Manager)
- **녹화/녹음 목록 관리**: 파일별 해상도, 재생 시간, 파일 크기, 생성 일시 확인
- **인라인 파일명 변경 (Rename)**: 히스토리 목록에서 연필 버튼 또는 Enter/Esc 키로 파일명을 즉시 변경
- **인앱 오디오 미리듣기**: 별도 프로그램 실행 없이 즉시 파형 기반 오디오 재생/일시정지
- **원클릭 빠른 연동**: 히스토리 파일들을 자막 생성기, 변환기(Audio Converter) 또는 병합기(Media Joiner)로 바로 전송

### 8. 🌐 크로스플랫폼 지원 (Cross-Platform)
- **Windows**: WASAPI Loopback, GDI Grab (`gdigrab`), Windows Explorer 연동
- **macOS**: CoreAudio, AVFoundation (`avfoundation`), Finder 연동, Apple Silicon & Intel Homebrew FFmpeg 자동 감지
- **Linux**: PulseAudio/ALSA, X11 Grab (`x11grab`)

---

## 🛠️ 기술 스택 (Tech Stack)

| 구분 | 기술 / 라이브러리 |
|---|---|
| **Core Framework** | [Tauri v2](https://tauri.app/) (Rust 2021) |
| **Frontend** | React 19, TypeScript, Vite, Tailwind CSS |
| **Icons & UI** | Lucide React |
| **Audio Capture & DSP** | [cpal](https://github.com/RustAudio/cpal), Custom IIR Biquad Filter, Noise Gate, Stereo Linear Resampler |
| **Media Processing** | FFmpeg, FFprobe (Process Pipe Streaming, silencedetect) |
| **Cross-Platform Bridge** | `tauri-plugin-dialog`, `tauri-plugin-fs`, `tauri-plugin-opener`, `tauri-plugin-shell` |
| **Typecast 자동화** | [chromiumoxide](https://github.com/mattsse/chromiumoxide) (Chrome DevTools Protocol, 실제 Google Chrome 별도 실행) |

---

## 📋 시스템 요구 사항 및 설치 (Prerequisites)

### 1. 필수 의존성
- **Node.js**: `v18.0.0` 이상
- **Rust**: `1.85.0` 이상 ([rustup 설치](https://rustup.rs/))
- **FFmpeg**: 시스템 PATH에 등록되어 있거나 애플리케이션 '환경 설정'에서 경로 지정
  - **macOS (Homebrew)**: `brew install ffmpeg`
  - **Windows (Chocolatey/Scoop/Winget)**: `winget install Gyan.FFmpeg` 또는 `choco install ffmpeg`
- **Google Chrome**: 대본 & TTS 자동화(Typecast)를 쓰려면 필요. OS별 기본 설치 위치를 자동 탐색하며, 다른 경로에 설치했다면 앱 설정에서 실행 파일 경로를 직접 지정 가능

---

## 🚀 시작하기 (Getting Started)

### 1. 저장소 클론 및 패키지 설치
```bash
git clone https://github.com/pickmoment/omnirecr.git
cd omnirecr
npm install
```

### 2. 개발 모드 실행 (Development)
```bash
npm run tauri dev
```

### 3. 프로덕션 빌드 (Production Build)
```bash
npm run tauri build
```

---

## 📂 프로젝트 구조 (Project Architecture)

```
omnirecr/
├── src/                          # 프론트엔드 (React + Vite + Tailwind)
│   ├── components/
│   │   ├── AudioConverter.tsx    # WAV ➔ MP3/M4A 오디오 포맷 변환기
│   │   ├── AudioRecorder.tsx     # 고음질 오디오 녹음기 UI & DSP 상태
│   │   ├── AudioVisualizer.tsx   # 실시간 스테레오 VU 미터 시각화
│   │   ├── HistoryList.tsx       # 녹화/녹음 히스토리 및 인라인 이름 변경 & 플레이어
│   │   ├── MediaJoiner.tsx       # 동영상/오디오 무손실 및 리인코딩 병합기
│   │   ├── MiniController.tsx    # 화면 녹화 시 상단 플로팅 미니바
│   │   ├── Navbar.tsx            # 통합 내비게이션 바
│   │   ├── ScreenRecorder.tsx    # 전체/영역 화면 녹화 인터페이스
│   │   ├── SelectionOverlay.tsx  # 투명 전체화면 영역 드래그 오버레이
│   │   ├── SettingsModal.tsx     # 팝업 환경 설정 모달
│   │   ├── SettingsView.tsx      # 풀페이지 환경 설정 탭
│   │   └── SubtitleGenerator.tsx # 대본-음성 결합 자막 생성 및 실시간 에디터
│   ├── App.tsx                   # 메인 라우팅 & 이벤트 통합 관리
│   └── types.ts                  # 데이터 타입 정의
│
├── src-tauri/                    # 백엔드 코어 (Rust + Tauri)
│   ├── src/
│   │   ├── audio/                # cpal 오디오 캡처 & DSP 엔진
│   │   │   ├── dsp.rs            # 노이즈게이트, 80Hz IIR 필터, 리샘플러, 무음 감지기
│   │   │   ├── engine.rs         # 시스템 루프백 & 마이크 스트림 믹서
│   │   │   └── notifications.rs  # OS 알림음 제어
│   │   ├── converter/            # FFmpeg 오디오 포맷 변환 컨트롤러
│   │   ├── history/              # 파일 시스템 히스토리, 이름 변경 및 OS 쉘 연동
│   │   ├── merger/               # FFmpeg 무손실 및 Concat 병합 컨트롤러
│   │   ├── recorder/             # 화면/오디오 녹화 세션 관리 (gdigrab/avfoundation)
│   │   ├── settings/             # 설정 파일 입출력 및 FFmpeg/FFprobe 자동 탐색
│   │   ├── subtitle/             # 대본-음성 정렬 및 SRT/VTT 자막 생성 엔진
│   │   ├── commands.rs           # Tauri IPC 커맨드 핸들러
│   │   ├── lib.rs                # 앱 초기화, 플러그인, 핫키 핸들러
│   │   └── types.rs              # Rust 데이터 모델
│   ├── Cargo.toml
│   └── tauri.conf.json           # Tauri 2 다중 윈도우 및 번들 설정
└── README.md
```

---

## 📄 라이선스 (License)

이 프로젝트는 [MIT License](LICENSE)에 따라 배포됩니다.
