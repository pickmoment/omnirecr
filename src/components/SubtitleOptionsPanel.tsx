import React from 'react';
import type { Settings, SubtitleSplitMode } from '../types';

interface SubtitleOptionsPanelProps {
  settings: Settings;
  onUpdateSettings: (partial: Partial<Settings>) => Promise<void>;
  disabled?: boolean;
  /** 강조 색. 자막 생성기는 amber, 다른 화면에서 바꿔 쓸 수 있다. */
  accent?: 'amber' | 'indigo';
}

const ACCENTS = {
  amber: { border: 'focus:border-amber-600', range: 'accent-amber-500', check: 'accent-amber-500' },
  indigo: {
    border: 'focus:border-indigo-600',
    range: 'accent-indigo-500',
    check: 'accent-indigo-500',
  },
};

/**
 * 자막 생성 옵션 폼.
 *
 * 자막 생성기(단건 편집)와 자막 일괄 생성이 같은 `settings` 필드를 쓰므로
 * 폼도 한 컴포넌트로 공유한다. 한쪽에서 바꾸면 양쪽에 반영된다.
 */
export const SubtitleOptionsPanel: React.FC<SubtitleOptionsPanelProps> = ({
  settings,
  onUpdateSettings,
  disabled = false,
  accent = 'amber',
}) => {
  const c = ACCENTS[accent];
  const isWhisper = settings.subtitle_sync_engine === 'ai-whisper';

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div>
          <label className="text-[11px] font-semibold text-slate-400 mb-1 block">싱크 엔진</label>
          <select
            value={settings.subtitle_sync_engine}
            onChange={(e) =>
              onUpdateSettings({ subtitle_sync_engine: e.target.value as 'ai-whisper' | 'vad' })
            }
            disabled={disabled}
            className={`w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 focus:outline-none ${c.border} disabled:opacity-50`}
          >
            <option value="ai-whisper">로컬 AI Whisper (정확도 우선)</option>
            <option value="vad">고속 음성 파형 VAD (속도 우선)</option>
          </select>
        </div>

        <div>
          <label className="text-[11px] font-semibold text-slate-400 mb-1 block">Whisper 모델</label>
          <select
            value={settings.subtitle_whisper_model}
            onChange={(e) =>
              onUpdateSettings({
                subtitle_whisper_model: e.target.value as Settings['subtitle_whisper_model'],
              })
            }
            disabled={disabled || !isWhisper}
            className={`w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 focus:outline-none ${c.border} disabled:opacity-50`}
          >
            <option value="Xenova/whisper-tiny">Whisper Tiny (39MB · 가장 빠름)</option>
            <option value="Xenova/whisper-base">Whisper Base (73MB · 권장)</option>
            <option value="Xenova/whisper-small">Whisper Small (240MB · 정확)</option>
          </select>
        </div>

        <div>
          <label className="text-[11px] font-semibold text-slate-400 mb-1 block">
            자막 분할 방식
          </label>
          <select
            value={settings.subtitle_split_mode}
            onChange={(e) =>
              onUpdateSettings({ subtitle_split_mode: e.target.value as SubtitleSplitMode })
            }
            disabled={disabled}
            className={`w-full px-3 py-2 rounded-xl bg-slate-950 border border-slate-800 text-xs text-slate-200 focus:outline-none ${c.border} disabled:opacity-50`}
          >
            <option value="auto">자동 (문장 + 길이)</option>
            <option value="sentence">문장 단위</option>
            <option value="line">줄 단위</option>
            <option value="length">글자 수 단위</option>
          </select>
        </div>

        <label className="block">
          <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
            한 줄 최대 글자 수 ({settings.subtitle_max_chars}자)
          </span>
          <input
            type="range"
            min={10}
            max={60}
            step={1}
            value={settings.subtitle_max_chars}
            onChange={(e) => onUpdateSettings({ subtitle_max_chars: Number(e.target.value) })}
            disabled={disabled}
            className={`w-full ${c.range}`}
          />
        </label>
      </div>

      {!isWhisper && (
        <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
          <label className="block">
            <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
              무음 판정 임계값 ({settings.subtitle_silence_threshold_db} dB)
            </span>
            <input
              type="range"
              min={-60}
              max={-15}
              step={1}
              value={settings.subtitle_silence_threshold_db}
              onChange={(e) =>
                onUpdateSettings({ subtitle_silence_threshold_db: Number(e.target.value) })
              }
              disabled={disabled}
              className={`w-full ${c.range}`}
            />
          </label>
          <label className="block">
            <span className="text-[11px] font-semibold text-slate-400 mb-1 block">
              최소 무음 길이 ({settings.subtitle_min_silence_duration.toFixed(2)}초)
            </span>
            <input
              type="range"
              min={0.05}
              max={1}
              step={0.05}
              value={settings.subtitle_min_silence_duration}
              onChange={(e) =>
                onUpdateSettings({ subtitle_min_silence_duration: Number(e.target.value) })
              }
              disabled={disabled}
              className={`w-full ${c.range}`}
            />
          </label>
        </div>
      )}

      <label className="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={settings.subtitle_split_on_comma}
          onChange={(e) => onUpdateSettings({ subtitle_split_on_comma: e.target.checked })}
          disabled={disabled}
          className={`w-4 h-4 ${c.check}`}
        />
        <span className="text-[11px] font-semibold text-slate-300">쉼표에서도 나누기</span>
      </label>
    </div>
  );
};
