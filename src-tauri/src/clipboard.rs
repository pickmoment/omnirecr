use std::process::Command;

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::process::Stdio;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// OS 기본 클립보드 도구로 UTF-8 텍스트를 복사한다.
/// (추가 크레이트 의존 없이 플랫폼 표준 CLI만 사용)
pub fn copy_text(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        pipe_to_command("pbcopy", &[], text)
    }

    #[cfg(target_os = "windows")]
    {
        // clip.exe 는 콘솔 코드페이지를 따라가 한글이 깨지므로
        // UTF-8 임시 파일 + PowerShell Set-Clipboard 경로를 사용한다.
        //
        // 파일 이름에 ms 타임스탬프만 쓰면 같은 밀리초에 들어온 두 호출(수동 "대본 보내기"
        // 와 일괄 러너의 다음 대본 복사가 실제로 겹친다)이 **같은 경로**를 잡는다. 그러면
        // 한쪽이 남의 파일을 덮어쓰고, 먼저 끝난 쪽의 remove_file 이 아직 읽히지도 않은
        // 다른 쪽 파일을 지워 "빈 클립보드" 또는 "엉뚱한 대본 복사"가 된다.
        // PID + 프로세스 내 카운터로 경로를 유일하게 만들고, create_new 로 배타 생성해
        // 남이 만든 파일은 절대 열지 않는다(그래서 지우는 것도 항상 자기 파일뿐이다).
        use std::io::Write as _;
        use std::sync::atomic::{AtomicU64, Ordering};
        static CLIP_SEQ: AtomicU64 = AtomicU64::new(0);

        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let mut opened: Option<(std::path::PathBuf, std::fs::File)> = None;
        // PID 는 재사용되므로 예전 실행이 남긴 같은 이름의 파일과 부딪힐 수 있다.
        // 그때는 다음 카운터 값으로 넘어간다(유한 횟수 — 무한 루프 금지).
        for _ in 0..16 {
            let seq = CLIP_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("omnirec_clip_{}_{}.txt", pid, seq));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    opened = Some((path, file));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("클립보드 임시 파일 생성 실패: {}", e)),
            }
        }
        let (tmp, mut file) = opened.ok_or_else(|| {
            "클립보드 임시 파일 이름을 확보할 수 없습니다(임시 폴더에 잔재가 많습니다).".to_string()
        })?;

        let mut write_result = file.write_all(text.as_bytes());
        if write_result.is_ok() {
            write_result = file.flush();
        }
        // PowerShell 이 읽기 전에 우리 핸들을 닫는다 — Windows 에서는 열린 쓰기 핸들이
        // 남아 있으면 Get-Content 가 공유 위반으로 실패할 수 있다.
        drop(file);
        if let Err(e) = write_result {
            // 정리 실패는 사용자가 할 수 있는 조치가 없고, 아래에서 진짜 원인을 돌려준다.
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("클립보드 임시 파일 쓰기 실패: {}", e));
        }

        let script = format!(
            "Set-Clipboard -Value (Get-Content -LiteralPath '{}' -Raw -Encoding UTF8)",
            tmp.to_string_lossy().replace('\'', "''")
        );

        let mut cmd = Command::new("powershell");
        cmd.creation_flags(CREATE_NO_WINDOW);
        let output = cmd
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| format!("클립보드 복사 실패(powershell): {}", e));

        // 우리가 create_new 로 만든 정확히 그 경로만 지운다.
        let _ = std::fs::remove_file(&tmp);
        let output = output?;
        if !output.status.success() {
            return Err(format!(
                "클립보드 복사 실패: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let candidates: [(&str, &[&str]); 3] = [
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ];
        let mut last_err = String::from("사용 가능한 클립보드 도구가 없습니다.");
        for (bin, args) in candidates {
            match pipe_to_command(bin, args, text) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = e,
            }
        }
        Err(format!(
            "{} (wl-copy / xclip / xsel 중 하나를 설치해 주세요)",
            last_err
        ))
    }
}

#[cfg(unix)]
fn pipe_to_command(bin: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("클립보드 도구 실행 실패({}): {}", bin, e))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| format!("{} stdin을 열 수 없습니다.", bin))?
        .write_all(text.as_bytes())
        .map_err(|e| format!("클립보드 쓰기 실패({}): {}", bin, e))?;

    let status = child
        .wait()
        .map_err(|e| format!("클립보드 도구 종료 대기 실패({}): {}", bin, e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{} 가 오류 코드로 종료했습니다.", bin))
    }
}
