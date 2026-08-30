use tauri::webview::{NewWindowResponse, WebviewWindowBuilder};
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WindowEvent};

use crate::settings::SettingsManager;
use crate::types::{
    TypecastBrowserState, TypecastDebugPayload, TypecastNavigationPayload, TypecastPopupPayload,
    TypecastStepPayload,
};

pub const TYPECAST_WINDOW_LABEL: &str = "typecast-browser";
pub const TYPECAST_POPUP_LABEL: &str = "typecast-popup";

/// 페이지에서 차단된 `window.open` 을 앱으로 넘길 때 쓰는 내부 전용 스킴.
/// 실제로 이동하지는 않고 네비게이션 핸들러가 가로채 취소한다.
const POPUP_SCHEME: &str = "omnirec-popup";

/// 페이지 → 앱 브리지 메시지의 문서 제목 접두사.
/// 원격 오리진에서는 Tauri IPC 가 막혀 있어, 제목 변경 이벤트를 단방향 채널로 쓴다.
const BRIDGE_TITLE_PREFIX: &str = "__omnirec__";

/// 로그인 화면으로 판단할 URL 경로 조각.
const SIGN_IN_PATH_HINTS: [&str; 6] = [
    "/sign-in",
    "/signin",
    "/login",
    "/sign-up",
    "/signup",
    "/auth",
];

pub struct TypecastController;

impl TypecastController {
    /// URL 경로만으로 로그인 여부를 추정한다. 로그인/가입 화면이 아니면 세션이 살아있는 것으로 본다.
    pub fn looks_signed_in(url: &str) -> bool {
        let lowered = url.to_lowercase();
        let path_and_query = match lowered.split_once("://") {
            Some((_, rest)) => match rest.split_once('/') {
                Some((_, path)) => format!("/{}", path),
                None => "/".to_string(),
            },
            None => lowered.clone(),
        };
        !SIGN_IN_PATH_HINTS
            .iter()
            .any(|hint| path_and_query.starts_with(hint) || path_and_query.contains(hint))
    }

    fn parse_url(url: &str) -> Result<Url, String> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err("접속할 주소가 비어 있습니다.".to_string());
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
            return Err(format!("http/https 주소만 열 수 있습니다: {}", trimmed));
        }
        Url::parse(trimmed).map_err(|e| format!("주소를 해석할 수 없습니다({}): {}", trimmed, e))
    }

    /// Typecast 창을 연다. 이미 열려 있으면 포커스만 준다.
    /// 창은 incognito 를 끈 상태(기본값)로 만들어져 로그인 쿠키가 앱 데이터에 영구 저장되고,
    /// 다음 실행에서도 같은 세션으로 자동 접속된다.
    pub fn open(app: &AppHandle, url: Option<String>) -> Result<(), String> {
        let settings = SettingsManager::load();
        let target = url.unwrap_or_else(|| settings.typecast_editor_url.clone());
        let parsed = Self::parse_url(&target)?;

        if let Some(existing) = app.get_webview_window(TYPECAST_WINDOW_LABEL) {
            let _ = existing.show();
            let _ = existing.unminimize();
            let _ = existing.set_focus();
            return Ok(());
        }

        let nav_handle = app.clone();
        let title_handle = app.clone();
        let new_window_handle = app.clone();

        let builder = WebviewWindowBuilder::new(
            app,
            TYPECAST_WINDOW_LABEL,
            WebviewUrl::External(parsed),
        )
        .title("Typecast TTS — OmniRec 대본 낭독")
        .inner_size(1180.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .resizable(true)
        .center()
        .focused(true)
        // macOS 는 창이 가려지거나 최소화되면 WKWebView 를 전력 절약 모드로 스로틀링해
        // 재생 중인 오디오까지 멈출 수 있다(정지 버튼과 무관한 멈춤의 실제 원인 중 하나).
        // 배치 자동화 중에는 창이 계속 최전면에 있지 않을 수 있으므로 꺼둔다.
        .background_throttling(BackgroundThrottlingPolicy::Disabled)
        // incognito(false): 쿠키/로컬스토리지를 영구 보관해 로그인 상태를 유지한다.
        .incognito(false)
        // 편집기가 iframe 안에 있을 수 있으므로 모든 프레임에 주입한다.
        .initialization_script_for_all_frames(MAIN_INIT_SCRIPT)
        .on_navigation(move |url| {
            // 커스텀 스킴 채널(제목 채널이 동작하지 않는 환경용 예비 경로)
            if url.scheme() == POPUP_SCHEME {
                let message = percent_decode(url.path());
                Self::handle_bridge_message(&nav_handle, &message);
                return false;
            }

            let url_string = url.to_string();
            Self::debug(&nav_handle, "navigate", &url_string);
            let _ = nav_handle.emit(
                "typecast_navigation",
                TypecastNavigationPayload {
                    looks_signed_in: Self::looks_signed_in(&url_string),
                    url: url_string,
                },
            );
            true
        })
        // 페이지 → 앱 단방향 브리지(문서 제목 채널)
        .on_document_title_changed(move |_window, title| {
            if let Some(message) = title.strip_prefix(BRIDGE_TITLE_PREFIX) {
                Self::handle_bridge_message(&title_handle, message);
            }
        })
        // 네이티브 팝업이 허용되는 환경에서는 WebKit 이 만든 진짜 팝업을 그대로 쓴다.
        // 이 경로가 살아 있으면 window.opener 관계가 유지돼 소셜 로그인이 그대로 동작한다.
        .on_new_window(move |url, _features| {
            Self::debug(&new_window_handle, "native-popup", url.as_str());
            NewWindowResponse::Allow
        });

        // 합성 클릭으로도 오디오가 재생되도록 자동재생 제한을 푼 설정을 쓴다.
        #[cfg(target_os = "macos")]
        let builder = match automation_webview_configuration() {
            Some(configuration) => builder.with_webview_configuration(configuration),
            None => builder,
        };

        let window = builder
            .build()
            .map_err(|e| format!("Typecast 창을 열 수 없습니다: {}", e))?;

        // WKWebView 는 사용자 제스처와 분리된 window.open 을 막는다.
        // 소셜 로그인은 비동기 흐름에서 팝업을 열기 때문에 이 설정이 없으면 차단된다.
        enable_automatic_popups(&window);

        let close_handle = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                let _ = close_handle.emit("typecast_browser_closed", ());
            }
        });

        Ok(())
    }

    /// 진단 로그를 프론트엔드로 흘려보낸다. 어떤 경로가 실제로 동작하는지 추적하기 위한 것.
    fn debug(app: &AppHandle, kind: &str, detail: &str) {
        let detail: String = detail.chars().take(300).collect();
        let _ = app.emit(
            "typecast_debug",
            TypecastDebugPayload {
                kind: kind.to_string(),
                detail,
                at: chrono::Local::now().format("%H:%M:%S").to_string(),
            },
        );
    }

    /// 페이지가 보낸 브리지 메시지를 처리한다. 형식은 `<kind>:<payload>`.
    ///
    /// - `open:<url>`   차단된 팝업 대신 앱이 로그인 창을 열어 준다
    /// - `close`        메인 페이지가 팝업을 닫으라고 요청
    /// - `msg:<json>`   팝업이 opener 로 보내려던 postMessage 를 메인 창으로 중계
    /// - `log:<text>`   진단용 로그
    fn handle_bridge_message(app: &AppHandle, message: &str) {
        // 제목 채널과 커스텀 스킴 채널이 같은 메시지를 각각 보내므로
        // 앞에 붙은 일련번호로 중복을 걸러낸다.
        let message = match message.split_once('|') {
            Some((seq, rest)) => {
                if !seen_bridge_message(seq) {
                    return;
                }
                rest
            }
            None => message,
        };

        let (kind, payload) = match message.split_once(':') {
            Some((k, p)) => (k, p),
            None => (message, ""),
        };

        Self::debug(app, &format!("bridge:{kind}"), payload);

        match kind {
            "open" => {
                let target = payload.to_string();
                if Self::parse_url(&target).is_err() {
                    return;
                }
                let _ = app.emit(
                    "typecast_popup_intercepted",
                    TypecastPopupPayload { url: target.clone() },
                );
                let handle = app.clone();
                // 델리게이트 콜백 안에서 창을 직접 만들지 않도록 이벤트 루프로 넘긴다.
                let _ = app.run_on_main_thread(move || {
                    let _ = Self::open_popup_window(&handle, &target);
                });
            }
            "close" => {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(popup) = handle.get_webview_window(TYPECAST_POPUP_LABEL) {
                        let _ = popup.close();
                    }
                });
            }
            // 페이지 자동화 단계 보고 (`step:<name>:<detail>`)
            "step" => {
                let (name, detail) = match payload.split_once(':') {
                    Some((n, d)) => (n.to_string(), d.to_string()),
                    None => (payload.to_string(), String::new()),
                };
                let _ = app.emit("typecast_step", TypecastStepPayload { name, detail });
            }
            "msg" => {
                let payload = payload.to_string();
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    Self::deliver_message_to_main(&handle, &payload);
                });
            }
            _ => {}
        }
    }

    /// 팝업이 `window.opener.postMessage(...)` 로 보내려던 값을 메인 창에 합성 이벤트로 전달한다.
    fn deliver_message_to_main(app: &AppHandle, payload_json: &str) {
        let Some(window) = app.get_webview_window(TYPECAST_WINDOW_LABEL) else {
            return;
        };
        // payload_json 은 { "data": ..., "origin": "..." } 형태의 JSON 문자열.
        let literal = match serde_json::to_string(payload_json) {
            Ok(v) => v,
            Err(_) => return,
        };
        let _ = window.eval(format!(
            "window.__omnirecDeliverMessage && window.__omnirecDeliverMessage({});",
            literal
        ));
        let _ = window.set_focus();
    }

    /// 네이티브 팝업이 막혔을 때 대신 여는 로그인 창.
    ///
    /// 이 창에는 `window.opener` 스텁을 주입해, 콜백 페이지가 보내는
    /// `opener.postMessage({type:"AUTH_TYPE", ...})` 를 가로채 메인 창으로 중계한다.
    /// (별도 창이라 진짜 opener 관계가 없기 때문에 반드시 필요하다.)
    fn open_popup_window(app: &AppHandle, target: &str) -> Result<(), String> {
        let parsed = Self::parse_url(target)?;

        if let Some(existing) = app.get_webview_window(TYPECAST_POPUP_LABEL) {
            existing
                .navigate(parsed)
                .map_err(|e| format!("로그인 창 이동 실패: {}", e))?;
            let _ = existing.show();
            let _ = existing.set_focus();
            return Ok(());
        }

        let title_handle = app.clone();
        let nav_handle = app.clone();
        let window =
            WebviewWindowBuilder::new(app, TYPECAST_POPUP_LABEL, WebviewUrl::External(parsed))
                .title("Typecast 로그인")
                .inner_size(560.0, 720.0)
                .min_inner_size(420.0, 560.0)
                .resizable(true)
                .center()
                .focused(true)
                .incognito(false)
                .initialization_script(POPUP_INIT_SCRIPT)
                .on_navigation(move |url| {
                    if url.scheme() == POPUP_SCHEME {
                        Self::handle_bridge_message(&nav_handle, &percent_decode(url.path()));
                        return false;
                    }
                    Self::debug(&nav_handle, "popup-navigate", url.as_str());
                    true
                })
                .on_document_title_changed(move |_w, title| {
                    if let Some(message) = title.strip_prefix(BRIDGE_TITLE_PREFIX) {
                        Self::handle_bridge_message(&title_handle, message);
                    }
                })
                .build()
                .map_err(|e| format!("로그인 창을 열 수 없습니다: {}", e))?;

        enable_automatic_popups(&window);

        // 로그인 창이 닫히면 메인 페이지의 `popup.closed` 폴링이 멈출 수 있도록 알려준다.
        let closed_handle = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                if let Some(main) = closed_handle.get_webview_window(TYPECAST_WINDOW_LABEL) {
                    let _ = main.eval("window.__omnirecPopupClosed && window.__omnirecPopupClosed();");
                }
            }
        });

        Ok(())
    }

    pub fn close(app: &AppHandle) -> Result<(), String> {
        if let Some(popup) = app.get_webview_window(TYPECAST_POPUP_LABEL) {
            let _ = popup.close();
        }
        if let Some(window) = app.get_webview_window(TYPECAST_WINDOW_LABEL) {
            window
                .close()
                .map_err(|e| format!("Typecast 창을 닫을 수 없습니다: {}", e))?;
        }
        Ok(())
    }

    pub fn focus(app: &AppHandle) -> Result<(), String> {
        let window = app
            .get_webview_window(TYPECAST_WINDOW_LABEL)
            .ok_or_else(|| "Typecast 창이 열려 있지 않습니다.".to_string())?;
        let _ = window.show();
        let _ = window.unminimize();
        window
            .set_focus()
            .map_err(|e| format!("Typecast 창 포커스 실패: {}", e))
    }

    pub fn navigate(app: &AppHandle, url: String) -> Result<(), String> {
        let parsed = Self::parse_url(&url)?;
        match app.get_webview_window(TYPECAST_WINDOW_LABEL) {
            Some(window) => window
                .navigate(parsed)
                .map_err(|e| format!("페이지 이동 실패: {}", e)),
            None => Self::open(app, Some(url)),
        }
    }

    pub fn go_back(app: &AppHandle) -> Result<(), String> {
        let window = app
            .get_webview_window(TYPECAST_WINDOW_LABEL)
            .ok_or_else(|| "Typecast 창이 열려 있지 않습니다.".to_string())?;
        window
            .eval("history.back();")
            .map_err(|e| format!("뒤로 가기 실패: {}", e))
    }

    pub fn reload(app: &AppHandle) -> Result<(), String> {
        let window = app
            .get_webview_window(TYPECAST_WINDOW_LABEL)
            .ok_or_else(|| "Typecast 창이 열려 있지 않습니다.".to_string())?;
        window.reload().map_err(|e| format!("새로고침 실패: {}", e))
    }

    /// 저장된 로그인 세션(쿠키/스토리지)을 모두 지우고 로그인 페이지로 되돌린다.
    pub fn clear_session(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();

        if let Some(popup) = app.get_webview_window(TYPECAST_POPUP_LABEL) {
            let _ = popup.close();
        }

        if let Some(window) = app.get_webview_window(TYPECAST_WINDOW_LABEL) {
            window
                .clear_all_browsing_data()
                .map_err(|e| format!("세션 데이터 삭제 실패: {}", e))?;
            if let Ok(parsed) = Self::parse_url(&settings.typecast_signin_url) {
                let _ = window.navigate(parsed);
            }
        } else {
            // 창이 없으면 잠깐 띄워서 데이터를 지운 뒤 로그인 페이지를 보여준다.
            Self::open(app, Some(settings.typecast_signin_url.clone()))?;
            if let Some(window) = app.get_webview_window(TYPECAST_WINDOW_LABEL) {
                window
                    .clear_all_browsing_data()
                    .map_err(|e| format!("세션 데이터 삭제 실패: {}", e))?;
            }
        }

        let mut updated = settings;
        updated.typecast_session_saved = false;
        updated.typecast_last_login_at = None;
        SettingsManager::save(&updated)?;

        Ok(())
    }

    pub fn state(app: &AppHandle) -> TypecastBrowserState {
        let settings = SettingsManager::load();
        let window = app.get_webview_window(TYPECAST_WINDOW_LABEL);
        let current_url = window
            .as_ref()
            .and_then(|w| w.url().ok())
            .map(|u| u.to_string());

        TypecastBrowserState {
            is_open: window.is_some(),
            looks_signed_in: current_url
                .as_deref()
                .map(Self::looks_signed_in)
                .unwrap_or(settings.typecast_session_saved),
            current_url,
            account_email: settings.typecast_account_email.clone(),
            last_login_at: settings.typecast_last_login_at.clone(),
        }
    }

    fn main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
        app.get_webview_window(TYPECAST_WINDOW_LABEL)
            .ok_or_else(|| "Typecast 창이 열려 있지 않습니다.".to_string())
    }

    /// 자동화용 선택자를 페이지에 심는다. 비워두면 내장 휴리스틱을 쓴다.
    pub fn apply_selectors(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();
        let window = Self::main_window(app)?;
        let editor = serde_json::to_string(&settings.typecast_editor_selector)
            .map_err(|e| e.to_string())?;
        let play =
            serde_json::to_string(&settings.typecast_play_selector).map_err(|e| e.to_string())?;
        window
            .eval(format!(
                "window.__omnirecSetSelectors && window.__omnirecSetSelectors({}, {});",
                editor, play
            ))
            .map_err(|e| format!("선택자 적용 실패: {}", e))
    }

    /// 대본을 편집기에 채우고, 결과를 `typecast_step` 이벤트로 보고한다.
    ///
    /// 자동 입력이 실패하더라도 사용자가 직접 붙여넣을 수 있도록 클립보드에도 넣어 둔다.
    /// 수동 녹음 화면의 "대본 보내기"도 같은 경로를 쓴다.
    pub fn prepare_script(app: &AppHandle, text: String) -> Result<(), String> {
        let _ = crate::clipboard::copy_text(&text);
        Self::apply_selectors(app)?;
        let window = Self::main_window(app)?;
        let payload = serde_json::to_string(&text).map_err(|e| e.to_string())?;
        window
            .eval(format!(
                "window.__omnirecPrepare && window.__omnirecPrepare({});",
                payload
            ))
            .map_err(|e| format!("대본 주입 실패: {}", e))?;
        let _ = window.show();
        Ok(())
    }

    /// 편집기의 재생 버튼을 누른다.
    pub fn play(app: &AppHandle) -> Result<(), String> {
        let window = Self::main_window(app)?;
        window
            .eval("window.__omnirecPlay && window.__omnirecPlay();")
            .map_err(|e| format!("재생 실행 실패: {}", e))
    }

    /// 재생을 멈춘다(정지/일시정지 버튼 탐색).
    pub fn stop_playback(app: &AppHandle) -> Result<(), String> {
        let window = Self::main_window(app)?;
        window
            .eval("window.__omnirecStopPlayback && window.__omnirecStopPlayback();")
            .map_err(|e| format!("재생 정지 실패: {}", e))
    }

    /// 편집기 / 재생 버튼 후보를 찾아 진단 정보를 보고한다.
    pub fn probe(app: &AppHandle) -> Result<(), String> {
        Self::apply_selectors(app)?;
        let window = Self::main_window(app)?;
        window
            .eval("window.__omnirecProbe && window.__omnirecProbe();")
            .map_err(|e| format!("페이지 진단 실패: {}", e))
    }

    /// Typecast 페이지 위에 안내 토스트를 띄운다(카운트다운 / 녹음 시작 알림 용도).
    pub fn notify(app: &AppHandle, message: String, tone: Option<String>) -> Result<(), String> {
        let window = app
            .get_webview_window(TYPECAST_WINDOW_LABEL)
            .ok_or_else(|| "Typecast 창이 열려 있지 않습니다.".to_string())?;

        let msg = serde_json::to_string(&message).map_err(|e| e.to_string())?;
        let tone = serde_json::to_string(&tone.unwrap_or_else(|| "info".to_string()))
            .map_err(|e| e.to_string())?;
        window
            .eval(format!(
                "window.__omnirecToast && window.__omnirecToast({}, {});",
                msg, tone
            ))
            .map_err(|e| format!("Typecast 알림 표시 실패: {}", e))
    }
}

/// 자동화에 맞춘 WKWebView 설정을 만든다.
///
/// - `mediaTypesRequiringUserActionForPlayback = None`:
///   JS 로 만든 합성 클릭은 사용자 제스처로 인정되지 않아 오디오 재생이 거부될 수 있다.
///   자동 낭독 녹음을 하려면 이 제한을 풀어야 한다.
/// - `javaScriptCanOpenWindowsAutomatically = true`: 소셜 로그인 팝업 차단 해제.
#[cfg(target_os = "macos")]
fn automation_webview_configuration(
) -> Option<objc2::rc::Retained<objc2_web_kit::WKWebViewConfiguration>> {
    use objc2::MainThreadMarker;
    use objc2_web_kit::{WKAudiovisualMediaTypes, WKWebViewConfiguration};

    let mtm = MainThreadMarker::new()?;
    // SAFETY: 메인 스레드에서 새 설정 객체를 만들고 속성만 설정한다.
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        configuration.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None);
        configuration
            .preferences()
            .setJavaScriptCanOpenWindowsAutomatically(true);
        Some(configuration)
    }
}

/// WKWebView 는 사용자 제스처와 분리된 `window.open` 을 차단한다.
/// Typecast 의 소셜 로그인은 async 흐름에서 팝업을 열기 때문에 이 설정이 필요하다.
/// 이 값이 켜져 네이티브 팝업이 열리면 `window.opener` 관계가 그대로 유지되어
/// 아래 브리지 없이도 로그인이 정상 동작한다.
#[cfg(target_os = "macos")]
fn enable_automatic_popups(window: &tauri::WebviewWindow) {
    use objc2_web_kit::WKWebView;

    let _ = window.with_webview(|platform| {
        let ptr = platform.inner() as *mut WKWebView;
        if ptr.is_null() {
            return;
        }
        // SAFETY: Tauri 가 넘겨주는 포인터는 살아 있는 WKWebView(하위 클래스) 인스턴스이며,
        // 이 클로저는 메인 스레드에서 실행된다.
        unsafe {
            let webview = &*ptr;
            let preferences = webview.configuration().preferences();
            preferences.setJavaScriptCanOpenWindowsAutomatically(true);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn enable_automatic_popups(_window: &tauri::WebviewWindow) {}

/// 페이지 → 앱 브리지 공통부. 문서 제목 채널을 주 경로로,
/// 커스텀 스킴 네비게이션을 예비 경로로 함께 사용한다.
/// (`concat!` 은 리터럴만 받으므로 상수가 아닌 매크로로 둔다.)
/// 페이지 → 앱 브리지 공통부. 문서 제목 채널을 주 경로로,
/// 커스텀 스킴 네비게이션을 예비 경로로 함께 사용한다.
/// 두 채널이 같은 메시지를 두 번 보낼 수 있어 일련번호로 중복을 거른다.
/// (`concat!` 은 리터럴만 받으므로 상수가 아닌 매크로로 둔다.)
macro_rules! bridge_snippet {
    () => {
        r#"
  var IS_TOP = (function () { try { return window.top === window; } catch (e) { return false; } })();
  // 페이지가 새로 로드될 때마다 번호가 1부터 다시 시작하면 이전 메시지와 충돌하므로
  // 로드마다 임의 토큰을 붙여 고유하게 만든다.
  var omnirecId = Math.random().toString(36).slice(2, 8);
  var omnirecSeq = 0;

  // 최상위 프레임만 앱과 직접 통신한다. 서브프레임은 top 으로 postMessage 해서 중계한다.
  function omnirecSend(message) {
    omnirecSeq += 1;
    var framed = omnirecId + '-' + omnirecSeq + '|' + message;
    try {
      var previous = document.title;
      document.title = '__omnirec__' + framed;
      setTimeout(function () {
        if (document.title.indexOf('__omnirec__') === 0) document.title = previous;
      }, 30);
    } catch (e) {}
    try {
      var frame = document.createElement('iframe');
      frame.style.display = 'none';
      frame.src = 'omnirec-popup:' + encodeURIComponent(framed);
      (document.body || document.documentElement).appendChild(frame);
      setTimeout(function () { frame.remove(); }, 200);
    } catch (e) {}
  }

  // 어느 프레임에서든 쓸 수 있는 보고 함수.
  function report(message) {
    if (IS_TOP) {
      omnirecSend(message);
      return;
    }
    try {
      window.top.postMessage({ __omnirec: 'report', message: message }, '*');
    } catch (e) {}
  }
"#
    };
}

/// Typecast 메인 창에 주입되는 스크립트. **모든 프레임**에서 실행된다.
///
/// - `window.open` 오버라이드: 네이티브 팝업이 막히면 앱이 연 로그인 창을 가리키는
///   프록시 window 객체를 돌려줘 사이트가 "팝업 차단"으로 오판하지 않게 한다.
///   Typecast 는 `window.open("", "authPopup", ...)` 처럼 **빈 URL** 로 먼저 열고
///   나중에 `popup.location.replace(authUrl)` 을 부르므로 location 프록시가 반드시 필요하다.
/// - 자동화: 편집기 입력 · 재생 버튼 클릭 · 진단. 편집기가 iframe 이나 shadow DOM 안에
///   있을 수 있으므로 모든 프레임에 주입하고 shadow root 까지 훑는다.
/// - `__omnirecDeliverMessage(json)`: 로그인 창이 opener 로 보내려던 postMessage 를 재현한다.
const MAIN_INIT_SCRIPT: &str = concat!(
    r#"
(function () {
  if (window.__omnirecInjected) return;
  window.__omnirecInjected = true;
"#,
    bridge_snippet!(),
    r#"
  function toast(message, tone) {
    try {
      var prev = document.getElementById('__omnirec_toast');
      if (prev) prev.remove();
      var el = document.createElement('div');
      el.id = '__omnirec_toast';
      el.textContent = message;
      el.style.cssText = [
        'position:fixed', 'z-index:2147483647', 'left:50%', 'top:24px',
        'transform:translateX(-50%)', 'padding:12px 18px', 'border-radius:12px',
        'font:600 13px/1.4 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif',
        'color:#fff', 'box-shadow:0 8px 30px rgba(0,0,0,.35)', 'pointer-events:none',
        'background:' + (tone === 'warn' ? '#b45309' : tone === 'rec' ? '#b91c1c' : '#1d4ed8')
      ].join(';');
      (document.body || document.documentElement).appendChild(el);
      setTimeout(function () { el.remove(); }, 4200);
    } catch (e) {}
  }
  if (IS_TOP) window.__omnirecToast = toast;

  // ── 차단된 소셜 로그인 팝업 대체 ────────────────────────────
  var proxyPopup = null;

  function makePopupProxy() {
    var closed = false;
    var currentHref = '';

    function go(url) {
      if (!url) return;
      currentHref = String(url);
      report('open:' + currentHref);
    }

    var location = {
      replace: go,
      assign: go,
      reload: function () {},
      toString: function () { return currentHref; }
    };
    try {
      Object.defineProperty(location, 'href', {
        get: function () { return currentHref; },
        set: function (value) { go(value); }
      });
    } catch (e) {
      location.href = '';
    }

    var proxy = {
      name: 'authPopup',
      opener: window,
      location: location,
      close: function () { closed = true; report('close'); },
      focus: function () {},
      blur: function () {},
      postMessage: function () {},
      addEventListener: function () {},
      removeEventListener: function () {},
      __omnirecMarkClosed: function () { closed = true; }
    };
    try {
      Object.defineProperty(proxy, 'closed', { get: function () { return closed; } });
    } catch (e) {
      proxy.closed = false;
    }
    return proxy;
  }

  window.__omnirecPopupClosed = function () {
    if (proxyPopup && proxyPopup.__omnirecMarkClosed) proxyPopup.__omnirecMarkClosed();
  };

  window.__omnirecDeliverMessage = function (raw) {
    try {
      var parsed = typeof raw === 'string' ? JSON.parse(raw) : raw;
      window.dispatchEvent(new MessageEvent('message', {
        data: parsed.data,
        origin: parsed.origin,
        source: proxyPopup || window
      }));
    } catch (e) {
      toast('로그인 결과를 전달하지 못했습니다: ' + e, 'warn');
    }
  };

  var nativeOpen = typeof window.open === 'function' ? window.open.bind(window) : null;

  window.open = function (url, target, features) {
    var opened = null;
    try {
      if (nativeOpen) opened = nativeOpen(url, target, features);
    } catch (e) {
      opened = null;
    }
    if (opened) return opened;

    proxyPopup = makePopupProxy();
    if (url) proxyPopup.location.replace(String(url));
    toast('팝업이 차단되어 OmniRec 로그인 창으로 대신 엽니다.', 'warn');
    return proxyPopup;
  };

  // ── 페이지 자동화 ──────────────────────────────────────────
  var selectors = { editor: '', play: '' };

  function isVisible(el) {
    if (!el) return false;
    var rect = el.getBoundingClientRect();
    if (rect.width < 2 || rect.height < 2) return false;
    try {
      var style = window.getComputedStyle(el);
      if (style.visibility === 'hidden' || style.display === 'none' || style.opacity === '0') {
        return false;
      }
    } catch (e) {}
    return true;
  }

  function area(el) {
    var r = el.getBoundingClientRect();
    return r.width * r.height;
  }

  // shadow DOM 스캔은 문서 전체를 훑어야 해서 비싸다.
  // 실제로 shadow root 가 있는 페이지에서만 하도록 결과를 잠깐 캐시한다.
  var shadowCheck = { value: null, at: 0 };
  function pageHasShadowDom() {
    var now = Date.now();
    if (shadowCheck.value !== null && now - shadowCheck.at < 5000) return shadowCheck.value;
    var found = false;
    try {
      var all = document.querySelectorAll('*');
      for (var i = 0; i < all.length; i++) {
        if (all[i].shadowRoot) { found = true; break; }
      }
    } catch (e) {}
    shadowCheck = { value: found, at: now };
    return found;
  }

  // shadow DOM 안까지 훑는다. 웹 컴포넌트로 만든 편집기를 놓치지 않기 위함.
  function deepQueryAll(selector, root, out, depth) {
    out = out || [];
    root = root || document;
    depth = depth || 0;
    try {
      Array.prototype.push.apply(out, root.querySelectorAll(selector));
    } catch (e) {}
    if (depth === 0 && !pageHasShadowDom()) return out;
    if (depth >= 6) return out;
    var all;
    try { all = root.querySelectorAll('*'); } catch (e) { return out; }
    for (var i = 0; i < all.length; i++) {
      if (all[i].shadowRoot) deepQueryAll(selector, all[i].shadowRoot, out, depth + 1);
    }
    return out;
  }

  function describe(el) {
    if (!el) return 'null';
    var parts = [el.tagName.toLowerCase()];
    if (el.id) parts.push('#' + el.id);
    var cls = el.getAttribute && el.getAttribute('class');
    if (cls) parts.push('.' + String(cls).trim().split(/\s+/).slice(0, 2).join('.'));
    var aria = el.getAttribute && el.getAttribute('aria-label');
    if (aria) parts.push('[aria-label="' + aria + '"]');
    var testid = el.getAttribute && el.getAttribute('data-testid');
    if (testid) parts.push('[data-testid="' + testid + '"]');
    var text = (el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 20);
    if (text) parts.push('"' + text + '"');
    return parts.join('');
  }

  // Typecast 는 Slate.js 편집기를 쓴다. 다른 후보보다 먼저 확인한다.
  var SLATE_SELECTOR = '[data-slate-editor="true"]';
  var EDITOR_SELECTOR =
    '[data-slate-editor="true"], textarea, [contenteditable="true"], [contenteditable=""], .ProseMirror, [role="textbox"], .ql-editor, .cm-content';

  function findEditor() {
    if (selectors.editor) {
      var custom = deepQueryAll(selectors.editor).filter(isVisible)[0];
      if (custom) return custom;
    }
    var slate = deepQueryAll(SLATE_SELECTOR).filter(isVisible)[0];
    if (slate) return slate;

    var candidates = deepQueryAll(EDITOR_SELECTOR).filter(function (el) {
      return isVisible(el) && area(el) > 400;
    });
    candidates.sort(function (a, b) { return area(b) - area(a); });
    return candidates[0] || null;
  }

  var PLAY_HINTS = ['play', '재생', '들어보기', 'preview', '미리듣기', 'listen', '생성', 'generate'];
  var STOP_HINTS = ['pause', 'stop', '정지', '일시정지', '멈춤'];

  // Typecast 스튜디오의 하단 플레이어 바 재생 버튼.
  // 아이콘만 있는 버튼이라 라벨 휴리스틱으로는 잡히지 않아 구조 선택자를 내장한다.
  // 위에서부터 순서대로 시도하며, 앞쪽일수록 정확하고 뒤쪽일수록 레이아웃 변경에 강하다.
  var DEFAULT_PLAY_SELECTORS = [
    '#root > div > div > div > main > div.flex.h-full.flex-col.bg-white > div.flex.min-h-0.grow > div.font-size-4.relative.flex.min-w-0.grow.flex-col.bg-white > div.flex.shrink.justify-center.border-t > div > div > div.relative.flex.h-8.items-center.justify-between > div > div > div > button > div',
    'div.flex.shrink.justify-center.border-t div.relative.flex.h-8.items-center.justify-between button',
    'div.border-t div.h-8.items-center.justify-between button',
    'div.border-t button'
  ];

  // 마지막으로 어떤 경로로 재생 버튼을 찾았는지(진단 로그용)
  var playSource = '';

  // 정지 요청(doStop)으로 인한 pause 는 오동작 신호로 취급하지 않는다.
  var intentionalStop = false;

  function buttonLabel(el) {
    var cls = el.className;
    if (cls && cls.baseVal !== undefined) cls = cls.baseVal;
    return [
      el.getAttribute('aria-label') || '',
      el.getAttribute('title') || '',
      el.getAttribute('data-testid') || '',
      el.id || '',
      cls || '',
      el.textContent || ''
    ].join(' ').toLowerCase();
  }

  function allButtons() {
    return deepQueryAll('button, [role="button"], a[role="button"]').filter(isVisible);
  }

  function findButton(hints, avoid) {
    var buttons = allButtons();
    for (var i = 0; i < buttons.length; i++) {
      var label = buttonLabel(buttons[i]);
      var hit = hints.some(function (h) { return label.indexOf(h) >= 0; });
      var bad = avoid && avoid.some(function (h) { return label.indexOf(h) >= 0; });
      if (hit && !bad && !buttons[i].disabled) return buttons[i];
    }
    return null;
  }

  function findPlayButton() {
    if (selectors.play) {
      var custom = deepQueryAll(selectors.play).filter(isVisible)[0];
      if (custom) {
        playSource = '사용자 선택자';
        return custom;
      }
    }

    for (var i = 0; i < DEFAULT_PLAY_SELECTORS.length; i++) {
      var hit = deepQueryAll(DEFAULT_PLAY_SELECTORS[i]).filter(isVisible)[0];
      if (hit) {
        playSource = '내장 선택자 #' + (i + 1);
        return hit;
      }
    }

    var byLabel = findButton(PLAY_HINTS, STOP_HINTS);
    if (byLabel) {
      playSource = '라벨 탐색';
      return byLabel;
    }

    playSource = '';
    return null;
  }

  function closestButton(el) {
    try {
      return (el.closest && el.closest('button')) || el;
    } catch (e) {
      return el;
    }
  }

  function isDisabled(el) {
    var button = closestButton(el);
    if (button.disabled) return true;
    var aria = button.getAttribute && button.getAttribute('aria-disabled');
    return aria === 'true';
  }

  function clickLikeUser(el) {
    try { el.scrollIntoView({ block: 'center' }); } catch (e) {}
    var rect = el.getBoundingClientRect();
    var base = {
      bubbles: true,
      cancelable: true,
      view: window,
      button: 0,
      clientX: rect.left + rect.width / 2,
      clientY: rect.top + rect.height / 2,
      pointerId: 1,
      pointerType: 'mouse',
      isPrimary: true
    };

    // hover 로만 나타나는 컨트롤이 있어 포인터 진입부터 흉내 낸다.
    // click 이벤트는 정확히 한 번만 발생시켜야 한다.
    // (예전에 마우스 시퀀스 뒤에 el.click() 까지 불러 클릭이 두 번 나갔고,
    //  재생 → 정지로 토글되어 아무 소리도 나지 않았다.)
    var sequence = [
      ['pointerover', 0], ['mouseover', 0], ['pointermove', 0], ['mousemove', 0],
      ['pointerdown', 1], ['mousedown', 1], ['pointerup', 0], ['mouseup', 0], ['click', 0]
    ];
    sequence.forEach(function (entry) {
      var type = entry[0];
      var opts = {};
      for (var key in base) opts[key] = base[key];
      opts.buttons = entry[1];
      var Ctor = type.indexOf('pointer') === 0 && window.PointerEvent ? PointerEvent : MouseEvent;
      try { el.dispatchEvent(new Ctor(type, opts)); } catch (e) {}
    });
  }

  function setNativeValue(el, value) {
    var proto = el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    var setter = Object.getOwnPropertyDescriptor(proto, 'value');
    if (setter && setter.set) setter.set.call(el, value);
    else el.value = value;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  function readEditor(el) {
    if (!el) return '';
    if (el.value !== undefined && el.value !== null) return String(el.value);

    // Slate 편집기의 textContent 에는 단락마다 붙는 화자 선택 버튼 이름("필재" 등)
    // 같은 UI 텍스트가 섞여 들어온다. 실제 대본은 [data-slate-string] 안에만 있다.
    var strings = el.querySelectorAll('[data-slate-string="true"]');
    if (strings.length) {
      return Array.prototype.map
        .call(strings, function (node) { return node.textContent || ''; })
        .join('\n');
    }
    return el.textContent || '';
  }

  // 편집기에 넣기 전 대본을 정리한다.
  // 빈 줄을 그대로 넣으면 Slate 가 빈 단락을 만들어 화자 선택 버튼만 늘어난다.
  function cleanScript(text) {
    return String(text || '')
      .replace(/\r\n?/g, '\n')
      .split('\n')
      .map(function (line) { return line.trim(); })
      .filter(function (line) { return line.length > 0; })
      .join('\n');
  }

  // Slate · ProseMirror 같은 편집기는 자체 paste 핸들러에서 줄바꿈을 단락으로 나눈다.
  // 사람이 ⌘V 로 붙여넣은 것과 같은 결과를 얻으려면 이 경로를 써야 한다.
  // 편집기가 처리했는지는 preventDefault 여부로 판단한다.
  function pasteInto(el, text) {
    try {
      var transfer = new DataTransfer();
      transfer.setData('text/plain', text);
      var event = new ClipboardEvent('paste', {
        bubbles: true,
        cancelable: true,
        clipboardData: transfer
      });
      el.dispatchEvent(event);
      return event.defaultPrevented;
    } catch (e) {
      return false;
    }
  }

  function firstTextNode(root) {
    if (!root) return null;
    if (root.nodeType === 3) return root;
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    return walker.nextNode();
  }

  function lastTextNode(root) {
    if (!root) return null;
    if (root.nodeType === 3) return root;
    var walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    var node = null;
    var next;
    while ((next = walker.nextNode())) node = next;
    return node;
  }

  // 편집기 내용을 전부 선택한다.
  //
  // Slate 에서는 execCommand('selectAll') 이 커서가 놓인 **문단 하나만** 선택하는 경우가 있다.
  // 그 상태로 붙여넣으면 첫 문단만 새 대본으로 바뀌고 이전 대본의 나머지 문단이 그대로 남는다.
  // 그래서 첫 텍스트 노드부터 마지막 텍스트 노드까지 범위를 직접 만들어 준다.
  function selectAllIn(el) {
    el.focus();
    var selection = window.getSelection();
    var range = document.createRange();

    var leaves = el.querySelectorAll('[data-slate-string="true"], [data-slate-zero-width]');
    var start = leaves.length ? firstTextNode(leaves[0]) : null;
    var end = leaves.length ? lastTextNode(leaves[leaves.length - 1]) : null;

    try {
      if (start && end) {
        range.setStart(start, 0);
        range.setEnd(end, end.nodeType === 3 ? end.length : end.childNodes.length);
      } else {
        range.selectNodeContents(el);
      }
      selection.removeAllRanges();
      selection.addRange(range);
      document.dispatchEvent(new Event('selectionchange'));
      return;
    } catch (e) {}

    try {
      document.execCommand('selectAll');
    } catch (e) {}
  }

  function editorIsEmpty(el) {
    return normalize(readEditor(el)).length === 0;
  }

  /** 편집기를 완전히 비운다. Slate 는 한 번의 delete 로 다 지워지지 않을 수 있어 반복한다. */
  function clearEditor(el, attempt, done) {
    attempt = attempt || 0;
    if (editorIsEmpty(el)) {
      done(true);
      return;
    }
    if (attempt >= 4) {
      done(false);
      return;
    }
    selectAllIn(el);
    var deleted = false;
    try { deleted = document.execCommand('delete'); } catch (e) {}
    if (!deleted) {
      // Slate 는 beforeinput 으로도 삭제를 처리한다.
      try {
        el.dispatchEvent(new InputEvent('beforeinput', {
          inputType: 'deleteContentBackward',
          bubbles: true,
          cancelable: true
        }));
      } catch (e) {}
    }
    setTimeout(function () { clearEditor(el, attempt + 1, done); }, 150);
  }

  /** 대본을 편집기에 넣고, 어떤 방식이 통했는지 돌려준다. */
  function fillEditor(el, text) {
    el.focus();
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      setNativeValue(el, text);
      return 'value';
    }

    selectAllIn(el);

    // 붙여넣기를 먼저 시도한다. Slate 는 paste 에서 줄바꿈을 단락으로 나눠 주지만
    // insertText 는 편집기에 따라 한 단락에 몰아넣기도 한다.
    if (pasteInto(el, text)) return 'paste';

    // paste 를 편집기가 처리하지 않았다면 다시 전체 선택 후 insertText 로 교체한다.
    // (혹시 일부라도 들어갔더라도 덧붙지 않고 대체되도록)
    selectAllIn(el);
    if (document.execCommand('insertText', false, text)) return 'insertText';

    el.textContent = text;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return 'textContent';
  }

  function normalize(value) {
    return String(value || '').replace(/\s+/g, '');
  }

  // 선택을 해제하고 캐럿을 본문 맨 앞으로 옮긴다.
  // insertText 직후에는 캐럿이 끝에 남는데, Typecast 는 커서 위치부터 낭독하므로
  // 이 처리를 하지 않으면 마지막 부분만 재생되거나 아무 소리도 나지 않는다.
  function collapseCaretToStart(el) {
    if (!el) return false;
    try {
      el.focus();
      if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
        el.setSelectionRange(0, 0);
        el.scrollTop = 0;
        el.dispatchEvent(new Event('select', { bubbles: true }));
        document.dispatchEvent(new Event('selectionchange'));
        return true;
      }

      var selection = window.getSelection();
      var range = document.createRange();

      // Slate 편집기는 단락마다 화자 선택 버튼(contenteditable="false")이 앞에 붙어 있어
      // 단순히 첫 텍스트 노드를 잡으면 편집 불가 영역에 캐럿을 놓게 된다.
      // 실제 대본 문자열 노드를 찾아 그 맨 앞에 놓는다.
      var firstString = el.querySelector('[data-slate-string="true"]');
      var anchor = null;
      if (firstString && firstString.firstChild && firstString.firstChild.nodeType === 3) {
        anchor = firstString.firstChild;
      } else {
        var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
          acceptNode: function (node) {
            var parent = node.parentElement;
            while (parent && parent !== el) {
              if (parent.getAttribute && parent.getAttribute('contenteditable') === 'false') {
                return NodeFilter.FILTER_REJECT;
              }
              parent = parent.parentElement;
            }
            return NodeFilter.FILTER_ACCEPT;
          }
        });
        anchor = walker.nextNode();
      }

      if (anchor) range.setStart(anchor, 0);
      else range.setStart(el, 0);
      range.collapse(true);
      selection.removeAllRanges();
      selection.addRange(range);
      el.scrollTop = 0;
      // ProseMirror 등은 selectionchange 로 내부 상태를 동기화한다.
      document.dispatchEvent(new Event('selectionchange'));
      return true;
    } catch (e) {
      return false;
    }
  }

  // 이 프레임에서 실제 작업을 수행한다. 대상이 없으면 조용히 false 를 돌려준다.
  function doPrepare(rawText) {
    var editor = findEditor();
    if (!editor) return false;
    // 빈 줄을 걷어내 빈 단락이 생기지 않게 한다.
    var text = cleanScript(rawText);
    editor.focus();

    // 이전 대본을 먼저 완전히 비운다. 비우지 않고 붙여넣으면 Slate 가
    // 커서가 놓인 문단만 교체해 이전 대본의 나머지 문단이 남는다.
    clearEditor(editor, 0, function (cleared) {
      var method;
      try {
        method = fillEditor(editor, text);
      } catch (e) {
        report('step:prepare-failed:입력 중 오류 ' + e);
        return;
      }

      setTimeout(function () {
        var actual = normalize(readEditor(editor));
        var expected = normalize(text);
        var head = expected.slice(0, 40);
        var paragraphs = editor.querySelectorAll('[data-slate-node="element"]').length;
        // 처음부터 낭독되도록 선택 해제 + 캐럿을 맨 앞으로 옮긴다.
        var caret = collapseCaretToStart(editor);

        var details =
          ' · ' + describe(editor) +
          ' · ' + method +
          (cleared ? '' : ' · 비우기실패') +
          (caret ? ' · 캐럿맨앞' : ' · 캐럿이동실패');

        if (actual.indexOf(head) < 0) {
          report('step:prepare-failed:입력 확인 실패' + details);
          return;
        }
        // 이전 대본이 남아 있으면 글자 수가 눈에 띄게 많아진다.
        if (actual.length > expected.length + 20) {
          report(
            'step:prepare-failed:이전 대본이 남아 있습니다 (기대 ' + expected.length +
            '자 / 실제 ' + actual.length + '자)' + details
          );
          return;
        }

        report(
          'step:prepared:' + actual.length + '자' +
          (paragraphs ? ' · ' + paragraphs + '단락' : '') + details
        );
      }, 500);
    });

    return true;
  }

  function doPlay() {
    intentionalStop = false;
    var button = findPlayButton();
    if (!button) return false;
    var source = playSource;
    // 입력 후 재생까지 사이에 커서가 움직였을 수 있으므로 직전에 다시 맨 앞으로 보낸다.
    var caret = collapseCaretToStart(findEditor());

    // 붙여넣기 직후에는 재생 버튼이 잠시 비활성일 수 있어 활성화될 때까지 기다린다.
    var attempts = 0;
    (function attempt() {
      attempts += 1;
      if (isDisabled(button)) {
        if (attempts < 25) {
          setTimeout(attempt, 200);
          return;
        }
        report('step:play-failed:재생 버튼이 계속 비활성 상태입니다 ' + describe(button));
        return;
      }

      // 클릭이 실제로 버튼까지 전달됐는지 확인해 진단에 남긴다.
      var target = closestButton(button);
      var delivered = false;
      var probeListener = function () { delivered = true; };
      target.addEventListener('click', probeListener, true);

      clickLikeUser(button);

      setTimeout(function () {
        target.removeEventListener('click', probeListener, true);
        var playing = false;
        deepQueryAll('audio, video').forEach(function (media) {
          if (!media.paused && !media.ended) playing = true;
        });
        report(
          'step:playing:' + describe(button) +
          (source ? ' · ' + source : '') +
          (caret ? ' · 캐럿맨앞' : '') +
          ' · 클릭' + (delivered ? '전달' : '미전달') +
          (playing ? ' · 미디어재생중' : '')
        );
      }, 700);
    })();
    return true;
  }

  function doStop() {
    intentionalStop = true;
    var button = findButton(STOP_HINTS, null);
    if (button) clickLikeUser(button);
    deepQueryAll('audio, video').forEach(function (el) {
      try { el.pause(); } catch (e) {}
    });
    return !!button;
  }

  // 진단: 이 프레임에 무엇이 있는지 최대한 자세히 보고한다.
  function doProbe() {
    var buttons = allButtons();
    var info = [
      'url=' + location.href.slice(0, 80),
      'top=' + (IS_TOP ? 'y' : 'n'),
      'iframes=' + window.frames.length,
      'textarea=' + deepQueryAll('textarea').length,
      'editable=' + deepQueryAll('[contenteditable="true"],[contenteditable=""]').length,
      'prosemirror=' + deepQueryAll('.ProseMirror').length,
      'textbox=' + deepQueryAll('[role="textbox"]').length,
      'canvas=' + deepQueryAll('canvas').length,
      'buttons=' + buttons.length,
      'editor=' + describe(findEditor()),
      'play=' + describe(findPlayButton()) + (playSource ? '(' + playSource + ')' : '')
    ];
    report('step:probe:' + info.join(' '));

    // 재생 버튼 선택자를 사람이 직접 고를 수 있도록 버튼 목록을 함께 낸다.
    var labels = buttons.slice(0, 14).map(describe).join(' | ');
    if (labels) report('step:probe-buttons:' + labels.slice(0, 600));
  }

  function handleRequest(request) {
    if (!request || !request.type) return;
    if (request.type === 'probe') doProbe();
    else if (request.type === 'prepare') { if (doPrepare(request.text)) markHandled('prepare'); }
    else if (request.type === 'play') { if (doPlay()) markHandled('play'); }
    else if (request.type === 'stop') doStop();
  }

  function markHandled(kind) {
    try {
      if (IS_TOP) window.__omnirecHandled[kind] = true;
      else window.top.postMessage({ __omnirec: 'handled', kind: kind }, '*');
    } catch (e) {}
  }

  window.__omnirecHandled = {};

  function collectFrames(win, out, depth) {
    out = out || [];
    depth = depth || 0;
    if (depth > 4) return out;
    try {
      for (var i = 0; i < win.frames.length; i++) {
        out.push(win.frames[i]);
        collectFrames(win.frames[i], out, depth + 1);
      }
    } catch (e) {}
    return out;
  }

  // 요청을 이 프레임과 모든 하위 프레임에 뿌린다.
  function dispatchRequest(request) {
    handleRequest(request);
    collectFrames(window).forEach(function (frame) {
      try { frame.postMessage({ __omnirec: 'request', request: request }, '*'); } catch (e) {}
    });
  }

  window.addEventListener('message', function (event) {
    var data = event.data;
    if (!data || typeof data !== 'object') return;
    if (data.__omnirec === 'request') handleRequest(data.request);
    else if (data.__omnirec === 'report' && IS_TOP) omnirecSend(data.message);
    else if (data.__omnirec === 'handled' && IS_TOP) window.__omnirecHandled[data.kind] = true;
  });

  if (IS_TOP) {
    window.__omnirecSetSelectors = function (editor, play) {
      selectors.editor = editor || '';
      selectors.play = play || '';
      dispatchRequest({ type: 'selectors', editor: editor, play: play });
    };

    window.__omnirecProbe = function () { dispatchRequest({ type: 'probe' }); };

    window.__omnirecPrepare = function (text) {
      window.__omnirecHandled.prepare = false;
      dispatchRequest({ type: 'prepare', text: text });
      // 어떤 프레임도 편집기를 찾지 못하면 실패로 보고한다.
      setTimeout(function () {
        if (!window.__omnirecHandled.prepare) {
          report('step:prepare-failed:편집기를 찾지 못했습니다 (모든 프레임 탐색)');
          toast('대본이 클립보드에 복사되었습니다. 편집기를 클릭하고 붙여넣기 하세요.', 'warn');
        }
      }, 1200);
    };

    window.__omnirecPlay = function () {
      window.__omnirecHandled.play = false;
      dispatchRequest({ type: 'play' });
      setTimeout(function () {
        if (!window.__omnirecHandled.play) {
          report('step:play-failed:재생 버튼을 찾지 못했습니다 (모든 프레임 탐색)');
        }
      }, 1200);
    };

    window.__omnirecStopPlayback = function () { dispatchRequest({ type: 'stop' }); };
  } else {
    // 서브프레임은 상위에서 온 selectors 요청도 처리해야 한다.
    var baseHandle = handleRequest;
    handleRequest = function (request) {
      if (request && request.type === 'selectors') {
        selectors.editor = request.editor || '';
        selectors.play = request.play || '';
        return;
      }
      baseHandle(request);
    };
  }

  // 미디어 엘리먼트로 재생하는 경우 정확한 시작/종료 신호를 얻는다.
  function hookMedia(el) {
    if (el.__omnirecHooked) return;
    el.__omnirecHooked = true;
    el.addEventListener('play', function () { report('step:media-play:'); });
    el.addEventListener('ended', function () { report('step:media-ended:'); });
    // ended 없이 pause 만 오는 경우: 페이지 쪽 오동작으로 재생이 중간에 멈춘 것.
    // 우리가 doStop 으로 직접 멈춘 경우(intentionalStop)는 제외한다.
    el.addEventListener('pause', function () {
      if (intentionalStop || el.ended) return;
      report('step:media-pause:' + describe(el));
    });
  }
  setInterval(function () {
    deepQueryAll('audio, video').forEach(hookMedia);
  }, 1500);
})();
"#
);

/// 앱이 대신 연 로그인 창에 주입되는 스크립트.
/// 별도 창이라 진짜 `window.opener` 가 없으므로 스텁을 심어,
/// 콜백 페이지의 `opener.postMessage(...)` 를 메인 창으로 중계한다.
const POPUP_INIT_SCRIPT: &str = concat!(
    r#"
(function () {
  if (window.__omnirecPopupInjected) return;
  window.__omnirecPopupInjected = true;
"#,
    bridge_snippet!(),
    r#"
  if (window.opener) return; // 진짜 opener 가 있으면 손대지 않는다.

  var openerStub = {
    closed: false,
    postMessage: function (data, targetOrigin) {
      try {
        omnirecSend('msg:' + JSON.stringify({ data: data, origin: window.location.origin }));
      } catch (e) {
        omnirecSend('log:opener.postMessage 직렬화 실패 ' + e);
      }
    },
    focus: function () {},
    close: function () {},
    location: { href: '', replace: function () {}, assign: function () {} }
  };

  try {
    Object.defineProperty(window, 'opener', {
      configurable: true,
      get: function () { return openerStub; },
      set: function () {}
    });
  } catch (e) {
    try { window.opener = openerStub; } catch (e2) {}
  }
})();
"#
);

/// 브리지 메시지 일련번호를 기억해 중복 전달을 한 번만 처리한다.
/// 두 채널(문서 제목 · 커스텀 스킴)이 같은 메시지를 보내기 때문에 필요하다.
/// 처음 보는 번호면 `true`.
fn seen_bridge_message(seq: &str) -> bool {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};

    static RECENT: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    let recent = RECENT.get_or_init(|| Mutex::new(VecDeque::with_capacity(64)));
    let Ok(mut guard) = recent.lock() else {
        return true;
    };
    if guard.iter().any(|s| s == seq) {
        return false;
    }
    if guard.len() >= 64 {
        guard.pop_front();
    }
    guard.push_back(seq.to_string());
    true
}

/// `encodeURIComponent` 로 인코딩된 문자열을 되돌린다.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, TypecastController};

    #[test]
    fn sign_in_pages_are_not_treated_as_signed_in() {
        for url in [
            "https://studio.typecast.ai/sign-in",
            "https://studio.typecast.ai/sign-up",
            "https://studio.typecast.ai/login?redirect=/text-to-speech",
            "https://typecast.ai/auth/callback",
        ] {
            assert!(!TypecastController::looks_signed_in(url), "{url}");
        }
    }

    #[test]
    fn workspace_pages_are_treated_as_signed_in() {
        for url in [
            "https://studio.typecast.ai/text-to-speech",
            "https://studio.typecast.ai/",
            "https://studio.typecast.ai/voice-casting",
            "https://app.typecast.ai/ko/editor/123",
        ] {
            assert!(TypecastController::looks_signed_in(url), "{url}");
        }
    }

    #[test]
    fn decodes_encode_uri_component_output() {
        assert_eq!(
            percent_decode(
                "https%3A%2F%2Faccounts.google.com%2Fo%2Foauth2%2Fv2%2Fauth%3Fscope%3Demail"
            ),
            "https://accounts.google.com/o/oauth2/v2/auth?scope=email"
        );
        // 인코딩되지 않은 문자열은 그대로 통과한다.
        assert_eq!(percent_decode("https://typecast.ai/"), "https://typecast.ai/");
        // 잘린 이스케이프는 원문 그대로 둔다.
        assert_eq!(percent_decode("abc%2"), "abc%2");
    }
}
