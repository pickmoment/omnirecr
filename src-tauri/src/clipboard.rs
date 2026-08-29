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
        let tmp = std::env::temp_dir().join(format!(
            "omnirec_clip_{}.txt",
            chrono::Local::now().format("%Y%m%d%H%M%S%3f")
        ));
        std::fs::write(&tmp, text).map_err(|e| format!("클립보드 임시 파일 생성 실패: {}", e))?;

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
