import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  BookText,
  Check,
  ClipboardCopy,
  Copy,
  Download,
  FolderOpen,
  Mic,
  Plus,
  Save,
  Search,
  Tags,
  Trash2,
  Upload,
  X,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import type { ScriptDraft, ScriptItem } from '../types';
import { formatDuration } from '../utils/format';

interface ScriptLibraryProps {
  scripts: ScriptItem[];
  isLoading: boolean;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  onRefresh: () => Promise<void>;
  /** 자동 일괄 녹음 대상으로 체크된 대본 ID들 */
  batchIds: string[];
  onToggleBatch: (id: string) => void;
  onSendToTts: (script: ScriptItem) => void;
  onOpenExplorer: (path: string) => Promise<void>;
}

const emptyDraft: ScriptDraft = { id: null, title: '', content: '', tags: [], memo: '' };

export const ScriptLibrary: React.FC<ScriptLibraryProps> = ({
  scripts,
  isLoading,
  selectedId,
  onSelect,
  onRefresh,
  batchIds,
  onToggleBatch,
  onSendToTts,
  onOpenExplorer,
}) => {
  const [draft, setDraft] = useState<ScriptDraft>(emptyDraft);
  const [tagInput, setTagInput] = useState('');
  const [searchQuery, setSearchQuery] = useState('');
  const [isDirty, setIsDirty] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const flashTimerRef = useRef<number | null>(null);

  // 편집기가 실제로 붙들고 있는 대본 id. 목록 강조(selectedId)와 어긋날 수 있다.
  const draftId = draft.id ?? null;

  const selected = useMemo(
    () => scripts.find((s) => s.id === selectedId) ?? null,
    [scripts, selectedId],
  );

  // 편집 중인 대본이 아직 목록에 살아 있는지(다른 화면에서 지워지지 않았는지)
  const editing = useMemo(
    () => (draftId ? (scripts.find((s) => s.id === draftId) ?? null) : null),
    [scripts, draftId],
  );

  const filtered = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return scripts;
    return scripts.filter(
      (s) =>
        s.title.toLowerCase().includes(q) ||
        s.content.toLowerCase().includes(q) ||
        s.tags.some((t) => t.toLowerCase().includes(q)),
    );
  }, [scripts, searchQuery]);

  const flash = (msg: string) => {
    setFeedback(msg);
    setErrorMsg(null);
    if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    flashTimerRef.current = window.setTimeout(() => {
      flashTimerRef.current = null;
      setFeedback((cur) => (cur === msg ? null : cur));
    }, 2600);
  };

  // 탭을 옮겨 이 화면이 사라져도 안내 타이머가 남지 않게 한다.
  useEffect(
    () => () => {
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    },
    [],
  );

  const loadIntoEditor = (item: ScriptItem | null) => {
    if (!item) {
      setDraft(emptyDraft);
      setTagInput('');
    } else {
      setDraft({
        id: item.id,
        title: item.title,
        content: item.content,
        tags: item.tags,
        memo: item.memo,
      });
      setTagInput(item.tags.join(', '));
    }
    setIsDirty(false);
  };

  // 저장하지 않은 편집을 안고 있는데 바깥(수동 녹음 드롭다운 · "TTS로 보내기" 등)에서
  // 선택이 다른 대본으로 바뀐 상태.
  const hasSelectionConflict = isDirty && selectedId !== null && draftId !== selectedId;
  // 편집 중이던 대본이 다른 화면에서 삭제된 상태(그대로 저장하면 "찾을 수 없습니다" 로 실패한다).
  const isEditingOrphan = draftId !== null && editing === null;
  const showDraftGuard = hasSelectionConflict || (isEditingOrphan && isDirty);

  // ── 외부 선택 변경 추종 ─────────────────────────────────
  // 정책: 편집 내용이 깨끗하면 편집기가 선택을 그대로 따라간다.
  //       저장하지 않은 편집이 있으면 절대 따라가지 않고(= 조용히 버리지 않고)
  //       위 배너로 "저장 후 이동 / 버리고 이동 / 선택을 편집 중인 대본으로 되돌리기"를 고르게 한다.
  // 이 추종을 지우면 목록은 B 를 강조하는데 저장은 A 로 날아가 A 를 덮어쓰는 사고가 난다.
  useEffect(() => {
    if (draftId === selectedId) return;
    if (isDirty) return;
    if (selectedId === null) {
      // 선택이 비었을 때는 편집하던 대본이 실제로 사라진 경우에만 편집기를 비운다.
      if (draftId !== null && !scripts.some((s) => s.id === draftId)) loadIntoEditor(null);
      return;
    }
    const next = scripts.find((s) => s.id === selectedId);
    // 목록 갱신이 아직 도착하지 않았으면 다음 렌더에서 다시 맞춘다.
    if (!next) return;
    loadIntoEditor(next);
  }, [selectedId, scripts, draftId, isDirty]);

  const handleSelect = (item: ScriptItem) => {
    if (isDirty && !window.confirm('저장하지 않은 변경 사항이 있습니다. 그래도 이동할까요?')) return;
    onSelect(item.id);
    loadIntoEditor(item);
  };

  const handleNew = () => {
    if (isDirty && !window.confirm('저장하지 않은 변경 사항이 있습니다. 새 대본을 시작할까요?')) return;
    onSelect(null);
    loadIntoEditor(null);
  };

  /** 편집기 내용을 저장한다. asNew 면 id 를 떼고 새 대본으로 만든다(원본이 삭제된 경우). */
  const persistDraft = async (asNew = false): Promise<ScriptItem | null> => {
    if (!draft.content.trim()) {
      setErrorMsg('대본 본문이 비어 있습니다.');
      return null;
    }
    setIsSaving(true);
    setErrorMsg(null);
    try {
      const tags = tagInput
        .split(',')
        .map((t) => t.trim())
        .filter(Boolean);
      const saved = await invoke<ScriptItem>('save_script', {
        draft: { ...draft, id: asNew ? null : draft.id, tags },
      });
      await onRefresh();
      setIsDirty(false);
      return saved;
    } catch (err) {
      setErrorMsg(`대본 저장 실패: ${err}`);
      return null;
    } finally {
      setIsSaving(false);
    }
  };

  const handleSave = async () => {
    const saved = await persistDraft(isEditingOrphan);
    if (!saved) return;
    // 선택과 편집 대상이 어긋나 있었다면(배너 상태) 선택은 건드리지 않는다.
    // isDirty 가 풀렸으므로 위 추종 effect 가 편집기를 selectedId 쪽으로 옮긴다.
    // 반대로 따라갈 선택이 아예 없으면(편집하던 대본이 삭제된 경우 등)
    // 방금 저장한 대본을 그대로 열어 둔다 — 안 그러면 저장 직후 편집기가 텅 빈다.
    if (draftId === selectedId || selectedId === null) {
      onSelect(saved.id);
      loadIntoEditor(saved);
    }
    flash(`"${saved.title}" 저장 완료`);
  };

  /** 배너: 편집 내용을 저장한 뒤 바깥에서 선택된 대본으로 이동한다. */
  const handleSaveThenFollow = async () => {
    const saved = await persistDraft(isEditingOrphan);
    if (!saved) return;
    if (selectedId === null) {
      // 따라갈 선택이 없다. 방금 저장한 대본을 계속 편집하게 둔다.
      onSelect(saved.id);
      loadIntoEditor(saved);
      flash(`"${saved.title}" 저장 완료`);
      return;
    }
    flash(`"${saved.title}" 저장 완료 — 선택된 대본으로 이동합니다.`);
  };

  /** 배너: 편집 내용을 사용자가 명시적으로 버리고 선택된 대본으로 이동한다. */
  const handleDiscardThenFollow = () => {
    setIsDirty(false);
    loadIntoEditor(selected);
  };

  /** 배너: 선택 강조를 편집 중인 대본으로 되돌린다(편집 계속). */
  const handleKeepEditing = () => {
    onSelect(draftId);
  };

  const handleDelete = async (item: ScriptItem) => {
    const dirtyWarning =
      isDirty && draftId === item.id
        ? '\n편집기에 저장하지 않은 변경 사항도 함께 사라집니다.'
        : '';
    if (!window.confirm(`"${item.title}" 대본을 삭제할까요? 되돌릴 수 없습니다.${dirtyWarning}`))
      return;
    try {
      await invoke('delete_script', { id: item.id });
      await onRefresh();
      if (selectedId === item.id) onSelect(null);
      // 선택과 편집 대상이 다를 수 있으므로 편집기는 따로 판단한다.
      if (draftId === item.id) loadIntoEditor(null);
      flash('대본을 삭제했습니다.');
    } catch (err) {
      setErrorMsg(`대본 삭제 실패: ${err}`);
    }
  };

  const handleDuplicate = async (item: ScriptItem) => {
    // 복제본을 편집기에 여는 동작이라 저장하지 않은 편집을 삼킬 수 있다.
    if (isDirty && !window.confirm('저장하지 않은 변경 사항이 있습니다. 복제본을 편집기에 열까요?'))
      return;
    try {
      const copy = await invoke<ScriptItem>('duplicate_script', { id: item.id });
      await onRefresh();
      onSelect(copy.id);
      loadIntoEditor(copy);
      flash('대본을 복제했습니다.');
    } catch (err) {
      setErrorMsg(`대본 복제 실패: ${err}`);
    }
  };

  const handleImport = async () => {
    // 가져온 대본을 편집기에 여는 동작이라 저장하지 않은 편집을 삼킬 수 있다.
    if (
      isDirty &&
      !window.confirm('저장하지 않은 변경 사항이 있습니다. 가져온 대본을 편집기에 열까요?')
    )
      return;
    try {
      const picked = await open({
        multiple: false,
        filters: [{ name: '텍스트 대본', extensions: ['txt', 'md', 'srt', 'vtt', 'text'] }],
      });
      if (!picked || typeof picked !== 'string') return;
      const imported = await invoke<ScriptItem>('import_script_file', { path: picked });
      await onRefresh();
      onSelect(imported.id);
      loadIntoEditor(imported);
      flash(`"${imported.title}" 가져오기 완료`);
    } catch (err) {
      setErrorMsg(`대본 가져오기 실패: ${err}`);
    }
  };

  const handleExport = async (item: ScriptItem) => {
    try {
      const target = await save({
        defaultPath: `${item.title}.txt`,
        filters: [{ name: '텍스트 파일', extensions: ['txt'] }],
      });
      if (!target) return;
      await invoke('export_script_file', { id: item.id, path: target });
      flash('대본을 파일로 저장했습니다.');
    } catch (err) {
      setErrorMsg(`대본 내보내기 실패: ${err}`);
    }
  };

  const handleCopy = async (item: ScriptItem) => {
    try {
      await invoke('copy_text_to_clipboard', { text: item.content });
      flash('대본을 클립보드에 복사했습니다.');
    } catch (err) {
      setErrorMsg(`클립보드 복사 실패: ${err}`);
    }
  };

  const draftCharCount = draft.content.length;
  const draftLineCount = draft.content.split('\n').filter((l) => l.trim().length > 0).length;
  const draftEstimate =
    draft.content.replace(/\s/g, '').length / 5.5 +
    (draft.content.match(/[.!?,…]/g)?.length ?? 0) * 0.35;

  // 편집기 패널이 기준으로 삼는 대본. 편집 대상이 우선이고, 새 대본일 때만 선택을 본다.
  const editorTarget = editing ?? selected;

  return (
    <div className="flex flex-col lg:flex-row gap-4 h-full min-h-0">
      {/* ── 대본 목록 ───────────────────────────── */}
      <div className="lg:w-[340px] shrink-0 flex flex-col gap-3 min-h-0">
        <div className="flex items-center gap-2">
          <button
            onClick={handleNew}
            className="flex items-center gap-1.5 px-3 py-2 rounded-xl bg-emerald-600 hover:bg-emerald-500 text-white text-xs font-bold shadow-lg shadow-emerald-600/20 transition active:scale-95"
          >
            <Plus className="w-4 h-4" />
            <span>새 대본</span>
          </button>
          <button
            onClick={handleImport}
            className="flex items-center gap-1.5 px-3 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
          >
            <Upload className="w-3.5 h-3.5 text-emerald-400" />
            <span>파일 가져오기</span>
          </button>
        </div>

        <div className="relative">
          <Search className="w-4 h-4 text-slate-500 absolute left-3 top-1/2 -translate-y-1/2" />
          <input
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="제목 · 본문 · 태그 검색"
            className="w-full pl-9 pr-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 placeholder:text-slate-600 focus:outline-none focus:border-emerald-600"
          />
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto space-y-2 pr-1">
          {isLoading && (
            <div className="text-xs text-slate-500 text-center py-6">대본을 불러오는 중...</div>
          )}

          {!isLoading && filtered.length === 0 && (
            <div className="text-center py-10 px-4 rounded-2xl border border-dashed border-slate-800 bg-slate-900/40">
              <BookText className="w-8 h-8 text-slate-700 mx-auto mb-2" />
              <p className="text-xs text-slate-500">
                {scripts.length === 0
                  ? '저장된 대본이 없습니다.\n"새 대본"으로 시작해 보세요.'
                  : '검색 결과가 없습니다.'}
              </p>
            </div>
          )}

          {filtered.map((item) => {
            const isActive = item.id === selectedId;
            // 저장하지 않은 편집 때문에 선택과 편집 대상이 갈린 경우, 어느 쪽이 편집기인지 보여준다.
            const isInEditor = item.id === draftId;
            return (
              <div
                key={item.id}
                onClick={() => handleSelect(item)}
                className={`group rounded-xl border p-3 cursor-pointer transition ${
                  isActive
                    ? 'bg-emerald-950/40 border-emerald-600/60 shadow-lg shadow-emerald-900/20'
                    : isInEditor && showDraftGuard
                      ? 'bg-amber-950/25 border-amber-600/60'
                      : 'bg-slate-900/70 border-slate-800 hover:border-slate-700'
                }`}
              >
                <div className="flex items-start gap-2">
                  <input
                    type="checkbox"
                    title="자동 일괄 녹음 대상에 포함"
                    checked={batchIds.includes(item.id)}
                    onClick={(e) => e.stopPropagation()}
                    onChange={() => onToggleBatch(item.id)}
                    className="w-3.5 h-3.5 mt-0.5 accent-indigo-500 shrink-0"
                  />
                  <span className="text-xs font-bold text-slate-100 line-clamp-1 flex-1">
                    {item.title}
                  </span>
                  {isInEditor && showDraftGuard && (
                    <span className="shrink-0 text-[10px] font-bold px-1.5 py-0.5 rounded bg-amber-950/70 border border-amber-600/50 text-amber-300">
                      편집 중
                    </span>
                  )}
                  {item.record_count > 0 && (
                    <span className="shrink-0 text-[10px] font-bold px-1.5 py-0.5 rounded bg-indigo-950/70 border border-indigo-700/40 text-indigo-300">
                      녹음 {item.record_count}
                    </span>
                  )}
                </div>

                <p className="text-[11px] text-slate-500 mt-1 line-clamp-2 whitespace-pre-wrap">
                  {item.content.slice(0, 90) || '(빈 대본)'}
                </p>

                <div className="flex items-center gap-2 mt-2 text-[10px] text-slate-500 font-mono">
                  <span>{item.char_count.toLocaleString()}자</span>
                  <span>·</span>
                  <span>{item.line_count}줄</span>
                  <span>·</span>
                  <span className="text-emerald-500">≈{formatDuration(item.estimated_secs)}</span>
                </div>

                {item.tags.length > 0 && (
                  <div className="flex flex-wrap gap-1 mt-2">
                    {item.tags.map((t) => (
                      <span
                        key={t}
                        className="text-[10px] px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 border border-slate-700"
                      >
                        #{t}
                      </span>
                    ))}
                  </div>
                )}

                <div className="flex items-center gap-1 mt-2.5 opacity-0 group-hover:opacity-100 transition-opacity">
                  <button
                    title="TTS 녹음으로 보내기"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSendToTts(item);
                    }}
                    className="p-1.5 rounded-lg bg-indigo-950/70 hover:bg-indigo-900 text-indigo-300 border border-indigo-800/50 transition"
                  >
                    <Mic className="w-3.5 h-3.5" />
                  </button>
                  <button
                    title="클립보드에 복사"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleCopy(item);
                    }}
                    className="p-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 transition"
                  >
                    <ClipboardCopy className="w-3.5 h-3.5" />
                  </button>
                  <button
                    title="복제"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDuplicate(item);
                    }}
                    className="p-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 transition"
                  >
                    <Copy className="w-3.5 h-3.5" />
                  </button>
                  <button
                    title="텍스트 파일로 내보내기"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleExport(item);
                    }}
                    className="p-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 transition"
                  >
                    <Download className="w-3.5 h-3.5" />
                  </button>
                  <button
                    title="삭제"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDelete(item);
                    }}
                    className="p-1.5 rounded-lg bg-red-950/60 hover:bg-red-900/70 text-red-300 border border-red-900/50 transition ml-auto"
                  >
                    <Trash2 className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* ── 대본 편집기 ─────────────────────────── */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col gap-3">
        {showDraftGuard && (
          <div className="bg-amber-950/30 border border-amber-700/60 rounded-2xl p-3.5 space-y-2.5 shadow-lg">
            <div className="flex items-start gap-2">
              <AlertTriangle className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />
              <p className="text-[11px] text-amber-100 leading-relaxed">
                {isEditingOrphan ? (
                  <>
                    편집 중이던 대본이 목록에서 사라졌습니다(다른 화면에서 삭제됨). 편집 내용은 그대로
                    두었으니 <b className="text-amber-300">새 대본으로 저장</b>하거나 직접 버리세요.
                  </>
                ) : (
                  <>
                    저장하지 않은 편집이{' '}
                    <b className="text-amber-300">
                      "{draftId ? draft.title || editing?.title || '(제목 없음)' : '새 대본'}"
                    </b>{' '}
                    에 남아 있는데, 목록 선택은{' '}
                    <b className="text-amber-300">"{selected?.title ?? '(없음)'}"</b> 로 바뀌었습니다.
                    지금 저장하면 <b>편집 중인 대본</b>에 저장됩니다.
                  </>
                )}
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {!isEditingOrphan && (
                <button
                  onClick={handleKeepEditing}
                  className="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 text-[11px] font-semibold border border-slate-700 transition"
                >
                  선택을 편집 중인 대본으로 되돌리기
                </button>
              )}
              <button
                onClick={handleSaveThenFollow}
                disabled={isSaving || !draft.content.trim()}
                className="px-3 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:bg-slate-800 disabled:text-slate-500 text-white text-[11px] font-bold transition"
              >
                {isSaving
                  ? '저장 중...'
                  : isEditingOrphan
                    ? '새 대본으로 저장'
                    : '저장하고 선택된 대본 열기'}
              </button>
              <button
                onClick={handleDiscardThenFollow}
                className="px-3 py-1.5 rounded-lg bg-red-950/60 hover:bg-red-900/70 text-red-200 text-[11px] font-semibold border border-red-900/50 transition"
              >
                {selected ? '편집 버리고 선택된 대본 열기' : '편집 버리기'}
              </button>
            </div>
          </div>
        )}

        <div className="bg-slate-900/80 border border-slate-800 rounded-2xl p-4 space-y-3 shadow-lg">
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm font-bold text-slate-200 flex items-center gap-2 min-w-0">
              <BookText className="w-4 h-4 text-emerald-400 shrink-0" />
              <span className="shrink-0">{draftId ? '대본 편집' : '새 대본 작성'}</span>
              {draftId && (
                <span className="text-[11px] font-semibold text-slate-400 truncate">
                  · {draft.title || editing?.title || '(제목 없음)'}
                </span>
              )}
            </span>
            <div className="flex items-center gap-2">
              {isDirty && (
                <span className="text-[11px] font-semibold text-amber-400">저장되지 않음</span>
              )}
              <button
                onClick={handleSave}
                disabled={isSaving || !draft.content.trim()}
                className="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-emerald-600 hover:bg-emerald-500 disabled:bg-slate-800 disabled:text-slate-500 text-white text-xs font-bold shadow-lg shadow-emerald-600/20 transition active:scale-95"
              >
                <Save className="w-3.5 h-3.5" />
                <span>
                  {isSaving
                    ? '저장 중...'
                    : isEditingOrphan
                      ? '새 대본으로 저장'
                      : showDraftGuard
                        ? '편집 중인 대본에 저장'
                        : '대본 저장'}
                </span>
              </button>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            <div>
              <label className="text-[11px] font-semibold text-slate-400 mb-1 block">제목</label>
              <input
                value={draft.title}
                onChange={(e) => {
                  setDraft((d) => ({ ...d, title: e.target.value }));
                  setIsDirty(true);
                }}
                placeholder="비워두면 첫 줄로 자동 생성됩니다"
                className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 placeholder:text-slate-600 focus:outline-none focus:border-emerald-600"
              />
            </div>
            <div>
              <label className="text-[11px] font-semibold text-slate-400 mb-1 flex items-center gap-1">
                <Tags className="w-3 h-3" />
                태그 (쉼표로 구분)
              </label>
              <input
                value={tagInput}
                onChange={(e) => {
                  setTagInput(e.target.value);
                  setIsDirty(true);
                }}
                placeholder="예: 유튜브, 인트로, 나레이션"
                className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 placeholder:text-slate-600 focus:outline-none focus:border-emerald-600"
              />
            </div>
          </div>

          <div>
            <label className="text-[11px] font-semibold text-slate-400 mb-1 block">메모 (선택)</label>
            <input
              value={draft.memo}
              onChange={(e) => {
                setDraft((d) => ({ ...d, memo: e.target.value }));
                setIsDirty(true);
              }}
              placeholder="목소리 설정, 속도, 참고 사항 등"
              className="w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 placeholder:text-slate-600 focus:outline-none focus:border-emerald-600"
            />
          </div>
        </div>

        <div className="flex-1 min-h-0 bg-slate-900/80 border border-slate-800 rounded-2xl p-4 flex flex-col shadow-lg">
          <div className="flex items-center justify-between mb-2">
            <label className="text-[11px] font-semibold text-slate-400">대본 본문</label>
            <div className="flex items-center gap-2 text-[10px] font-mono text-slate-500">
              <span>{draftCharCount.toLocaleString()}자</span>
              <span>·</span>
              <span>{draftLineCount}줄</span>
              <span>·</span>
              <span className="text-emerald-500">예상 낭독 ≈{formatDuration(draftEstimate)}</span>
            </div>
          </div>
          <textarea
            value={draft.content}
            onChange={(e) => {
              setDraft((d) => ({ ...d, content: e.target.value }));
              setIsDirty(true);
            }}
            placeholder={'문장 단위로 줄바꿈해서 작성하면\nTTS 낭독과 자막 싱크 품질이 좋아집니다.'}
            spellCheck={false}
            className="flex-1 min-h-[220px] w-full px-3.5 py-3 rounded-xl bg-slate-950 border border-slate-800 text-sm leading-relaxed text-slate-200 placeholder:text-slate-600 resize-none focus:outline-none focus:border-emerald-600 font-sans"
          />
        </div>

        {/* 이 카드는 편집기 패널의 일부다. 선택(강조)과 편집 대상이 갈렸을 때
            엉뚱한 대본의 녹음 결과를 보여주지 않도록 편집 중인 대본을 우선한다. */}
        {editorTarget?.last_recorded_path && (
          <div className="bg-indigo-950/30 border border-indigo-800/50 rounded-2xl p-3 flex items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="text-[11px] font-bold text-indigo-300">
                최근 녹음 결과 · {editorTarget.title}
              </p>
              <p className="text-[11px] text-slate-400 font-mono truncate">
                {editorTarget.last_recorded_path}
              </p>
              {editorTarget.last_recorded_at && (
                <p className="text-[10px] text-slate-500 mt-0.5">{editorTarget.last_recorded_at}</p>
              )}
            </div>
            <button
              onClick={() => onOpenExplorer(editorTarget.last_recorded_path!)}
              className="shrink-0 flex items-center gap-1.5 px-3 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-semibold border border-slate-700 transition"
            >
              <FolderOpen className="w-3.5 h-3.5 text-indigo-400" />
              <span>폴더 열기</span>
            </button>
          </div>
        )}

        {(feedback || errorMsg) && (
          <div
            className={`rounded-xl px-3.5 py-2.5 text-xs font-semibold flex items-center gap-2 border ${
              errorMsg
                ? 'bg-red-950/50 border-red-800/60 text-red-300'
                : 'bg-emerald-950/50 border-emerald-800/60 text-emerald-300'
            }`}
          >
            {errorMsg ? <X className="w-3.5 h-3.5" /> : <Check className="w-3.5 h-3.5" />}
            <span>{errorMsg || feedback}</span>
          </div>
        )}
      </div>
    </div>
  );
};
