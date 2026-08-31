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
use chromiumoxide::{Handler, Page};

use crate::settings::SettingsManager;
use crate::types::{
    TypecastBrowserState, TypecastDebugPayload, TypecastNavigationPayload, TypecastPopupPayload,
    TypecastStepPayload,
};

/// 페이지 → 앱 브리지에 쓰는 CDP 바인딩 이름. 최상위 프레임에서만 호출된다.
const BRIDGE_BINDING_NAME: &str = "__omnirecBridge";

/// 세션이 살아 있는지 확인하는 CDP 왕복의 제한 시간.
/// 죽은 브라우저는 응답이 아예 오지 않으므로 "안 오면 죽은 것"으로 판정한다.
const LIVENESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 브라우저 종료(close + wait)에 허용하는 시간.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 네비게이션 완료를 기다리는 예산. `page.goto()` 의 하드코딩된 30초를 쓰지 않고
/// 우리가 폴링하므로 값도 우리가 정한다(무거운 SPA 는 30초를 쉽게 넘긴다).
const NAVIGATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// 브리지 페이로드의 바이트 상한.
///
/// 바인딩(`__omnirecBridge`)은 **원격 페이지에 그대로 노출된 함수**다. 페이지 코드나
/// 거기 끼어든 서드파티 스크립트가 임의 크기 문자열을 밀어 넣을 수 있으므로, 파싱
/// 전에 크기부터 본다. 이게 없으면 한 번의 호출로 Tauri 이벤트 큐와 프론트엔드 진단
/// 로그가 통째로 잠긴다.
const MAX_BRIDGE_PAYLOAD_BYTES: usize = 16 * 1024;

/// `typecast_step` 으로 내보내는 단계 이름 / 상세의 상한(문자 수).
/// UTF-8 경계에서 자르기 위해 바이트가 아니라 문자 수로 센다(`debug()` 와 같은 방식).
const MAX_STEP_NAME_CHARS: usize = 64;
const MAX_STEP_DETAIL_CHARS: usize = 600;

/// 세션 세대 발급기.
///
/// `discard_dead_session` 이 **자기 세션만** 버리도록 하는 표식이다. 이 값이 없으면
/// 뒤늦게 종료된 옛 handler 태스크가 방금 만든 새 세션을 지운다.
static SESSION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 네비게이션 표식 발급기. 이동 전 문서에 심어 두고, 이 값이 사라진 것으로
/// "새 문서가 커밋됐다"를 판정한다(`navigate_and_wait`).
static NAV_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 로그인 화면으로 판단할 URL 경로 조각.
const SIGN_IN_PATH_HINTS: [&str; 6] = [
    "/sign-in", "/signin", "/login", "/sign-up", "/signup", "/auth",
];

/// 실행 중인 Chrome 세션 하나. 앱 전체에 Typecast 창은 하나만 존재하는 모델이라
/// 세션도 하나만 유지한다.
struct CdpSession {
    /// 이 세션의 세대. 슬롯에 있는 세션이 "내가 만든 그 세션"인지 확인하는 데 쓴다.
    /// 세션 객체를 비교할 수단이 없어(Browser 는 Clone/Eq 둘 다 아니다) 번호로 판정한다.
    id: u64,
    browser: Browser,
    main_page: Page,
    handler_task: tokio::task::JoinHandle<()>,
    binding_task: tokio::task::JoinHandle<()>,
    navigation_task: tokio::task::JoinHandle<()>,
    target_task: tokio::task::JoinHandle<()>,
}

impl CdpSession {
    async fn shutdown(mut self) {
        // browser.close() 는 CDP 요청/응답 왕복이라 handler_task(연결을 실제로 읽는 루프)가
        // 계속 돌고 있어야 응답을 받는다. handler_task 를 먼저 abort 하면 close() 가
        // 영원히 응답을 못 받아 멈춘다 — 반드시 close() → wait() 다음에 태스크들을 정리한다.
        //
        // 브라우저가 **이미 죽어 있으면** 그 응답은 영영 오지 않는다(사용자가 Chrome 창을
        // 직접 닫은 뒤 앱의 닫기를 누른 경우). 시간 제한을 걸어 반드시 태스크 정리까지
        // 도달하게 한다 — 여기서 멈추면 다음 `open()` 이 상태 잠금을 못 얻어 창이 안 뜬다.
        {
            let browser = &mut self.browser;
            let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, async move {
                let _ = browser.close().await;
                let _ = browser.wait().await;
            })
            .await;
        }
        self.abort_tasks();
    }

    /// CDP 왕복 없이 백그라운드 태스크만 정리한다. 브라우저가 이미 죽은 경우에 쓴다.
    fn abort_tasks(&self) {
        self.handler_task.abort();
        self.binding_task.abort();
        self.navigation_task.abort();
        self.target_task.abort();
    }
}

/// Tauri 관리 상태.
///
/// **잠금 순서는 `transition` → `session` 이며, 반대로 잡지 말 것.**
///
/// - `session`: 세션 슬롯(없으면 `None`). **CDP `await` 너머로 들고 있지 말 것** —
///   죽은 세션의 응답을 기다리는 동안 붙들면, 연결이 끊겨 세션을 정리하려는 handler
///   태스크가 이 잠금을 못 얻어 서로 영원히 기다린다. 핸들(`Page` 클론)만 꺼내거나
///   세션을 통째로 `take()` 한 뒤 잠금을 놓고 왕복한다.
/// - `transition`: open/close/clear_session 전이 직렬화용. 이 잠금은 CDP 왕복을 넘어
///   들고 있어도 된다 — handler 태스크(`discard_dead_session`)는 `session` 만 잡으므로
///   교착이 생기지 않는다. 대신 이 잠금을 든 상태에서 `session` 을 CDP `await` 너머로
///   겹쳐 잡지 말 것(그 순간 위의 교착이 그대로 되살아난다).
pub struct TypecastCdpState {
    session: AsyncMutex<Option<CdpSession>>,
    transition: AsyncMutex<()>,
}

impl TypecastCdpState {
    pub fn new() -> Self {
        Self {
            session: AsyncMutex::new(None),
            transition: AsyncMutex::new(()),
        }
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
        // 크기부터 본다. 원격 페이지가 부르는 함수이므로 내용은 신뢰할 수 없다.
        if message.len() > MAX_BRIDGE_PAYLOAD_BYTES {
            Self::debug(
                app,
                "bridge:oversized",
                &format!(
                    "{}바이트 페이로드를 버렸습니다(상한 {}바이트)",
                    message.len(),
                    MAX_BRIDGE_PAYLOAD_BYTES
                ),
            );
            return;
        }

        let (kind, payload) = match message.split_once(':') {
            Some((k, p)) => (k, p),
            None => (message, ""),
        };

        Self::debug(app, &format!("bridge:{kind}"), payload);

        if kind == "step" {
            let (name, detail) = match payload.split_once(':') {
                Some((n, d)) => (n, d),
                None => (payload, ""),
            };
            // 상한을 넘기면 자른다. 문자 단위로 세어 UTF-8 경계를 깨지 않는다 —
            // 바이트로 자르면 한글 진단 문자열 중간이 잘려 프론트엔드가 깨진 글자를 받는다.
            let name: String = name.chars().take(MAX_STEP_NAME_CHARS).collect();
            let detail: String = detail.chars().take(MAX_STEP_DETAIL_CHARS).collect();
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

    /// 남아 있는 세션 객체를 버린다. `browser.close()` 는 부르지 않는다 — 이 경로는
    /// 브라우저가 이미 죽었을 때만 쓰이고, CDP 응답을 기다리면 영원히 멈춘다.
    ///
    /// **슬롯의 세션이 `id` 와 같을 때만** 버린다. 세대 검증이 없던 시절에는 뒤늦게
    /// 종료된 옛 handler 태스크가 슬롯을 무조건 `take()` 해, 그 사이 새로 만들어진
    /// 멀쩡한 세션을 지우고 그 백그라운드 태스크 4개까지 abort 했다 — 창은 떠 있는데
    /// 브리지·네비게이션 이벤트가 전부 죽어 자동 녹음이 조용히 멈춘다.
    async fn discard_dead_session(app: &AppHandle, id: u64) {
        let session = {
            let state = Self::cdp_state(app);
            let mut guard = state.session.lock().await;
            // 세대가 다르면 손대지 않는다(슬롯의 세션은 내 것이 아니다).
            if guard.as_ref().map(|session| session.id) == Some(id) {
                guard.take()
            } else {
                None
            }
        };
        if let Some(session) = session {
            session.abort_tasks();
        }
    }

    /// 살아 있는 메인 페이지와 그 URL 을 돌려준다. 세션이 없거나 죽었으면 `None`.
    ///
    /// 세션 객체가 남아 있다고 브라우저가 살아 있는 것은 아니다 — 사용자가 Chrome 창을
    /// 직접 닫거나 프로세스가 죽으면 객체만 남고 CDP 응답은 오지 않는다. 이 상태를 걸러
    /// 내지 않으면 다음 `open()` 이 "이미 열려 있다"고 판단해 창을 새로 띄우지 않는다
    /// (실측: 한 번 녹음한 뒤 창을 닫고 다시 시작하면 브라우저가 안 뜨는 증상).
    ///
    /// **상태 잠금을 CDP `await` 너머로 들고 있지 말 것.** 죽은 세션의 응답을 기다리는
    /// 동안 잠금을 붙들면, 연결이 끊겼을 때 세션을 정리하려는 handler 태스크가 그 잠금을
    /// 못 얻어 서로 영원히 기다린다.
    async fn live_main_page(app: &AppHandle) -> Option<(Page, Option<String>)> {
        // 세대까지 함께 꺼낸다 — 왕복이 실패했을 때 버려야 할 대상은 **그때 확인한 그
        // 세션**이다. 그 사이 새 세션이 설치됐다면 그것을 지워선 안 된다.
        let (page, id) = {
            let state = Self::cdp_state(app);
            let guard = state.session.lock().await;
            guard
                .as_ref()
                .map(|session| (session.main_page.clone(), session.id))
        }?;

        match tokio::time::timeout(LIVENESS_TIMEOUT, page.url()).await {
            Ok(Ok(url)) => Some((page, url)),
            _ => {
                Self::debug(
                    app,
                    "session-dead",
                    "브라우저가 응답하지 않아 세션을 정리합니다",
                );
                Self::discard_dead_session(app, id).await;
                None
            }
        }
    }

    async fn get_main_page(app: &AppHandle) -> Result<Page, String> {
        let state = Self::cdp_state(app);
        let guard = state.session.lock().await;
        guard
            .as_ref()
            .map(|session| session.main_page.clone())
            .ok_or_else(|| "Typecast 브라우저가 열려 있지 않습니다.".to_string())
    }

    /// 남은 데드라인 안에서 페이지 JS 를 평가하고 결과 값을 그대로 돌려준다
    /// (반환값이 `undefined` 면 `Ok(None)`).
    ///
    /// 개별 왕복에 시간 제한이 없으면 폴링 한 번이 멈추는 것만으로 전체 예산을 넘겨
    /// 커맨드 타임아웃까지 밀린다 — 그러면 프론트엔드는 "이동 실패"가 아니라 "앱이
    /// 멈췄다"를 보게 된다.
    async fn eval_before(
        page: &Page,
        script: String,
        deadline: tokio::time::Instant,
    ) -> Result<Option<serde_json::Value>, String> {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("남은 시간이 없습니다".to_string());
        }
        match tokio::time::timeout(remaining, page.evaluate(script)).await {
            Ok(Ok(result)) => Ok(result.value().cloned()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("응답 없음(시간 초과)".to_string()),
        }
    }

    /// 프래그먼트(`#…`)만 다른 이동인가?
    ///
    /// 그런 이동은 같은 문서 안에서 처리되어 문서가 새로 만들어지지 않으므로,
    /// "이동 표식이 사라졌는가"로는 완료를 영원히 판정할 수 없다. 두 URL 이 완전히
    /// 같은 경우는 여기에 해당하지 않는다 — `location.replace()` 가 문서를 다시
    /// 로드하므로 표식 경로가 맞다.
    fn fragment_only_change(current: &str, target: &str) -> bool {
        let mut from = match tauri::Url::parse(current) {
            Ok(url) => url,
            Err(_) => return false,
        };
        let mut to = match tauri::Url::parse(target) {
            Ok(url) => url,
            Err(_) => return false,
        };
        if to.fragment().is_none() || from.fragment() == to.fragment() {
            return false;
        }
        from.set_fragment(None);
        to.set_fragment(None);
        from == to
    }

    /// `page.goto()`(CDP `Page.navigate`)는 chromiumoxide 0.9.1 내부에 **하드코딩된 30초**
    /// 프레임 라이프사이클 타임아웃이 있다(`BrowserConfig::request_timeout` 과 무관하게
    /// `FrameNavigationRequest::new` 가 항상 `REQUEST_TIMEOUT` 상수를 그대로 씀 — 빌더로
    /// 늘릴 수 없다). Typecast 같은 무거운 SPA 는 분석 스크립트 등 3rd-party 리소스가 느리면
    /// `load` 이벤트가 30초를 넘겨 실측으로 타임아웃이 났다(예: `studio.typecast.ai`).
    ///
    /// `Page.navigate` CDP 커맨드 자체를 보내지 않고 `location.replace()` JS 로 이동시킨
    /// 뒤 우리가 직접 폴링해, 타임아웃 값도 우리가 정한다.
    ///
    /// **`document.readyState` 만 봐서는 안 된다.** `location.replace()` 는 비동기라서
    /// 호출이 돌아온 뒤에도 잠시 **옛 문서**가 그대로 살아 있고, 그 문서의 readyState 는
    /// 이미 `'complete'` 다. 그래서 첫 폴링이 옛 문서에서 통과해, 아무것도 로드되지
    /// 않았는데 "이동 완료"로 넘어간다(그 다음 조작이 사라진 컨텍스트에서 실패한다).
    /// 이동 전 문서에 표식(`window.__omnirecNavToken`)을 심고, **그 표식이 사라진**
    /// 새 문서에서 readyState 까지 확인한 뒤에야 완료로 본다.
    async fn navigate_and_wait(
        page: &Page,
        url: &str,
        timeout: std::time::Duration,
    ) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let url_literal = serde_json::to_string(url).map_err(|e| e.to_string())?;
        let token = NAV_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // 표식을 심고 현재 URL 을 한 왕복으로 읽는다. 문자열이 돌아왔다는 것 자체가
        // 표식 대입이 실행됐다는 증거다 — 표식을 못 심었다면 이동 완료를 검증할 수
        // 없으므로(옛 문서를 새 문서로 오인한다) 이동을 시작하지 않고 실패로 끝낸다.
        let current_url = Self::eval_before(
            page,
            format!("(window.__omnirecNavToken = {}, location.href)", token),
            deadline,
        )
        .await
        .map_err(|e| format!("페이지가 스크립트에 응답하지 않습니다: {}", e))?
        .and_then(|value| value.as_str().map(|s| s.to_string()))
        .ok_or_else(|| "이동 표식을 심을 수 없어 이동을 검증할 수 없습니다".to_string())?;

        // 프래그먼트만 바뀌는 이동은 문서가 새로 만들어지지 않아 표식이 남는다.
        // 이 앱의 이동 대상(에디터/로그인 URL)에는 프래그먼트가 없어 실제로는 발생하지
        // 않지만, 나중에 그런 URL 이 들어왔을 때 45초를 통째로 날리지 않도록 그 경우만
        // URL 일치로 판정한다.
        let fragment_only = Self::fragment_only_change(&current_url, url);

        // 이동을 시작한다. 네비게이션이 커밋되며 실행 컨텍스트가 파괴돼 이 호출 자체가
        // 에러로 끝날 수 있는데, 정상적인 현상이라 실패로 보지 않는다. 대신 아래 폴링이
        // 끝까지 완료되지 못했을 때 원인을 알 수 있게 메시지는 들고 있는다.
        let replace_error = Self::eval_before(
            page,
            format!("location.replace({});", url_literal),
            deadline,
        )
        .await
        .err();

        let ready_script = if fragment_only {
            format!(
                "location.href === {} && document.readyState !== 'loading'",
                url_literal
            )
        } else {
            format!(
                "window.__omnirecNavToken !== {} && document.readyState !== 'loading'",
                token
            )
        };

        let mut last_poll_error: Option<String> = None;
        loop {
            match Self::eval_before(page, ready_script.clone(), deadline).await {
                Ok(Some(serde_json::Value::Bool(true))) => return Ok(()),
                // 아직 옛 문서이거나(false) 문서 교체 중이라 컨텍스트가 잠깐 사라진
                // 상태(에러)다. 둘 다 정상적인 과도 상태이므로 데드라인까지 계속 본다.
                Ok(_) => {}
                Err(e) => last_poll_error = Some(e),
            }
            if tokio::time::Instant::now() >= deadline {
                let mut message = "페이지 로딩이 너무 오래 걸립니다(시간 초과)".to_string();
                if let Some(e) = replace_error.as_ref() {
                    message.push_str(&format!(" · 이동 시작: {}", e));
                }
                if let Some(e) = last_poll_error.as_ref() {
                    message.push_str(&format!(" · 마지막 확인: {}", e));
                }
                return Err(message);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// 무해한 CDP 왕복 하나로 이 브라우저가 실제로 명령을 받는지 확인한다.
    ///
    /// **`Handler` 를 이 자리에서 직접 굴려야 한다.** 연결을 실제로 읽는 것은 handler 이고
    /// 이 시점에는 아직 폴링 태스크가 없으므로, 여기서 돌리지 않으면 응답이 영원히 오지
    /// 않아 "검증"이 그냥 타임아웃으로 끝난다.
    async fn browser_responds(
        browser: &Browser,
        handler: &mut Handler,
        timeout: std::time::Duration,
    ) -> bool {
        let probe = browser.version();
        tokio::pin!(probe);
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                result = &mut probe => return result.is_ok(),
                event = handler.next() => {
                    // 스트림이 끝났거나 에러가 나오면 연결이 죽은 것이다.
                    if !matches!(event, Some(Ok(()))) {
                        return false;
                    }
                }
                _ = &mut deadline => return false,
            }
        }
    }

    /// Chrome 프로필 디렉터리는 한 번에 하나의 Chrome 프로세스만 열 수 있다 —
    /// 두 번째로 열려는 순간 `SingletonLock` 충돌로 `Browser::launch` 가 실패한다.
    /// 이 상황이 그냥 에러로 끝나면 사용자는 "Failed to create .../SingletonLock:
    /// File exists" 같은 원시 Chrome 에러를 그대로 보게 된다(실측 확인). 원인은 둘 중 하나다:
    ///
    /// 1. 이전에 뜬 Chrome 이 앱이 재시작/충돌하는 사이에도 **여전히 살아있다** — 이 경우
    ///    새로 띄우는 대신 그 Chrome 의 DevTools 엔드포인트(`DevToolsActivePort` 파일)에
    ///    접속해 그대로 이어 쓴다.
    /// 2. Chrome 이 비정상 종료(강제 종료 등)해 락 파일만 남았다 — 이 경우 락 파일을
    ///    지우고 한 번 더 실행을 시도한다.
    async fn launch_with_recovery(
        config: BrowserConfig,
        profile_dir: &std::path::Path,
    ) -> Result<(Browser, Handler), String> {
        match Browser::launch(config.clone()).await {
            Ok(pair) => Ok(pair),
            Err(launch_err) => {
                let message = launch_err.to_string();
                if !message.contains("SingletonLock") {
                    return Err(message);
                }

                // (1) 이미 떠 있는 Chrome 에 접속을 시도한다.
                //     `DevToolsActivePort` 는 비정상 종료 후에도 남을 수 있어, 그 포트가
                //     응답하지 않으면 접속이 끝나지 않는다 — 시간 제한을 건다.
                if let Some(port) = Self::read_devtools_port(profile_dir) {
                    let connect = Browser::connect(format!("http://127.0.0.1:{}", port));
                    if let Ok(Ok((browser, mut handler))) =
                        tokio::time::timeout(LIVENESS_TIMEOUT, connect).await
                    {
                        // **접속 성공을 그대로 반환하지 말 것.** 죽어 가는 Chrome 은
                        // 웹소켓 핸드셰이크까지는 받아 주면서 그 뒤 커맨드에는 응답하지
                        // 않는다. 그대로 넘기면 나중에 `new_page()`/바인딩 등록이 실패해
                        // "Typecast 페이지를 열 수 없습니다"로 끝나고, 락 파일 정리
                        // 경로로는 영영 못 넘어간다(사용자에게는 창이 안 뜨는 증상).
                        if Self::browser_responds(&browser, &mut handler, LIVENESS_TIMEOUT).await {
                            return Ok((browser, handler));
                        }
                        // 쓸 수 없는 연결은 여기서 버린다(연결만 놓는 것이라 CDP 왕복이
                        // 없다 — 이미 응답하지 않는 브라우저에 close() 를 걸면 멈춘다).
                        drop(handler);
                        drop(browser);
                    }
                }

                // (2) 접속도 안 되면 죽은 프로세스가 남긴 락으로 보고 정리한 뒤 재시도한다.
                //     방금 닫은 Chrome 이 아직 정리 중일 수 있어 잠깐 기다렸다 다시 띄운다.
                //     재시도는 유한하게 유지한다(무한 루프 금지).
                let mut last_error = message;
                for _ in 0..2 {
                    Self::clear_singleton_lock_files(profile_dir);
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    match Browser::launch(config.clone()).await {
                        Ok(pair) => return Ok(pair),
                        Err(e) => last_error = e.to_string(),
                    }
                }
                Err(last_error)
            }
        }
    }

    fn read_devtools_port(profile_dir: &std::path::Path) -> Option<u16> {
        let content = std::fs::read_to_string(profile_dir.join("DevToolsActivePort")).ok()?;
        content.lines().next()?.trim().parse::<u16>().ok()
    }

    fn clear_singleton_lock_files(profile_dir: &std::path::Path) {
        for name in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
            let _ = std::fs::remove_file(profile_dir.join(name));
        }
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
        // open/close/clear 전이를 직렬화한다. 두 번 연달아 눌리면(프론트엔드의 재시도,
        // 일괄 러너의 "창 열림 확인" 폴링) 두 실행이 겹쳐 뒤에 온 쪽이 앞의 세션을
        // 덮어써 Chrome 프로세스와 백그라운드 태스크 4개가 통째로 누출된다.
        // 잠금 순서는 transition → session 이며, 이 잠금은 CDP 왕복을 넘어 들고 있어도
        // 된다(handler 태스크는 session 만 잡으므로 교착이 없다).
        let _transition = state.transition.lock().await;

        // 이미 살아 있는 창이 있으면 그것을 앞으로 올리기만 한다.
        // 죽은 세션은 `live_main_page` 가 걸러 내며, 아래 실행 경로로 넘어간다.
        if let Some((page, _url)) = Self::live_main_page(app).await {
            match tokio::time::timeout(LIVENESS_TIMEOUT, page.bring_to_front()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => Self::debug(app, "focus-failed", &e.to_string()),
                Err(_) => Self::debug(app, "focus-timeout", "탭 활성화에 응답이 없습니다"),
            }
            Self::activate_chrome_app();
            return Ok(());
        }

        let chrome_path = SettingsManager::find_chrome(settings.custom_chrome_path.as_deref())?;
        let profile_dir = SettingsManager::typecast_chrome_profile_dir();

        let config = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(profile_dir.clone())
            .with_head()
            .window_size(1180, 840)
            // 합성 클릭으로도 재생되도록 자동재생 사용자 제스처 요구를 끈다.
            // (WKWebView 시절 mediaTypesRequiringUserActionForPlayback = None 과 같은 목적.)
            .arg("--autoplay-policy=no-user-gesture-required")
            .build()
            .map_err(|e| format!("Chrome 실행 설정을 만들 수 없습니다: {}", e))?;

        let (mut browser, mut handler) = Self::launch_with_recovery(config, &profile_dir)
            .await
            .map_err(|e| format!("Chrome 을 실행할 수 없습니다: {}", e))?;

        // 세션 세대를 여기서 발급해 handler 태스크에 넘긴다. handler 는 자기 세션만
        // 버려야 한다 — 예전에는 세대 없이 슬롯을 통째로 take 했고, 뒤늦게 끝난 옛
        // handler 가 방금 만든 새 세션을 지우고 그 태스크 4개를 abort 해 브라우저가
        // 열려 있는데 아무 명령도 먹지 않는 상태가 됐다.
        let session_id = SESSION_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let handler_app = app.clone();
        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
            // 이벤트 스트림이 끝났다는 것은 연결이 끊겼다는 뜻이다(사용자가 Chrome 을 직접 닫은 경우 포함).
            let _ = handler_app.emit("typecast_browser_closed", ());
            // 남은 세션 객체를 반드시 버린다. 그냥 두면 다음 `open()` 이 "이미 열려 있다"고
            // 판단해 죽은 페이지를 앞으로 올리려다 커맨드 타임아웃까지 멈추고, 정작 창은
            // 뜨지 않는다. (프론트엔드의 `typecast_browser_closed` 처리만으로는 Rust 쪽
            // 상태가 그대로 남는다.)
            Self::discard_dead_session(&handler_app, session_id).await;
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

            Self::navigate_and_wait(&page, &target, NAVIGATE_TIMEOUT)
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
                // close() 는 handler_task 가 살아 있어야 CDP 응답을 받는다 — 순서를
                // 바꾸지 말 것. 이미 죽은 브라우저면 응답이 오지 않으므로 시간 제한을 건다.
                let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
                    let _ = browser.close().await;
                    let _ = browser.wait().await;
                })
                .await;
                handler_task.abort();
                return Err(error);
            }
        };

        // 슬롯이 이미 차 있으면 **정리한 뒤** 설치한다. 예전에는 그냥 덮어써서, 그
        // 세션의 Chrome 프로세스와 백그라운드 태스크 4개가 아무도 정리하지 않는 상태로
        // 영구 누출됐다(창이 두 개 떠 있는데 앱은 새 것만 아는 증상).
        // 정리(CDP 왕복)는 반드시 상태 잠금을 놓은 뒤에 한다.
        let previous = {
            let mut guard = state.session.lock().await;
            let previous = guard.take();
            *guard = Some(CdpSession {
                id: session_id,
                browser,
                main_page: page,
                handler_task,
                binding_task,
                navigation_task,
                target_task,
            });
            previous
        };
        if let Some(previous) = previous {
            Self::debug(app, "session-replaced", "남아 있던 이전 세션을 정리합니다");
            previous.shutdown().await;
        }

        Self::activate_chrome_app();
        Ok(())
    }

    pub async fn close(app: &AppHandle) -> Result<(), String> {
        let state = Self::cdp_state(app);
        // open() 과 직렬화한다. 잠금 순서는 transition → session.
        let _transition = state.transition.lock().await;
        let session = {
            let mut guard = state.session.lock().await;
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
        // 핸들만 꺼내고 잠금을 바로 놓는다 — 아래 이동은 최대 45초짜리 CDP 왕복이다.
        let existing = {
            let guard = state.session.lock().await;
            guard.as_ref().map(|session| session.main_page.clone())
        };
        match existing {
            Some(page) => Self::navigate_and_wait(&page, &target, NAVIGATE_TIMEOUT)
                .await
                .map_err(|e| format!("페이지 이동 실패: {}", e)),
            // 세션이 없으면 창부터 띄운다. open() 이 전이 잠금을 잡으므로 여기서는
            // 잠금을 들고 있지 않아야 한다(위 블록에서 이미 놓았다).
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
    ///
    /// **상태 잠금을 CDP `await` 너머로 들고 있지 않는다.** 예전에는 `state.session` 가드를
    /// 든 채로 `clear_cookies()` / `ClearDataForOrigin` / `navigate_and_wait()` 를 모두
    /// 기다렸다. 그 사이 연결이 끊기면 세션을 정리하려는 handler 태스크가 같은 잠금을
    /// 기다려, 둘이 서로 영원히 멈춘다(Typecast 관련 커맨드 전체가 굳는다).
    /// `Browser::clear_cookies` 는 `&Browser` 가 필요하고 `Browser` 는 `Clone` 이 아니므로,
    /// 세션을 슬롯에서 통째로 꺼내 잠금을 **놓은 뒤** 왕복하고 끝나면 되돌려 놓는다.
    pub async fn clear_session(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();
        let state = Self::cdp_state(app);

        // 세션을 슬롯 밖으로 들고 있는 동안 `open()` 이 새 세션을 설치하면 둘 중 하나가
        // 누출되므로 전이 잠금으로 직렬화한다. 잠금 순서는 transition → session.
        let _transition = state.transition.lock().await;

        let session = {
            let mut guard = state.session.lock().await;
            guard.take()
        };

        let outcome = match session {
            Some(session) => {
                let result = Self::clear_open_session(&session, &settings).await;
                // 성공/실패 어느 경로에서도 세션 슬롯을 계약대로 되돌린다. 창은 여전히
                // 열려 있고, 정말 죽었다면 다음 `live_main_page()` 왕복이 세대 검증으로
                // 버린다(그때 태스크도 함께 정리된다).
                let leftover = {
                    let mut guard = state.session.lock().await;
                    if guard.is_some() {
                        // 전이 잠금 덕에 정상 경로에는 없다. 그래도 남의 세션을 덮어써
                        // 브라우저와 태스크 4개를 누출시키는 것보다 우리 것을 정리한다.
                        Some(session)
                    } else {
                        *guard = Some(session);
                        None
                    }
                };
                if let Some(leftover) = leftover {
                    Self::debug(
                        app,
                        "session-conflict",
                        "세션 초기화 중 새 세션이 설치되어 이전 세션을 정리합니다",
                    );
                    leftover.shutdown().await;
                }
                result
            }
            None => {
                // Chrome 이 실행 중이 아니므로 프로필 디렉터리를 지우는 것이 안전하다.
                let dir = SettingsManager::typecast_chrome_profile_dir();
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => Ok(()),
                    // 애초에 없으면 지울 것도 없다(한 번도 로그인하지 않은 상태).
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(format!(
                        "로그인 프로필 디렉터리를 지울 수 없습니다({}): {}",
                        dir.display(),
                        e
                    )),
                }
            }
        };

        // 플래그는 어느 경로에서든 내린다. 삭제가 중간에 실패했더라도 "저장된 세션이
        // 있다"고 계속 표시하면 사용자가 재로그인 경로를 찾지 못한다.
        let mut updated = settings;
        updated.typecast_session_saved = false;
        updated.typecast_last_login_at = None;
        SettingsManager::save(&updated)?;

        // 실패는 실패로 보고한다 — 쿠키가 남아 있는데 "초기화 완료"로 끝내면 사용자는
        // 왜 여전히 로그인된 상태인지 알 수 없다.
        outcome
    }

    /// 열려 있는 세션의 쿠키/오리진 스토리지를 지우고 로그인 페이지로 되돌린다.
    ///
    /// **호출 전에 세션을 슬롯에서 꺼내 상태 잠금을 놓아야 한다** — 아래는 전부 CDP 왕복이다.
    /// 죽은 브라우저는 응답이 오지 않으므로 왕복마다 시간 제한을 건다.
    async fn clear_open_session(
        session: &CdpSession,
        settings: &crate::types::Settings,
    ) -> Result<(), String> {
        tokio::time::timeout(LIVENESS_TIMEOUT, session.browser.clear_cookies())
            .await
            .map_err(|_| "쿠키 삭제에 응답이 없습니다(브라우저가 죽었을 수 있습니다).".to_string())?
            .map_err(|e| format!("쿠키 삭제 실패: {}", e))?;

        let origin = tauri::Url::parse(&settings.typecast_editor_url)
            .map_err(|e| {
                format!(
                    "오리진 주소를 해석할 수 없습니다({}): {}",
                    settings.typecast_editor_url, e
                )
            })?
            .origin()
            .ascii_serialization();
        tokio::time::timeout(
            LIVENESS_TIMEOUT,
            session
                .main_page
                .execute(ClearDataForOriginParams::new(origin, "all")),
        )
        .await
        .map_err(|_| "스토리지 삭제에 응답이 없습니다.".to_string())?
        .map_err(|e| format!("스토리지 삭제 실패: {}", e))?;

        let signin = Self::parse_url(&settings.typecast_signin_url)?;
        Self::navigate_and_wait(&session.main_page, &signin, NAVIGATE_TIMEOUT)
            .await
            .map_err(|e| format!("로그인 페이지로 되돌리지 못했습니다: {}", e))
    }

    pub async fn state(app: &AppHandle) -> TypecastBrowserState {
        let settings = SettingsManager::load();
        // 세션 객체의 존재가 아니라 **실제 응답**으로 판정한다. 죽은 세션을 "열려 있음"
        // 으로 보고하면 프론트엔드가 창을 다시 열지 않아 자동 녹음이 통째로 실패한다.
        let (is_open, current_url) = match Self::live_main_page(app).await {
            Some((_page, url)) => (true, url),
            None => (false, None),
        };

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

    /// 자동화 선택자를 페이지에 심는다. 비워두면 내장 휴리스틱을 쓴다.
    ///
    /// 페이지가 새로 로드되면 주입 스크립트가 다시 실행되며 기본값으로 돌아가므로,
    /// 실제 조작(`editor_ready` · `prepare_script` · `play` · `probe`) 직전에 매번 다시 심는다.
    pub async fn apply_automation_options(app: &AppHandle) -> Result<(), String> {
        let settings = SettingsManager::load();
        let page = Self::get_main_page(app).await?;
        let editor =
            serde_json::to_string(&settings.typecast_editor_selector).map_err(|e| e.to_string())?;
        let play =
            serde_json::to_string(&settings.typecast_play_selector).map_err(|e| e.to_string())?;
        page.evaluate(format!(
            "window.__omnirecSetOptions && window.__omnirecSetOptions({}, {});",
            editor, play
        ))
        .await
        .map(|_| ())
        .map_err(|e| format!("자동화 옵션 적용 실패: {}", e))
    }

    /// 사용자가 직접 연 Typecast 프로젝트에 편집기와 플레이어가 모두 준비됐는지 확인한다.
    pub async fn editor_ready(app: &AppHandle) -> Result<bool, String> {
        Self::apply_automation_options(app).await?;
        let page = Self::get_main_page(app).await?;
        page.evaluate("Boolean(window.__omnirecEditorReady && window.__omnirecEditorReady())")
            .await
            .map_err(|e| format!("프로젝트 편집기 확인 실패: {}", e))?
            .into_value::<bool>()
            .map_err(|e| format!("프로젝트 편집기 확인 결과 해석 실패: {}", e))
    }

    /// 대본을 편집기에 채우고, 결과를 `typecast_step` 이벤트로 보고한다.
    ///
    /// 자동 입력이 실패하더라도 사용자가 직접 붙여넣을 수 있도록 클립보드에도 넣어 둔다.
    /// 수동 녹음 화면의 "대본 보내기"도 같은 경로를 쓴다.
    ///
    /// `copy_to_clipboard` 는 자동 일괄 녹음을 위한 예외다. 무인 실행이라 붙여넣을 사람이
    /// 없는데, 대본마다 복사하면 사용자가 쓰던 클립보드를 대본 수만큼 덮어써 버린다.
    pub async fn prepare_script(
        app: &AppHandle,
        text: String,
        copy_to_clipboard: bool,
    ) -> Result<(), String> {
        if copy_to_clipboard {
            // 클립보드 복사는 자동 입력이 실패했을 때의 수동 붙여넣기 폴백이다.
            // 실패해도 자동 입력은 계속 시도해야 하므로 오류로 올리지 않지만(AGENTS.md),
            // 조용히 버리면 "붙여넣기가 안 되는" 원인을 아무 데서도 알 수 없다.
            if let Err(e) = crate::clipboard::copy_text(&text) {
                Self::debug(app, "clipboard-failed", &e);
            }
        }
        Self::apply_automation_options(app).await?;
        let page = Self::get_main_page(app).await?;
        let payload = serde_json::to_string(&text).map_err(|e| e.to_string())?;
        page.evaluate(format!(
            "window.__omnirecPrepare && window.__omnirecPrepare({});",
            payload
        ))
        .await
        .map_err(|e| format!("대본 주입 실패: {}", e))?;
        // 탭 활성화 실패는 대본 주입 결과를 바꾸지 않는다(주입은 위에서 이미 성공했다).
        // 다만 진단에는 남긴다 — 창이 앞으로 오지 않는 증상의 단서다.
        if let Err(e) = page.bring_to_front().await {
            Self::debug(app, "focus-failed", &e.to_string());
        }
        Ok(())
    }

    /// 편집기의 재생 버튼을 누른다.
    ///
    /// 누르기 전에 자동화 선택자를 다시 심는다. 대본 주입 이후 페이지가 다시
    /// 로드됐을 수 있기 때문이다.
    pub async fn play(app: &AppHandle) -> Result<(), String> {
        Self::apply_automation_options(app).await?;
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
        Self::apply_automation_options(app).await?;
        let page = Self::get_main_page(app).await?;
        page.evaluate("window.__omnirecProbe && window.__omnirecProbe();")
            .await
            .map(|_| ())
            .map_err(|e| format!("페이지 진단 실패: {}", e))
    }

    /// Typecast 페이지 위에 안내 토스트를 띄운다(카운트다운 / 녹음 시작 알림 용도).
    pub async fn notify(
        app: &AppHandle,
        message: String,
        tone: Option<String>,
    ) -> Result<(), String> {
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

  // 재생 버튼 활성화를 기다리는 콜백 취소용 세대 번호.
  // doPlay()/doStop() 이 모두 이 값을 올리고, 최대 5초간 도는 활성화 대기는
  // 자기 세대가 아니면 클릭도 보고도 하지 않고 즉시 끝낸다.
  var playGeneration = 0;

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
        // 텍스트 노드가 하나도 없는 완전히 빈 문단이면 el 전체 대신 편집 가능한
        // <p> 안쪽만 선택한다. el 전체를 선택하면 화자 선택 버튼(contenteditable="false")
        // 까지 선택 범위에 들어가, 그 상태로 붙여넣으면 Slate 가 문단 자체를(화자 버튼까지)
        // 지워버려 문단이 0개가 되고 이후 붙여넣기가 들어갈 자리가 없어지는 문제가 있었다.
        var editableParagraphs = el.querySelectorAll('p');
        var target = editableParagraphs.length
          ? editableParagraphs[editableParagraphs.length - 1]
          : el;
        range.selectNodeContents(target);
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

  /**
   * 편집기를 완전히 비운다.
   *
   * execCommand('delete') 는 Slate 의 컨트롤을 거치지 않고 DOM 을 직접 바꾼다. 그 결과
   * Slate 가 항상 유지해야 할 불변식("문단이 최소 1개는 있어야 한다")이 깨져 문단이
   * 통째로(화자 선택 버튼까지) 사라지는 경우가 실측으로 확인됐다 — 이 상태가 되면
   * Slate 가 이 편집기 DOM 과 완전히 어긋나 이후 어떤 execCommand 로도 복구되지 않는다.
   * 대신 진짜 Backspace keydown 이벤트를 보낸다 — Slate 는 이 키를 자기 onKeyDown 에서
   * 가로채 자신의 delete 트랜잭션으로 처리하므로 불변식이 깨지지 않는다.
   */
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
    try {
      el.dispatchEvent(new KeyboardEvent('keydown', {
        key: 'Backspace',
        code: 'Backspace',
        keyCode: 8,
        which: 8,
        bubbles: true,
        cancelable: true
      }));
    } catch (e) {}
    setTimeout(function () { clearEditor(el, attempt + 1, done); }, 150);
  }

  /**
   * execCommand('delete') 는 Slate 의 컨트롤을 거치지 않고 DOM 을 직접 바꾼다.
   * 그 결과 Slate 가 항상 유지해야 할 불변식("문단이 최소 1개는 있어야 한다")이
   * 깨져 문단이 통째로(화자 선택 버튼까지) 사라지는 경우가 실측으로 확인됐다 —
   * 이 상태에서는 붙여넣기가 들어갈 자리가 없어 입력이 조용히 사라진다.
   * execCommand('insertText') 로 진짜 문자 입력 하나를 흘려보내면 Slate 의
   * onChange/정규화 파이프라인이 정상적으로 돌아 문단을 스스로 복구한다.
   */
  function repairEmptyEditor(el, done) {
    if (el.querySelectorAll('[data-slate-node="element"]').length > 0) {
      done();
      return;
    }
    el.focus();
    try { document.execCommand('insertText', false, ' '); } catch (e) {}
    setTimeout(done, 150);
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

  // 편집기 내용과 대본을 비교하기 위한 정규화.
  //
  // Slate 가 **필연적으로** 바꾸는 것만 흡수한다. 그 밖의 차이는 전부 "대본이 제대로
  // 들어가지 않았다"는 뜻이므로 여기서 관대하게 지워 없애면 안 된다.
  //  1) 공백·개행: Slate 는 대본을 단락으로 쪼개고(readEditor 가 '\n' 으로 다시 이어붙인다),
  //     줄 끝에 렌더링용 개행을 하나 더 붙이며, 연속 공백을 NBSP(\u00A0 — \s 에 포함된다)로
  //     바꾼다.
  //  2) 제로폭 문자: 빈 리프 자리에 Slate 가 넣는 스캐폴딩(\uFEFF 등)이며 대본 글자가 아니다.
  //  3) 한글 조합 형태: DOM 왕복에서 조합형/완성형이 뒤바뀌어도 같은 글자다(NFC 로 통일).
  function normalize(value) {
    var text = String(value === null || value === undefined ? '' : value)
      .replace(/[\u200B-\u200D\uFEFF]/g, '')
      .replace(/\s+/g, '');
    try { return text.normalize('NFC'); } catch (e) { return text; }
  }

  // 두 문자열이 어디서부터 갈라졌는지(문자 인덱스).
  // 앞부분이 같고 길이만 다르면 짧은 쪽의 길이를 돌려준다.
  function firstDifference(a, b) {
    var len = Math.min(a.length, b.length);
    for (var i = 0; i < len; i++) {
      if (a.charCodeAt(i) !== b.charCodeAt(i)) return i;
    }
    return len;
  }

  // 불일치 지점 주변만 잘라 보여준다(진단 로그가 대본 전체로 넘치지 않게).
  function excerpt(value, at) {
    return String(value).slice(Math.max(0, at - 8), at + 12);
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
    if (!editor) {
      report('step:prepare-failed:작업할 Typecast 프로젝트를 직접 열어 둔 뒤 다시 시작하세요');
      return true;
    }

    // 빈 줄을 걷어내 빈 단락이 생기지 않게 한다.
    var text = cleanScript(rawText);
    editor.focus();

    // 이전 대본을 먼저 완전히 비운다. 비우지 않고 붙여넣으면 Slate 가
    // 커서가 놓인 문단만 교체해 이전 대본의 나머지 문단이 남는다.
    clearEditor(editor, 0, function (cleared) {
      repairEmptyEditor(editor, function () {
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
          var paragraphs = editor.querySelectorAll('[data-slate-node="element"]').length;
          // 처음부터 낭독되도록 선택 해제 + 캐럿을 맨 앞으로 옮긴다.
          var caret = collapseCaretToStart(editor);

          var details =
            ' · ' + describe(editor) +
            ' · ' + method +
            (cleared ? '' : ' · 비우기실패') +
            (caret ? ' · 캐럿맨앞' : ' · 캐럿이동실패');

          // **완전 일치**로 검증한다.
          //
          // 예전에는 "앞 40자가 어딘가 들어 있다" + "기대보다 20자 넘게 길지 않다"만 봤다.
          // 하한이 없어서 3,000자 대본에서 앞 50자만 들어가도 step:prepared 가 나갔고,
          // 앞부분만 낭독한 파일이 정상 완료로 저장됐다. 관대한 오차 허용으로 되돌리지
          // 말 것 — 편집기가 필연적으로 바꾸는 부분은 normalize() 안에서만 흡수한다.
          if (actual !== expected) {
            var at = firstDifference(actual, expected);
            var reason = (actual.length < expected.length && at === actual.length)
              ? '대본 앞부분만 들어갔습니다'
              : (actual.length > expected.length && at === expected.length)
                ? '이전 대본이 뒤에 남아 있습니다'
                : '내용이 어긋납니다';
            report(
              'step:prepare-failed:입력 확인 실패 — ' + reason +
              ' (기대 ' + expected.length + '자 / 실제 ' + actual.length + '자' +
              ' / 첫 불일치 ' + at + '번째' +
              ' / 기대 [' + excerpt(expected, at) + '] 실제 [' + excerpt(actual, at) + '])' +
              details
            );
            return;
          }

          report(
            'step:prepared:' + actual.length + '자' +
            (paragraphs ? ' · ' + paragraphs + '단락' : '') + details
          );
        }, 500);
      });
    });

    return true;
  }

  function doPlay() {
    intentionalStop = false;
    // 이 호출 이전에 예약된 클릭/대기 콜백을 전부 무효화한다(중복 트리거 방지).
    var generation = ++playGeneration;
    var button = findPlayButton();
    if (!button) {
      report('step:play-failed:작업할 Typecast 프로젝트의 재생 버튼을 찾지 못했습니다');
      return true;
    }
    var source = playSource;
    // 입력 후 재생까지 사이에 커서가 움직였을 수 있으므로 직전에 다시 맨 앞으로 보낸다.
    var caret = collapseCaretToStart(findEditor());

    // 붙여넣기 직후에는 재생 버튼이 잠시 비활성일 수 있어 활성화될 때까지 기다린다.
    var waits = 0;
    (function waitUntilEnabled() {
      // 기다리는 동안 정지 요청이 들어왔으면 클릭하지 않는다.
      if (generation !== playGeneration) return;
      waits += 1;
      if (isDisabled(button)) {
        if (waits < 25) {
          setTimeout(waitUntilEnabled, 200);
          return;
        }
        report('step:play-failed:재생 버튼이 계속 비활성 상태입니다 ' + describe(button));
        return;
      }
      clickPlayOnce(button, source, caret);
    })();
    return true;
  }

  /**
   * 재생 버튼을 정확히 한 번 누르고, 클릭 전달 여부를 진단에 남긴다.
   *
   * 반복 합성 클릭은 Typecast 가 아직 합성을 준비하는 동안 재생/정지 토글을
   * 연속으로 바꿔 화면만 재생 상태이고 소리는 나지 않는 경합을 만들 수 있다.
   * 편집기 안정화는 호출자가 녹음 시작 전에 기다리고, 여기서는 단일 클릭만 보낸다.
   */
  function clickPlayOnce(button, source, caret) {
    var target = closestButton(button);
    var delivered = 0;
    var probeListener = function () { delivered += 1; };
    target.addEventListener('click', probeListener, true);
    clickLikeUser(button);
    try { target.removeEventListener('click', probeListener, true); } catch (e) {}

    report(
      'step:playing:' + describe(button) +
      (source ? ' · ' + source : '') +
      (caret ? ' · 캐럿맨앞' : '') +
      ' · 클릭 1회(전달 ' + delivered + ')'
    );
  }

  function doStop() {
    intentionalStop = true;
    // 활성화 대기 중이면 아래 정지 처리 뒤 재생 버튼을 누르지 못하게 취소한다.
    playGeneration += 1;
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
      'play=' + describe(findPlayButton()) + (playSource ? '(' + playSource + ')' : ''),
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
    window.__omnirecSetOptions = function (editor, play) {
      selectors.editor = editor || '';
      selectors.play = play || '';
      dispatchRequest({
        type: 'options',
        editor: editor,
        play: play
      });
    };
    window.__omnirecEditorReady = function () {
      return !!findEditor() && !!findPlayButton();
    };


    window.__omnirecProbe = function () { dispatchRequest({ type: 'probe' }); };

    window.__omnirecPrepare = function (text) {
      window.__omnirecHandled.prepare = false;
      dispatchRequest({ type: 'prepare', text: text });
      // 어떤 프레임도 편집기를 찾지 못하면 실패로 보고한다.
      setTimeout(function () {
        if (!window.__omnirecHandled.prepare) {
          report('step:prepare-failed:작업할 Typecast 프로젝트를 직접 열어 둔 뒤 다시 시작하세요');
          toast('Typecast에서 작업할 프로젝트를 직접 열어 주세요.', 'warn');
        }
      }, 1200);
    };

    window.__omnirecPlay = function () {
      window.__omnirecHandled.play = false;
      dispatchRequest({ type: 'play' });
      setTimeout(function () {
        if (!window.__omnirecHandled.play) {
          report('step:play-failed:작업할 Typecast 프로젝트의 재생 버튼을 찾지 못했습니다');
        }
      }, 1200);
    };

    window.__omnirecStopPlayback = function () { dispatchRequest({ type: 'stop' }); };
  } else {
    // 서브프레임은 상위에서 온 selectors 요청도 처리해야 한다.
    var baseHandle = handleRequest;
    handleRequest = function (request) {
      if (request && request.type === 'options') {
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
    use super::{TypecastController, BRIDGE_BINDING_NAME, MAIN_INIT_SCRIPT};

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
    fn devtools_port_is_read_only_when_the_file_is_usable() {
        let dir = std::env::temp_dir().join(format!(
            "omnirec-devtools-port-test-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // 파일이 아예 없는 경우(정상 종료 후)
        assert_eq!(TypecastController::read_devtools_port(&dir), None);

        // Chrome 이 남기는 형식: 첫 줄이 포트, 둘째 줄이 브라우저 타깃 경로
        std::fs::write(
            dir.join("DevToolsActivePort"),
            "51234\n/devtools/browser/abc\n",
        )
        .unwrap();
        assert_eq!(TypecastController::read_devtools_port(&dir), Some(51234));

        // 깨진 내용이면 포트로 쓰지 않는다(엉뚱한 곳에 접속을 시도하면 안 된다).
        std::fs::write(dir.join("DevToolsActivePort"), "").unwrap();
        assert_eq!(TypecastController::read_devtools_port(&dir), None);
        std::fs::write(dir.join("DevToolsActivePort"), "not-a-port\n").unwrap();
        assert_eq!(TypecastController::read_devtools_port(&dir), None);

        // 락 파일 정리는 없는 파일에도 조용히 성공해야 한다.
        TypecastController::clear_singleton_lock_files(&dir);
        std::fs::write(dir.join("SingletonLock"), "x").unwrap();
        TypecastController::clear_singleton_lock_files(&dir);
        assert!(!dir.join("SingletonLock").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 이동 완료 판정의 기준선. 프래그먼트만 다른 이동은 문서가 새로 만들어지지 않으므로
    /// `navigate_and_wait` 가 "표식이 사라졌는가"로 판정할 수 없고, 그 밖의 모든 이동은
    /// 반드시 표식 경로로 판정해야 한다(옛 문서의 `readyState: complete` 를 완료로
    /// 오인하는 것이 이 헬퍼가 막는 사고다).
    #[test]
    fn only_fragment_navigations_skip_the_document_marker() {
        // 프래그먼트만 다르다 — 같은 문서에서 처리된다.
        assert!(TypecastController::fragment_only_change(
            "https://studio.typecast.ai/text-to-speech",
            "https://studio.typecast.ai/text-to-speech#editor"
        ));

        // 완전히 같은 URL 은 다시 로드된다(새 문서) — 표식으로 판정해야 한다.
        assert!(!TypecastController::fragment_only_change(
            "https://studio.typecast.ai/sign-in",
            "https://studio.typecast.ai/sign-in"
        ));
        // 경로가 다르면 당연히 새 문서다.
        assert!(!TypecastController::fragment_only_change(
            "https://studio.typecast.ai/text-to-speech",
            "https://studio.typecast.ai/sign-in#a"
        ));
        // 첫 이동은 about:blank 에서 출발한다.
        assert!(!TypecastController::fragment_only_change(
            "about:blank",
            "https://studio.typecast.ai/text-to-speech"
        ));
        // 해석할 수 없는 주소는 엄격한(표식) 경로로 보낸다.
        assert!(!TypecastController::fragment_only_change(
            "",
            "https://studio.typecast.ai/text-to-speech#editor"
        ));
    }

    /// 사용자가 보고한 증상("한 번 녹음한 뒤 브라우저를 닫고 다시 시작하면 잘 안 뜬다")의
    /// 백엔드 절반 — 같은 프로필 디렉터리로 띄우고, 완전히 종료한 직후 곧바로 다시 띄울 수
    /// 있는지 확인한다. 종료가 남긴 `SingletonLock` 잔재 때문에 두 번째 실행이 실패하면
    /// 여기서 잡힌다.
    ///
    /// 수동 실행: `cargo test --manifest-path src-tauri/Cargo.toml --lib tts::tests::relaunches_immediately_after_a_clean_shutdown -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn relaunches_immediately_after_a_clean_shutdown() {
        use crate::settings::SettingsManager;
        use chromiumoxide::browser::BrowserConfig;
        use futures::StreamExt;

        let chrome_path =
            SettingsManager::find_chrome(None).expect("Chrome executable should be discoverable");
        let profile_dir =
            std::env::temp_dir().join(format!("omnirec-relaunch-test-{}", std::process::id()));

        let build_config = || {
            BrowserConfig::builder()
                .chrome_executable(&chrome_path)
                .user_data_dir(&profile_dir)
                .with_head()
                .window_size(800, 600)
                .build()
                .expect("browser config should build")
        };

        for round in 1..=2 {
            let (mut browser, mut handler) =
                TypecastController::launch_with_recovery(build_config(), &profile_dir)
                    .await
                    .unwrap_or_else(|e| panic!("round {round}: Chrome should launch: {e}"));
            let handler_task = tokio::spawn(async move {
                while let Some(event) = handler.next().await {
                    if event.is_err() {
                        break;
                    }
                }
            });

            let page = browser
                .new_page("about:blank")
                .await
                .unwrap_or_else(|e| panic!("round {round}: page should open: {e}"));
            let value: i32 = page
                .evaluate("1 + 1")
                .await
                .unwrap_or_else(|e| panic!("round {round}: evaluate should succeed: {e}"))
                .into_value()
                .expect("should be a number");
            assert_eq!(value, 2, "round {round}");
            println!("round {round}: ok");

            let _ = browser.close().await;
            let _ = browser.wait().await;
            handler_task.abort();
        }

        let _ = std::fs::remove_dir_all(&profile_dir);
    }

    /// 실제 Chrome + CDP 왕복이 이 머신에서 동작하는지 확인하는 수동 스모크 테스트.
    /// 일반 `cargo test` 에는 포함하지 않는다(실제 Chrome 프로세스를 띄우고 로그인 없이도
    /// 접근 가능한 studio.typecast.ai 초기 페이지로 이동한다 — CI/헤드리스 환경에서 불안정할
    /// 수 있다). 수동으로 확인하려면: `cargo test --lib tts::tests::real_chrome_cdp_round_trip -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn real_chrome_cdp_round_trip() {
        use crate::settings::SettingsManager;
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
        use futures::StreamExt;

        let chrome_path =
            SettingsManager::find_chrome(None).expect("Chrome executable should be discoverable");
        println!("using chrome executable: {}", chrome_path.display());

        let profile_dir =
            std::env::temp_dir().join(format!("omnirec-cdp-smoke-test-{}", std::process::id()));

        let config = BrowserConfig::builder()
            .chrome_executable(&chrome_path)
            .user_data_dir(&profile_dir)
            .with_head()
            .window_size(1024, 768)
            .build()
            .expect("browser config should build");

        let (mut browser, mut handler) =
            Browser::launch(config).await.expect("Chrome should launch");

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        let page = browser
            .new_page("about:blank")
            .await
            .expect("should create a page");

        // 실제 프로덕션 코드(open())와 똑같은 순서: 바인딩 등록 → 초기화 스크립트 등록 →
        // 그 다음에야 실제 URL로 이동한다.
        page.execute(AddBindingParams::new(BRIDGE_BINDING_NAME))
            .await
            .expect("binding should register");
        page.evaluate_on_new_document(MAIN_INIT_SCRIPT)
            .await
            .expect("init script should register");

        let mut binding_events = page
            .event_listener::<EventBindingCalled>()
            .await
            .expect("should subscribe to binding events");

        TypecastController::navigate_and_wait(
            &page,
            "https://studio.typecast.ai/sign-in",
            std::time::Duration::from_secs(45),
        )
        .await
        .expect("navigation should complete");

        let url = page.url().await.ok().flatten();
        println!("landed on: {:?}", url);
        assert!(url.is_some(), "page should report a URL after navigation");

        // 실제 자동화 진입점(__omnirecProbe)을 호출해 MAIN_INIT_SCRIPT 가 실제로 이 페이지에
        // 주입·실행됐는지, 그리고 report() → __omnirecBridge 브리지가 실제로 동작하는지 확인한다.
        page.evaluate("window.__omnirecProbe && window.__omnirecProbe();")
            .await
            .expect("evaluate should succeed");

        let received =
            tokio::time::timeout(std::time::Duration::from_secs(5), binding_events.next())
                .await
                .expect("should receive a probe step within 5s")
                .expect("binding stream should not end");
        assert_eq!(received.name, BRIDGE_BINDING_NAME);
        assert!(
            received.payload.starts_with("step:probe:"),
            "expected a step:probe: message, got: {}",
            received.payload
        );
        println!("real automation probe payload: {}", received.payload);

        let editor_ready: bool = page
            .evaluate("Boolean(window.__omnirecEditorReady && window.__omnirecEditorReady())")
            .await
            .expect("editor readiness evaluate should succeed")
            .into_value()
            .expect("editor readiness should be boolean");
        assert!(
            !editor_ready,
            "a page without a project editor must not pass batch preflight"
        );

        let project_ready: bool = page
            .evaluate(
                r#"(() => {
                  const editor = document.createElement('div');
                  editor.setAttribute('data-slate-editor', 'true');
                  editor.setAttribute('contenteditable', 'true');
                  editor.style.cssText = 'width:400px;height:200px';
                  document.body.appendChild(editor);
                  const play = document.createElement('button');
                  play.setAttribute('aria-label', 'Play');
                  play.style.cssText = 'width:40px;height:40px';
                  document.body.appendChild(play);
                  return Boolean(window.__omnirecEditorReady && window.__omnirecEditorReady());
                })()"#,
            )
            .await
            .expect("project editor readiness evaluate should succeed")
            .into_value()
            .expect("project editor readiness should be boolean");
        assert!(
            project_ready,
            "a visible Slate editor and play button must pass batch preflight"
        );

        let _ = browser.close().await;
        let _ = browser.wait().await;
        handler_task.abort();
        let _ = std::fs::remove_dir_all(&profile_dir);
    }

    /// 실제 로그인까지 포함한 수동 종단 테스트. 사용자가 뜬 Chrome 창에서 직접 로그인하는
    /// 동안 URL을 폴링하다가, 로그인이 확인되면 실제 대본 입력 → 재생까지 실행한다.
    ///
    /// **실제 프로덕션 프로필**(`SettingsManager::typecast_chrome_profile_dir`)을 그대로
    /// 쓴다 — 여기서 로그인하면 앱을 실행했을 때도 같은 세션으로 이어진다.
    ///
    /// 수동 실행: `cargo test --manifest-path src-tauri/Cargo.toml --lib tts::tests::real_login_and_prepare_flow -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn real_login_and_prepare_flow() {
        use crate::settings::SettingsManager;
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
        use futures::StreamExt;

        let chrome_path =
            SettingsManager::find_chrome(None).expect("Chrome executable should be discoverable");
        let profile_dir = SettingsManager::typecast_chrome_profile_dir();
        println!("using chrome executable: {}", chrome_path.display());
        println!("using profile dir: {}", profile_dir.display());

        let config = BrowserConfig::builder()
            .chrome_executable(&chrome_path)
            .user_data_dir(&profile_dir)
            .with_head()
            .window_size(1180, 840)
            .arg("--autoplay-policy=no-user-gesture-required")
            .build()
            .expect("browser config should build");

        let (mut browser, mut handler) =
            Browser::launch(config).await.expect("Chrome should launch");

        let handler_task = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        let page = browser
            .new_page("about:blank")
            .await
            .expect("should create a page");

        page.execute(AddBindingParams::new(BRIDGE_BINDING_NAME))
            .await
            .expect("binding should register");
        page.evaluate_on_new_document(MAIN_INIT_SCRIPT)
            .await
            .expect("init script should register");

        let mut binding_events = page
            .event_listener::<EventBindingCalled>()
            .await
            .expect("should subscribe to binding events");

        TypecastController::navigate_and_wait(
            &page,
            "https://studio.typecast.ai/text-to-speech",
            std::time::Duration::from_secs(45),
        )
        .await
        .expect("navigation should complete");

        println!("======================================================");
        println!("Chrome 창에서 로그인을 완료한 뒤 이 터미널에서 Enter 를 누르세요.");
        println!("(URL 만으로는 로그인 여부를 정확히 알 수 없어 직접 확인을 기다립니다.)");
        println!("최대 10분 기다립니다.");
        println!("======================================================");

        let wait_for_enter = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        });

        let signed_in = tokio::select! {
            _ = wait_for_enter => {
                println!("입력을 받았습니다. 대본 입력을 시도합니다.");
                true
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(600)) => {
                println!("10분 안에 확인을 받지 못했습니다. 대본 입력/재생 테스트는 건너뜁니다.");
                false
            }
        };

        let url = page.url().await.ok().flatten().unwrap_or_default();
        println!("현재 URL: {}", url);

        if signed_in {
            println!("앱이 완전히 렌더링될 시간을 5초 더 준다...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            println!("진단(probe)을 먼저 실행합니다.");
            page.evaluate("window.__omnirecProbe && window.__omnirecProbe();")
                .await
                .expect("probe evaluate should succeed");
            for _ in 0..4 {
                match tokio::time::timeout(std::time::Duration::from_secs(2), binding_events.next())
                    .await
                {
                    Ok(Some(event)) => println!("probe: {}", event.payload),
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
            let sample_text = "안녕하세요. 이것은 OmniRec 자동화 테스트 대본입니다.";
            let payload = serde_json::to_string(sample_text).expect("json encode should succeed");
            page.evaluate(format!(
                "window.__omnirecPrepare && window.__omnirecPrepare({});",
                payload
            ))
            .await
            .expect("prepare evaluate should succeed");

            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    println!("prepare: 시간 안에 최종 결과가 오지 않았습니다");
                    break;
                }
                match tokio::time::timeout(remaining, binding_events.next()).await {
                    Ok(Some(event)) => {
                        println!("prepare 진행: {}", event.payload);
                        if event.payload.starts_with("step:prepared")
                            || event.payload.starts_with("step:prepare-failed")
                        {
                            break;
                        }
                    }
                    Ok(None) => {
                        println!("prepare: 브리지 스트림이 끊겼습니다");
                        break;
                    }
                    Err(_) => {
                        println!("prepare: 시간 안에 최종 결과가 오지 않았습니다");
                        break;
                    }
                }
            }

            // 프로덕션 배치 러너와 같이 Slate 내부 상태와 합성 준비가 반영될 시간을 준다.
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            println!("재생을 시도합니다.");
            page.evaluate("window.__omnirecPlay && window.__omnirecPlay();")
                .await
                .expect("play evaluate should succeed");

            let play_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                let remaining =
                    play_deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    println!("play: 시간 안에 최종 결과가 오지 않았습니다");
                    break;
                }
                match tokio::time::timeout(remaining, binding_events.next()).await {
                    Ok(Some(event)) => {
                        println!("play 진행: {}", event.payload);
                        if event.payload.starts_with("step:playing")
                            || event.payload.starts_with("step:play-failed")
                        {
                            break;
                        }
                    }
                    Ok(None) => {
                        println!("play: 브리지 스트림이 끊겼습니다");
                        break;
                    }
                    Err(_) => {
                        println!("play: 시간 안에 최종 결과가 오지 않았습니다");
                        break;
                    }
                }
            }

            // 낭독을 몇 초 들어볼 시간을 준다.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            page.evaluate("window.__omnirecStopPlayback && window.__omnirecStopPlayback();")
                .await
                .ok();
        }

        println!("테스트 종료, 브라우저를 정리합니다.");
        let _ = browser.close().await;
        let _ = browser.wait().await;
        handler_task.abort();
    }

    /// `Browser::launch` 를 같은 프로필로 두 번 하면 실측대로 `SingletonLock` 충돌이
    /// 난다. `launch_with_recovery` 가 이미 떠 있는 그 Chrome 에 재접속해 복구하는지
    /// 확인한다(사용자가 실제로 겪은 "Failed to create .../SingletonLock: File exists"
    /// 에러 재현 + 복구).
    #[tokio::test]
    #[ignore]
    async fn recovers_from_live_singleton_lock() {
        use crate::settings::SettingsManager;
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use futures::StreamExt;

        let chrome_path =
            SettingsManager::find_chrome(None).expect("Chrome executable should be discoverable");
        let profile_dir =
            std::env::temp_dir().join(format!("omnirec-lock-recovery-test-{}", std::process::id()));

        let build_config = || {
            BrowserConfig::builder()
                .chrome_executable(&chrome_path)
                .user_data_dir(&profile_dir)
                .with_head()
                .window_size(800, 600)
                .build()
                .expect("browser config should build")
        };

        // 1. "이미 떠 있는" Chrome 을 흉내 낸다 — 그냥 직접 띄우고 닫지 않는다.
        let (first_browser, mut first_handler) = Browser::launch(build_config())
            .await
            .expect("first Chrome should launch");
        let first_handler_task = tokio::spawn(async move {
            while let Some(event) = first_handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        // 2. 같은 프로필로 다시 열려고 하면 SingletonLock 충돌이 난다 — 복구 로직이
        //    새로 띄우는 대신 위 인스턴스에 재접속해야 한다.
        let (mut second_browser, mut second_handler) =
            TypecastController::launch_with_recovery(build_config(), &profile_dir)
                .await
                .expect("launch_with_recovery should recover by reconnecting");

        let second_handler_task = tokio::spawn(async move {
            while let Some(event) = second_handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        // 재접속이 실제로 됐는지 새 페이지를 만들어 확인한다.
        let page = second_browser
            .new_page("about:blank")
            .await
            .expect("reconnected browser should be usable");
        let value: i32 = page
            .evaluate("1 + 1")
            .await
            .expect("evaluate should succeed")
            .into_value()
            .expect("should be a number");
        assert_eq!(value, 2);
        println!("재접속 후 evaluate 결과: {}", value);

        // second_browser (재접속) 를 통해 닫으면 실제 Chrome 프로세스 자체가 종료된다.
        // handler_task 는 close() 가 CDP 응답을 받아야 하므로 반드시 close()/wait() 다음에 abort 한다.
        let _ = second_browser.close().await;
        let _ = second_browser.wait().await;
        second_handler_task.abort();

        // first_browser 가 실제로 띄웠던 프로세스는 위에서 이미 종료됐다. 여기서 또
        // close()/wait() 를 부르면 이미 죽은 프로세스의 응답을 영원히 기다리게 된다 —
        // drop 만으로 충분하다(Browser::drop 이 이미 종료됐음을 감지해 조용히 넘어간다).
        first_handler_task.abort();
        drop(first_browser);
        let _ = std::fs::remove_dir_all(&profile_dir);
    }
}
