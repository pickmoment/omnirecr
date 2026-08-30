//! Typecast 자동화.
//!
//! 앱 내장 웹뷰(WKWebView)가 아니라, 사용자가 실제로 보는 별도의 Google Chrome 프로세스를
//! Chrome DevTools Protocol(CDP, `chromiumoxide` 크레이트)로 제어한다. 이유:
//!
//! 1. WKWebView 는 창이 가려지거나 최소화되면 배터리 절약을 위해 그 프로세스를
//!    스로틀링/서스펜드한다. 재생 중인 오디오가 정지 버튼을 누르지 않았는데도
//!    멈추는 사고의 실제 원인 중 하나였다.
//! 2. WKWebView 는 사용자 제스처와 분리된 `window.open` 을 차단한다. Typecast 의 소셜
//!    로그인(팝업 + `window.opener.postMessage`)은 비동기 흐름에서 팝업을 열기 때문에,
//!    프록시 window 객체 + opener 스텁으로 팝업을 흉내 내는 코드가 400줄 넘게 필요했다.
//! 3. 실제 Chrome 은 이 둘 다 겪지 않는다. 팝업은 진짜 `window.opener` 관계를 유지해
//!    Typecast 자신의 `postMessage` 코드가 그대로 동작하고, `popup.close()` 도 실제
//!    창을 닫는다. (2)의 코드 전체가 필요 없어졌다.
//!
//! 로그인 세션은 앱 전용 Chrome 프로필 디렉터리(`~/.omnirec/typecast-chrome-profile`)에
//! 영구 저장된다. 사용자의 평소 개인 Chrome 프로필과는 절대 공유하지 않는다 — Chrome
//! 136+ 는 기본 프로필에 대한 원격 디버깅 자체를 거부하기도 하고, 자동화가 사용자의
//! 실제 브라우징 세션(로그인된 다른 사이트들)에 손대는 것도 피해야 한다.
//!
//! 페이지 → 앱 브리지는 CDP `Runtime.addBinding` 하나로 단순화됐다. 원격 오리진이라
//! Tauri IPC 가 막혀 있던 WKWebView 시절에는 `document.title` 변경 + 커스텀 스킴
//! 네비게이션이라는 이중 채널과 일련번호 중복 제거가 필요했지만, CDP 바인딩은 페이지
//! JS 가 직접 호출하는 진짜 함수라 그런 우회가 필요 없다.

use futures::StreamExt;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex as AsyncMutex;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::EventFrameNavigated;
use chromiumoxide::cdp::browser_protocol::storage::ClearDataForOriginParams;
use chromiumoxide::cdp::browser_protocol::target::EventTargetCreated;
use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
use chromiumoxide::Page;

use crate::settings::SettingsManager;
use crate::types::{
    TypecastBrowserState, TypecastDebugPayload, TypecastNavigationPayload, TypecastPopupPayload,
    TypecastStepPayload,
};

/// 페이지 → 앱 브리지에 쓰는 CDP 바인딩 이름. 최상위 프레임에서만 호출된다.
const BRIDGE_BINDING_NAME: &str = "__omnirecBridge";

/// 로그인 화면으로 판단할 URL 경로 조각.
const SIGN_IN_PATH_HINTS: [&str; 6] = [
    "/sign-in",
    "/signin",
    "/login",
    "/sign-up",
    "/signup",
    "/auth",
];

/// 실행 중인 Chrome 세션 하나. 앱 전체에 Typecast 창은 하나만 존재하는 모델이라
/// 세션도 하나만 유지한다.
struct CdpSession {
    browser: Browser,
    main_page: Page,
    handler_task: tokio::task::JoinHandle<()>,
    binding_task: tokio::task::JoinHandle<()>,
    navigation_task: tokio::task::JoinHandle<()>,
    target_task: tokio::task::JoinHandle<()>,
}

impl CdpSession {
    async fn shutdown(mut self) {
        self.handler_task.abort();
        self.binding_task.abort();
        self.navigation_task.abort();
        self.target_task.abort();
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
    }
}

/// Tauri 관리 상태. 세션이 없으면 `None`.
pub struct TypecastCdpState(AsyncMutex<Option<CdpSession>>);

impl TypecastCdpState {
    pub fn new() -> Self {
        Self(AsyncMutex::new(None))
    }
}

impl Default for TypecastCdpState {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn parse_url(url: &str) -> Result<String, String> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err("접속할 주소가 비어 있습니다.".to_string());
        }
        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
            return Err(format!("http/https 주소만 열 수 있습니다: {}", trimmed));
        }
        Ok(trimmed.to_string())
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

    /// 페이지가 CDP 바인딩으로 보낸 메시지를 처리한다. 형식은 `<kind>:<payload>`.
    /// 지금은 `step:<name>:<detail>` (자동화 단계 보고) 하나만 의미 있게 처리한다.
    /// `open`/`close`/`msg` 같은 팝업 브리지 케이스는 실제 브라우저에서는 필요 없다 —
    /// 진짜 팝업의 `window.opener.postMessage` / `popup.close()` 가 그대로 동작한다.
    fn handle_bridge_message(app: &AppHandle, message: &str) {
        let (kind, payload) = match message.split_once(':') {
            Some((k, p)) => (k, p),
            None => (message, ""),
        };

        Self::debug(app, &format!("bridge:{kind}"), payload);

        if kind == "step" {
            let (name, detail) = match payload.split_once(':') {
                Some((n, d)) => (n.to_string(), d.to_string()),
                None => (payload.to_string(), String::new()),
            };
            let _ = app.emit("typecast_step", TypecastStepPayload { name, detail });
        }
    }

    /// macOS 에서 Chrome 앱 자체를 최전면으로 올린다. `Page.bringToFront` 는 탭만
    /// 활성화할 뿐 OS 레벨에서 다른 앱 뒤에 있는 Chrome 창을 끌어올리지는 못한다.
    #[cfg(target_os = "macos")]
    fn activate_chrome_app() {
        let _ = std::process::Command::new("open")
            .arg("-a")
            .arg("Google Chrome")
            .spawn();
    }

    #[cfg(not(target_os = "macos"))]
    fn activate_chrome_app() {}

    fn cdp_state(app: &AppHandle) -> tauri::State<'_, TypecastCdpState> {
        app.state::<TypecastCdpState>()
    }

    async fn get_main_page(app: &AppHandle) -> Result<Page, String> {
        let state = Self::cdp_state(app);
        let guard = state.0.lock().await;
        guard
            .as_ref()
            .map(|session| session.main_page.clone())
            .ok_or_else(|| "Typecast 브라우저가 열려 있지 않습니다.".to_string())
    }

    /// Typecast 를 연다. 이미 열려 있으면 탭 활성화 + 앱 활성화만 한다.
    ///
    /// 창은 앱 전용 Chrome 프로필(`SettingsManager::typecast_chrome_profile_dir`)로
    /// 실행되어 로그인 쿠키가 영구 저장되고, 다음 실행에서도 같은 세션으로 자동 접속된다.
    pub async fn open(app: &AppHandle, url: Option<String>) -> Result<(), String> {
        let settings = SettingsManager::load();
        let target = url.unwrap_or_else(|| settings.typecast_editor_url.clone());
        let target = Self::parse_url(&target)?;

        let state = Self::cdp_state(app);
        {
            let guard = state.0.lock().await;
            if let Some(session) = guard.as_ref() {
                let _ = session.main_page.bring_to_front().await;
                drop(guard);
                Self::activate_chrome_app();
                return Ok(());
            }
        }

        let chrome_path = SettingsManager::find_chrome(settings.custom_chrome_path.as_deref())?;
        let profile_dir = SettingsManager::typecast_chrome_profile_dir();

        let config = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(profile_dir)
            .with_head()
            .window_size(1180, 840)
            // 합성 클릭으로도 재생되도록 자동재생 사용자 제스처 요구를 끈다.
            // (WKWebView 시절 mediaTypesRequiringUserActionForPlayback = None 과 같은 목적.)
            .arg("--autoplay-policy=no-user-gesture-required")
            .build()
            .map_err(|e| format!("Chrome 실행 설정을 만들 수 없습니다: {}", e))?;

        let (mut browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| format!("Chrome 을 실행할 수 없습니다: {}", e))?;

        let handler_app = app.clone();
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
            // 이벤트 스트림이 끝났다는 것은 연결이 끊겼다는 뜻이다(사용자가 Chrome 을 직접 닫은 경우 포함).
            let _ = handler_app.emit("typecast_browser_closed", ());
        });

        let setup: Result<
            (
                Page,
                tokio::task::JoinHandle<()>,
                tokio::task::JoinHandle<()>,
                tokio::task::JoinHandle<()>,
            ),
            String,
        > = async {
            // about:blank 로 먼저 만들어 실제 콘텐츠가 로드되기 전에 바인딩/초기 스크립트를 심는다.
            let page = browser
                .new_page("about:blank")
                .await
                .map_err(|e| format!("Typecast 페이지를 열 수 없습니다: {}", e))?;

            page.execute(AddBindingParams::new(BRIDGE_BINDING_NAME))
                .await
                .map_err(|e| format!("브리지 바인딩 등록 실패: {}", e))?;
            page.evaluate_on_new_document(MAIN_INIT_SCRIPT)
                .await
                .map_err(|e| format!("자동화 스크립트 등록 실패: {}", e))?;

            let mut binding_events = page
                .event_listener::<EventBindingCalled>()
                .await
                .map_err(|e| format!("브리지 이벤트 구독 실패: {}", e))?;
            let mut nav_events = page
                .event_listener::<EventFrameNavigated>()
                .await
                .map_err(|e| format!("네비게이션 이벤트 구독 실패: {}", e))?;
            let mut popup_events = browser
                .event_listener::<EventTargetCreated>()
                .await
                .map_err(|e| format!("팝업 감지 구독 실패: {}", e))?;

            page.goto(target.as_str())
                .await
                .map_err(|e| format!("Typecast 페이지로 이동할 수 없습니다: {}", e))?;

            let bridge_app = app.clone();
            let binding_task = tokio::spawn(async move {
                while let Some(event) = binding_events.next().await {
                    if event.name == BRIDGE_BINDING_NAME {
                        Self::handle_bridge_message(&bridge_app, &event.payload);
                    }
                }
            });

            let nav_app = app.clone();
            let navigation_task = tokio::spawn(async move {
                while let Some(event) = nav_events.next().await {
                    // 최상위 프레임 네비게이션만 로그인 상태 판정에 쓴다.
                    if event.frame.parent_id.is_some() {
                        continue;
                    }
                    let url = event.frame.url.clone();
                    Self::debug(&nav_app, "navigate", &url);
                    let _ = nav_app.emit(
                        "typecast_navigation",
                        TypecastNavigationPayload {
                            looks_signed_in: Self::looks_signed_in(&url),
                            url,
                        },
                    );
                }
            });

            let popup_app = app.clone();
            let target_task = tokio::spawn(async move {
                while let Some(event) = popup_events.next().await {
                    if event.target_info.r#type != "page" {
                        continue;
                    }
                    Self::debug(&popup_app, "popup-opened", &event.target_info.url);
                    let _ = popup_app.emit(
                        "typecast_popup_intercepted",
                        TypecastPopupPayload {
                            url: event.target_info.url.clone(),
                        },
                    );
                }
            });

            Ok((page, binding_task, navigation_task, target_task))
        }
        .await;

        let (page, binding_task, navigation_task, target_task) = match setup {
            Ok(parts) => parts,
            Err(error) => {
                handler_task.abort();
                let _ = browser.close().await;
                return Err(error);
            }
        };

        let mut guard = state.0.lock().await;
        *guard = Some(CdpSession {
            browser,
            main_page: page,
            handler_task,
            binding_task,
            navigation_task,
            target_task,
        });
        drop(guard);

        Self::activate_chrome_app();
        Ok(())
    }

    pub async fn close(app: &AppHandle) -> Result<(), String> {
        let state = Self::cdp_state(app);
        let session = {
            let mut guard = state.0.lock().await;
            guard.take()
        };
        if let Some(session) = session {
            session.shutdown().await;
        }
        Ok(())
    }

    pub async fn focus(app: &AppHandle) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        page.bring_to_front()
            .await
            .map_err(|e| format!("탭 활성화 실패: {}", e))?;
        Self::activate_chrome_app();
        Ok(())
    }

    pub async fn navigate(app: &AppHandle, url: String) -> Result<(), String> {
        let target = Self::parse_url(&url)?;
        let state = Self::cdp_state(app);
        let existing = {
            let guard = state.0.lock().await;
            guard.as_ref().map(|session| session.main_page.clone())
        };
        match existing {
            Some(page) => page
                .goto(target.as_str())
                .await
                .map(|_| ())
                .map_err(|e| format!("페이지 이동 실패: {}", e)),
            None => Self::open(app, Some(url)).await,
        }
    }

    pub async fn go_back(app: &AppHandle) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        page.evaluate("history.back();")
            .await
            .map(|_| ())
            .map_err(|e| format!("뒤로 가기 실패: {}", e))
    }

    pub async fn reload(app: &AppHandle) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        page.reload()
            .await
            .map(|_| ())
            .map_err(|e| format!("새로고침 실패: {}", e))
    }

    /// 저장된 로그인 세션(쿠키/스토리지)을 모두 지우고 로그인 페이지로 되돌린다.
    ///
    /// 세션이 열려 있으면 CDP 로 쿠키/오리진 스토리지를 지운 뒤 로그인 페이지로 이동한다.
    /// 세션이 없으면 쿠키가 디스크의 프로필 디렉터리에만 존재하므로, 그 디렉터리 자체를
    /// 지우는 쪽이 더 확실하다(Chrome 이 실행 중이 아니므로 파일 삭제가 안전하다).
    pub async fn clear_session(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();
        let state = Self::cdp_state(app);

        let session_open = {
            let guard = state.0.lock().await;
            if let Some(session) = guard.as_ref() {
                session
                    .browser
                    .clear_cookies()
                    .await
                    .map_err(|e| format!("쿠키 삭제 실패: {}", e))?;
                if let Ok(origin_url) = tauri::Url::parse(&settings.typecast_editor_url) {
                    let _ = session
                        .main_page
                        .execute(ClearDataForOriginParams::new(
                            origin_url.origin().ascii_serialization(),
                            "all",
                        ))
                        .await;
                }
                if let Ok(signin) = Self::parse_url(&settings.typecast_signin_url) {
                    let _ = session.main_page.goto(signin.as_str()).await;
                }
                true
            } else {
                false
            }
        };

        if !session_open {
            let _ = std::fs::remove_dir_all(SettingsManager::typecast_chrome_profile_dir());
        }

        let mut updated = settings;
        updated.typecast_session_saved = false;
        updated.typecast_last_login_at = None;
        SettingsManager::save(&updated)?;

        Ok(())
    }

    pub async fn state(app: &AppHandle) -> TypecastBrowserState {
        let settings = SettingsManager::load();
        let cdp_state = Self::cdp_state(app);
        let guard = cdp_state.0.lock().await;
        let (is_open, current_url) = match guard.as_ref() {
            Some(session) => (true, session.main_page.url().await.ok().flatten()),
            None => (false, None),
        };
        drop(guard);

        TypecastBrowserState {
            is_open,
            looks_signed_in: current_url
                .as_deref()
                .map(Self::looks_signed_in)
                .unwrap_or(settings.typecast_session_saved),
            current_url,
            account_email: settings.typecast_account_email.clone(),
            last_login_at: settings.typecast_last_login_at.clone(),
        }
    }

    /// 자동화용 선택자를 페이지에 심는다. 비워두면 내장 휴리스틱을 쓴다.
    pub async fn apply_selectors(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();
        let page = Self::get_main_page(app).await?;
        let editor = serde_json::to_string(&settings.typecast_editor_selector)
            .map_err(|e| e.to_string())?;
        let play =
            serde_json::to_string(&settings.typecast_play_selector).map_err(|e| e.to_string())?;
        page.evaluate(format!(
            "window.__omnirecSetSelectors && window.__omnirecSetSelectors({}, {});",
            editor, play
        ))
        .await
        .map(|_| ())
        .map_err(|e| format!("선택자 적용 실패: {}", e))
    }

    /// 대본을 편집기에 채우고, 결과를 `typecast_step` 이벤트로 보고한다.
    ///
    /// 자동 입력이 실패하더라도 사용자가 직접 붙여넣을 수 있도록 클립보드에도 넣어 둔다.
    /// 수동 녹음 화면의 "대본 보내기"도 같은 경로를 쓴다.
    pub async fn prepare_script(app: &AppHandle, text: String) -> Result<(), String> {
        let _ = crate::clipboard::copy_text(&text);
        Self::apply_selectors(app).await?;
        let page = Self::get_main_page(app).await?;
        let payload = serde_json::to_string(&text).map_err(|e| e.to_string())?;
        page.evaluate(format!(
            "window.__omnirecPrepare && window.__omnirecPrepare({});",
            payload
        ))
        .await
        .map_err(|e| format!("대본 주입 실패: {}", e))?;
        let _ = page.bring_to_front().await;
        Ok(())
    }

    /// 편집기의 재생 버튼을 누른다.
    pub async fn play(app: &AppHandle) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        page.evaluate("window.__omnirecPlay && window.__omnirecPlay();")
            .await
            .map(|_| ())
            .map_err(|e| format!("재생 실행 실패: {}", e))
    }

    /// 재생을 멈춘다(정지/일시정지 버튼 탐색).
    pub async fn stop_playback(app: &AppHandle) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        page.evaluate("window.__omnirecStopPlayback && window.__omnirecStopPlayback();")
            .await
            .map(|_| ())
            .map_err(|e| format!("재생 정지 실패: {}", e))
    }

    /// 편집기 / 재생 버튼 후보를 찾아 진단 정보를 보고한다.
    pub async fn probe(app: &AppHandle) -> Result<(), String> {
        Self::apply_selectors(app).await?;
        let page = Self::get_main_page(app).await?;
        page.evaluate("window.__omnirecProbe && window.__omnirecProbe();")
            .await
            .map(|_| ())
            .map_err(|e| format!("페이지 진단 실패: {}", e))
    }

    /// Typecast 페이지 위에 안내 토스트를 띄운다(카운트다운 / 녹음 시작 알림 용도).
    pub async fn notify(app: &AppHandle, message: String, tone: Option<String>) -> Result<(), String> {
        let page = Self::get_main_page(app).await?;
        let msg = serde_json::to_string(&message).map_err(|e| e.to_string())?;
        let tone = serde_json::to_string(&tone.unwrap_or_else(|| "info".to_string()))
            .map_err(|e| e.to_string())?;
        page.evaluate(format!(
            "window.__omnirecToast && window.__omnirecToast({}, {});",
            msg, tone
        ))
        .await
        .map(|_| ())
        .map_err(|e| format!("Typecast 알림 표시 실패: {}", e))
    }
}

/// Typecast 페이지에 주입되는 자동화 스크립트. **모든 프레임**에서 실행된다
/// (`Page.addScriptToEvaluateOnNewDocument` 는 새로 생성되는 모든 프레임에 적용되고,
/// 그 프레임의 스크립트가 실행되기 전에 먼저 돈다).
///
/// - 편집기 입력 · 재생 버튼 클릭 · 진단. 편집기가 iframe 이나 shadow DOM 안에 있을 수
///   있으므로 모든 프레임에 주입하고 shadow root 까지 훑는다.
/// - `window.open`/`postMessage` 관련 우회 코드가 전혀 없다 — 실제 Chrome 에서는
///   Typecast 자신의 팝업 로그인 코드가 그대로 동작한다.
const MAIN_INIT_SCRIPT: &str = r#"
(function () {
  if (window.__omnirecInjected) return;
  window.__omnirecInjected = true;

  var IS_TOP = (function () { try { return window.top === window; } catch (e) { return false; } })();

  // 최상위 프레임만 앱과 직접 통신한다(CDP 바인딩은 최상위 실행 컨텍스트에서만 보장된다).
  // 서브프레임은 top 으로 postMessage 해서 중계한다.
  function omnirecSend(message) {
    try { window.__omnirecBridge && window.__omnirecBridge(message); } catch (e) {}
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
"#;

#[cfg(test)]
mod tests {
    use super::TypecastController;

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
}
