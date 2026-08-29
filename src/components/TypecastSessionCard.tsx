import React, { useCallback, useEffect, useState } from 'react';
import {
  ArrowLeft,
  Check,
  ChevronDown,
  Globe,
  Info,
  KeyRound,
  LogIn,
  LogOut,
  RefreshCw,
  ShieldCheck,
  X,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  Settings,
  TypecastBrowserState,
  TypecastDebugPayload,
  TypecastNavigationPayload,
  TypecastPopupPayload,
} from '../types';

interface TypecastSessionCardProps {
  settings: Settings;
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  /** 안내 메시지를 부모 화면의 피드백 영역으로 올린다. */
  onNotice?: (message: string) => void;
  onError?: (message: string) => void;
}

/**
 * Typecast 브라우저 창의 로그인 · 세션 · 탐색을 담당하는 공용 카드.
 *
 * 자동 일괄 녹음과 수동 녹음 양쪽에서 쓴다. 예전에는 수동 녹음 화면에만 있어서
 * 자동 녹음만 쓰는 사용자는 로그인 UI 자체를 찾을 수 없었다.
 */
export const TypecastSessionCard: React.FC<TypecastSessionCardProps> = ({
  settings,
  onUpdateSettings,
  onNotice,
  onError,
}) => {
  const [browserState, setBrowserState] = useState<TypecastBrowserState>({
    is_open: false,
    current_url: null,
    looks_signed_in: settings.typecast_session_saved,
    account_email: settings.typecast_account_email ?? null,
    last_login_at: settings.typecast_last_login_at ?? null,
  });
  const [emailInput, setEmailInput] = useState(settings.typecast_account_email ?? '');
  const [expanded, setExpanded] = useState(false);

  const refreshBrowserState = useCallback(async () => {
    try {
      setBrowserState(await invoke<TypecastBrowserState>('get_typecast_browser_state'));
    } catch (err) {
      console.error('Typecast 상태 조회 실패:', err);
    }
  }, []);

  useEffect(() => {
    refreshBrowserState();

    const unlistenNav = listen<TypecastNavigationPayload>('typecast_navigation', (event) => {
      setBrowserState((prev) => ({
        ...prev,
        is_open: true,
        current_url: event.payload.url,
        looks_signed_in: event.payload.looks_signed_in,
      }));
    });

    const unlistenClosed = listen('typecast_browser_closed', () => {
      setBrowserState((prev) => ({ ...prev, is_open: false, current_url: null }));
    });

    const unlistenPopup = listen<TypecastPopupPayload>('typecast_popup_intercepted', () => {
      onNotice?.('차단된 소셜 로그인 팝업을 OmniRec 로그인 창으로 대신 열었습니다.');
    });

    return () => {
      unlistenNav.then((u) => u());
      unlistenClosed.then((u) => u());
      unlistenPopup.then((u) => u());
    };
  }, [refreshBrowserState, onNotice]);

  useEffect(() => {
    setEmailInput(settings.typecast_account_email ?? '');
  }, [settings.typecast_account_email]);

  const openBrowser = async (url: string) => {
    try {
      await invoke('open_typecast_browser', { url });
      await refreshBrowserState();
    } catch (err) {
      onError?.(`Typecast 창을 열 수 없습니다: ${err}`);
    }
  };

  const runCommand = async (command: string, failure: string) => {
    try {
      await invoke(command);
    } catch (err) {
      onError?.(`${failure}: ${err}`);
    }
  };

  const markLoggedIn = async () => {
    try {
      const updated = await invoke<Settings>('mark_typecast_login', {
        email: emailInput.trim() || null,
      });
      await onUpdateSettings({
        typecast_session_saved: updated.typecast_session_saved,
        typecast_last_login_at: updated.typecast_last_login_at,
        typecast_account_email: updated.typecast_account_email,
      });
      await refreshBrowserState();
      onNotice?.('로그인 세션을 저장했습니다. 다음부터 이 계정으로 바로 접속합니다.');
    } catch (err) {
      onError?.(`로그인 상태 저장 실패: ${err}`);
    }
  };

  const clearSession = async () => {
    if (
      !window.confirm(
        '저장된 Typecast 로그인 세션(쿠키 · 로컬 저장소)을 모두 지웁니다. 다시 로그인해야 합니다. 계속할까요?',
      )
    )
      return;
    try {
      await invoke('clear_typecast_session');
      await onUpdateSettings({ typecast_session_saved: false, typecast_last_login_at: null });
      await refreshBrowserState();
      onNotice?.('로그인 세션을 초기화했습니다.');
    } catch (err) {
      onError?.(`세션 초기화 실패: ${err}`);
    }
  };

  const signedIn =
    browserState.looks_signed_in && (browserState.is_open || settings.typecast_session_saved);

  return (
    <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 shadow-lg space-y-3">
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <span className="text-sm font-bold text-slate-200 flex items-center gap-2">
          <Globe className="w-4 h-4 text-blue-400" />
          Typecast 로그인 & 접속
        </span>
        <div className="flex items-center gap-2">
          <div
            className={`flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-[11px] font-bold border ${
              signedIn
                ? 'bg-emerald-950/50 border-emerald-800/60 text-emerald-400'
                : 'bg-amber-950/50 border-amber-800/60 text-amber-400'
            }`}
          >
            {signedIn ? <ShieldCheck className="w-3.5 h-3.5" /> : <KeyRound className="w-3.5 h-3.5" />}
            <span>{signedIn ? '로그인 세션 저장됨' : '로그인 필요'}</span>
          </div>
          <button
            onClick={() => setExpanded((v) => !v)}
            className="text-slate-400 hover:text-slate-200 transition"
            title="세부 설정"
          >
            <ChevronDown className={`w-4 h-4 transition-transform ${expanded ? 'rotate-180' : ''}`} />
          </button>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button
          onClick={() => openBrowser(settings.typecast_editor_url)}
          className="flex items-center gap-1.5 px-4 py-2.5 rounded-xl bg-blue-600 hover:bg-blue-500 text-white text-xs font-bold shadow-lg shadow-blue-600/25 transition active:scale-95"
        >
          <Globe className="w-4 h-4" />
          <span>Typecast 열기</span>
        </button>
        <button
          onClick={() => openBrowser(settings.typecast_signin_url)}
          className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
        >
          <LogIn className="w-3.5 h-3.5 text-blue-400" />
          <span>로그인 페이지</span>
        </button>
        <button
          onClick={markLoggedIn}
          className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-emerald-700 hover:bg-emerald-600 text-white text-xs font-semibold transition"
        >
          <Check className="w-3.5 h-3.5" />
          <span>로그인 완료 저장</span>
        </button>

        {browserState.is_open && (
          <>
            <button
              onClick={() => runCommand('typecast_go_back', '뒤로 가기 실패')}
              title="Typecast 창 뒤로 가기"
              className="flex items-center gap-1.5 px-3 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-semibold border border-slate-700 transition"
            >
              <ArrowLeft className="w-3.5 h-3.5" />
              <span>뒤로</span>
            </button>
            <button
              onClick={() => runCommand('typecast_reload', '새로고침 실패')}
              title="Typecast 창 새로고침"
              className="flex items-center gap-1.5 px-3 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-semibold border border-slate-700 transition"
            >
              <RefreshCw className="w-3.5 h-3.5" />
              <span>새로고침</span>
            </button>
            <button
              onClick={() => runCommand('close_typecast_browser', '창 닫기 실패')}
              className="flex items-center gap-1.5 px-3 py-2.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-semibold border border-slate-700 transition"
            >
              <X className="w-3.5 h-3.5" />
              <span>창 닫기</span>
            </button>
          </>
        )}

        <button
          onClick={clearSession}
          className="flex items-center gap-1.5 px-3.5 py-2.5 rounded-xl bg-red-950/50 hover:bg-red-900/60 text-red-300 text-xs font-semibold border border-red-900/50 transition ml-auto"
        >
          <LogOut className="w-3.5 h-3.5" />
          <span>세션 초기화</span>
        </button>
      </div>

      <div className="flex items-center gap-2 text-[10px] text-slate-500 font-mono">
        <span className={browserState.is_open ? 'text-emerald-500' : 'text-slate-600'}>
          ● {browserState.is_open ? '창 열림' : '창 닫힘'}
        </span>
        {browserState.current_url && (
          <>
            <span>·</span>
            <span className="truncate">{browserState.current_url}</span>
          </>
        )}
        {settings.typecast_last_login_at && (
          <>
            <span>·</span>
            <span>마지막 로그인 {settings.typecast_last_login_at}</span>
          </>
        )}
      </div>

      {expanded && (
        <div className="space-y-3 pt-1 border-t border-slate-800">
          <div className="rounded-xl bg-blue-950/25 border border-blue-900/40 p-3 flex gap-2.5 mt-3">
            <Info className="w-4 h-4 text-blue-400 shrink-0 mt-0.5" />
            <p className="text-[11px] text-slate-300 leading-relaxed">
              창에서 한 번만 로그인하면 <b className="text-blue-300">쿠키 세션이 앱에 영구 저장</b>되어
              다음 실행부터 자동으로 같은 계정으로 접속합니다. 구글 · 애플 소셜 로그인 팝업이 앱 내
              웹뷰에서 차단되면 OmniRec이 그 주소를 받아 전용 로그인 창을 열고 인증 결과를 원래 창으로
              되돌려 줍니다.
              <br />
              <span className="text-slate-400">
                OmniRec은 비밀번호를 저장하지 않으며, 아래 이메일은 어떤 계정인지 알아보기 위한 표시용
                메모입니다.
              </span>
            </p>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div>
              <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
                계정 이메일 (표시용, 선택)
              </label>
              <input
                value={emailInput}
                onChange={(e) => setEmailInput(e.target.value)}
                onBlur={() => {
                  const next = emailInput.trim();
                  if (next !== (settings.typecast_account_email ?? '')) {
                    onUpdateSettings({ typecast_account_email: next || null });
                  }
                }}
                placeholder="you@example.com"
                className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 placeholder:text-slate-600 focus:outline-none focus:border-blue-600"
              />
            </div>
            <div>
              <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
                Typecast 편집기 주소
              </label>
              <input
                value={settings.typecast_editor_url}
                onChange={(e) => onUpdateSettings({ typecast_editor_url: e.target.value })}
                placeholder="https://studio.typecast.ai/text-to-speech"
                className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 font-mono placeholder:text-slate-600 focus:outline-none focus:border-blue-600"
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

interface TypecastDiagnosticsLogProps {
  onCopy?: (text: string) => void;
}

/**
 * Typecast 연동 진단 로그. 어떤 경로가 실제로 동작했는지 추적한다.
 * 자동 · 수동 녹음 화면에서 같은 컴포넌트를 쓴다.
 */
export const TypecastDiagnosticsLog: React.FC<TypecastDiagnosticsLogProps> = ({ onCopy }) => {
  const [entries, setEntries] = useState<TypecastDebugPayload[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const unlisten = listen<TypecastDebugPayload>('typecast_debug', (event) => {
      setEntries((prev) => [event.payload, ...prev].slice(0, 60));
    });
    return () => {
      unlisten.then((u) => u());
    };
  }, []);

  const copyAll = async () => {
    const text = entries
      .map((e) => `${e.at} [${e.kind}] ${e.detail}`)
      .reverse()
      .join('\n');
    try {
      await invoke('copy_text_to_clipboard', { text });
      onCopy?.('진단 로그를 클립보드에 복사했습니다.');
    } catch (err) {
      console.error('진단 로그 복사 실패:', err);
    }
  };

  return (
    <div className="rounded-2xl border border-slate-800 bg-slate-900/60 overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center justify-between px-4 py-3 text-[11px] font-semibold text-slate-400 hover:text-slate-200 transition"
      >
        <span className="flex items-center gap-2">
          <Globe className="w-3.5 h-3.5" />
          연동 진단 로그 ({entries.length})
        </span>
        <ChevronDown className={`w-3.5 h-3.5 transition-transform ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="border-t border-slate-800">
          <div className="max-h-52 overflow-y-auto px-4 py-2 space-y-1">
            {entries.length === 0 ? (
              <p className="text-[10px] text-slate-600 py-2">
                아직 기록이 없습니다. "연동 테스트"를 눌러 보세요.
              </p>
            ) : (
              entries.map((entry, i) => (
                <div key={`${entry.at}-${i}`} className="flex gap-2 text-[10px] font-mono">
                  <span className="text-slate-600 shrink-0">{entry.at}</span>
                  <span className="text-cyan-500 shrink-0">{entry.kind}</span>
                  <span className="text-slate-400 break-all">{entry.detail}</span>
                </div>
              ))
            )}
          </div>
          {entries.length > 0 && (
            <div className="flex items-center gap-3 px-4 py-2 border-t border-slate-800">
              <button
                onClick={copyAll}
                className="text-[10px] font-semibold text-slate-400 hover:text-slate-200 transition"
              >
                로그 복사
              </button>
              <button
                onClick={() => setEntries([])}
                className="text-[10px] font-semibold text-slate-400 hover:text-slate-200 transition"
              >
                지우기
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
};
