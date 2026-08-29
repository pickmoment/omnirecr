import React from 'react';

export interface TabBarItem<T extends string> {
  key: T;
  label: string;
  icon: React.ReactNode;
  /** 선택 시 배경 클래스 (예: 'bg-blue-600 shadow-blue-600/30') */
  accent: string;
}

interface TabBarProps<T extends string> {
  items: TabBarItem<T>[];
  current: T;
  onSelect: (key: T) => void;
  className?: string;
}

/**
 * 상단 탭과 화면 안 서브 탭이 같은 모양을 쓰도록 만든 공용 탭 바.
 * 예전에는 화면마다 같은 마크업을 복사해 두고 있었다.
 */
export const TabBar = <T extends string>({
  items,
  current,
  onSelect,
  className = '',
}: TabBarProps<T>) => (
  <div
    className={`flex items-center gap-1 bg-slate-950/60 p-1 rounded-xl border border-slate-800 ${className}`}
  >
    {items.map((item) => (
      <button
        key={item.key}
        onClick={() => onSelect(item.key)}
        className={`flex items-center gap-2 px-3.5 py-2 rounded-lg text-xs font-semibold transition-all duration-200 whitespace-nowrap ${
          current === item.key
            ? `${item.accent} text-white shadow-md`
            : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/50'
        }`}
      >
        {item.icon}
        <span>{item.label}</span>
      </button>
    ))}
  </div>
);
