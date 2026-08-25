# 🎙️ OmniRec Studio (옴니렉 스튜디오)

> **All-in-One Screen & Audio Recorder, Media Joiner & Audio Converter**  
> 크로스플랫폼(Windows / macOS / Linux) 화면 녹화, 스튜디오급 오디오 녹음, 초고속 무손실 미디어 병합 및 WAV 포맷 변환을 제공하는 데스크톱 미디어 스튜디오입니다.

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

### 2. 🎧 스튜디오급 오디오 녹음 (Audio Recording Studio)
- **동시 믹싱 (Dual Channel Mixing)**: 시스템 사운드(WASAPI / CoreAudio)와 마이크 입력을 독립 볼륨 게인(0%~200%)으로 실시간 믹싱
- **실시간 DSP 오디오 필터**:
  - **스마트 노이즈 게이트 (Noise Gate)**: 미세 팬 소음 및 화이트 노이즈 자동 차단 (-60dB ~ -20dB 임계값 조절)
  - **80Hz 하이패스 필터 (Low-cut Filter)**: 2차 IIR 필터를 통한 책상 진동 및 저음역 웅웅거림 제거
  - **무음 자동화 (Silence Automation)**: 무음 구간 자동 일시정지 & 재개, 장시간 무음 지속 시 자동 녹음 종료 및 안전 저장
  - **알림음 자동 차단**: 녹화/녹음 중 시스템 팝업 및 경고 알림음 자동 음소거 및 복구
- **다양한 오디오 포맷 지원**: 무손실 `WAV` (16-bit PCM), 고음질 `M4A (AAC)`, 범용 `MP3`

### 3. 🔄 WAV 오디오 포맷 변환기 (Audio Converter)
- **WAV ➔ MP3 / M4A 변환**: WAV 무손실 음원을 고음질 MP3(`libmp3lame`) 또는 고효율 M4A(`aac`)로 일괄/개별 변환
- **세부 인코딩 설정**:
  - 오디오 비트레이트: `128 kbps`, `192 kbps`, `256 kbps (추천)`, `320 kbps (최고 음질)`
  - 샘플링 레이트: `원본 유지`, `44.1 kHz (CD)`, `48.0 kHz (스튜디오)`
  - 오디오 채널: `원본 유지`, `Stereo (2ch)`, `Mono (1ch)`
  - 저장 위치: `원본 폴더 동일 위치`, `기본 저장 폴더`, `사용자 정의 폴더`
- **실시간 진행률 및 속도 표시**: FFmpeg 파이프 연동을 통한 퍼센트(%), 현재 변환 파일, 배속(Speed, 예: 15.2x) 표시 및 즉시 재생/폴더 열기 지원

### 4. 🔗 미디어 파일 연결 & 병합 (Media Joiner)
- **무손실 직접 복사 (Direct Stream Copy Concat)**: 동일 코덱 및 규격의 동영상/오디오 파일들을 재인코딩 없이 0.1초 만에 무손실 병합
- **스마트 리인코딩 병합**: 규격이 다른 파일들도 해상도/샘플레이트 자동 일치화 후 하나의 MP4/M4A/MP3 파일로 결합
- **드래그 & 드롭 순서 변경**: 파일 순서 상하 이동 및 실시간 메타데이터(ffprobe) 분석

### 5. 📂 히스토리 & 인앱 플레이어 (History & File Manager)
- **녹화/녹음 목록 관리**: 파일별 해상도, 재생 시간, 파일 크기, 생성 일시 확인
- **인앱 오디오 미리듣기**: 별도 프로그램 실행 없이 즉시 파형 기반 오디오 재생/일시정지
- **원클릭 빠른 연동**: 히스토리 파일들을 변환기(Audio Converter) 또는 병합기(Media Joiner)로 바로 전송

### 6. 🌐 크로스플랫폼 지원 (Cross-Platform)
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
| **Media Processing** | FFmpeg, FFprobe (Process Pipe Streaming) |
| **Cross-Platform Bridge** | `tauri-plugin-dialog`, `tauri-plugin-fs`, `tauri-plugin-opener`, `tauri-plugin-shell` |

---

## 📦 시스템 요구 사항 및 설치 (Prerequisites)

### 1. 필수 의존성
- **Node.js**: `v18.0.0` 이상
- **Rust**: `1.77.2` 이상 ([rustup 설치](https://rustup.rs/))
- **FFmpeg**: 시스템 PATH에 등록되어 있거나 애플리케이션 '환경 설정'에서 경로 지정
  - **macOS (Homebrew)**: `brew install ffmpeg`
  - **Windows (Chocolatey/Scoop/Winget)**: `winget install Gyan.FFmpeg` 또는 `choco install ffmpeg`

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

## 📁 프로젝트 구조 (Project Architecture)

```
omnirecr/
├── src/                          # 프론트엔드 (React + Vite + Tailwind)
│   ├── components/
│   │   ├── AudioConverter.tsx    # WAV ➔ MP3/M4A 오디오 포맷 변환기
│   │   ├── AudioRecorder.tsx     # 고음질 오디오 녹음기 UI & DSP 상태
│   │   ├── AudioVisualizer.tsx   # 실시간 스테레오 VU 미터 시각화
│   │   ├── HistoryList.tsx       # 녹화/녹음 히스토리 및 인앱 오디오 플레이어
│   │   ├── MediaJoiner.tsx       # 동영상/오디오 무손실 및 리인코딩 병합기
│   │   ├── MiniController.tsx    # 화면 녹화 시 상단 플로팅 미니바
│   │   ├── Navbar.tsx            # 통합 내비게이션 바
│   │   ├── ScreenRecorder.tsx    # 전체/영역 화면 녹화 인터페이스
│   │   ├── SelectionOverlay.tsx  # 투명 전체화면 영역 드래그 오버레이
│   │   ├── SettingsModal.tsx     # 팝업 환경 설정 모달
│   │   └── SettingsView.tsx      # 풀페이지 환경 설정 탭
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
│   │   ├── history/              # 파일 시스템 히스토리 및 OS 쉘 연동
│   │   ├── merger/               # FFmpeg 무손실 및 Concat 병합 컨트롤러
│   │   ├── recorder/             # 화면/오디오 녹화 세션 관리 (gdigrab/avfoundation)
│   │   ├── settings/             # 설정 파일 입출력 및 FFmpeg/FFprobe 자동 탐색
│   │   ├── commands.rs           # Tauri IPC 커맨드 핸들러
│   │   ├── lib.rs                # 앱 초기화, 플러그인, 핫키 핸들러
│   │   └── types.rs              # Rust 데이터 모델
│   ├── Cargo.toml
│   └── tauri.conf.json           # Tauri 2 다중 윈도우 및 번들 설정
└── README.md
```

---

## 📜 라이선스 (License)

이 프로젝트는 [MIT License](LICENSE)에 따라 배포됩니다.
