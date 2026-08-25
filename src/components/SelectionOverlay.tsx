import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, X, Crop, Monitor, Sparkles } from 'lucide-react';
import type { RectRegion } from '../types';

export const SelectionOverlay: React.FC = () => {
  const [isDragging, setIsDragging] = useState(false);
  const [startPoint, setStartPoint] = useState<{ x: number; y: number } | null>(null);
  const [currentRect, setCurrentRect] = useState<RectRegion | null>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        invoke('hide_selection_overlay');
      } else if (e.key === 'Enter') {
        if (currentRect && currentRect.width > 20 && currentRect.height > 20) {
          invoke('confirm_selection_region', { region: currentRect });
        } else {
          // Confirm full screen
          const fullRect: RectRegion = {
            x: 0,
            y: 0,
            width: window.innerWidth,
            height: window.innerHeight,
          };
          invoke('confirm_selection_region', { region: fullRect });
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [currentRect]);

  const handleMouseDown = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('.preset-bar') || (e.target as HTMLElement).closest('.action-popup')) {
      return;
    }
    setIsDragging(true);
    setStartPoint({ x: e.clientX, y: e.clientY });
    setCurrentRect({
      x: e.clientX,
      y: e.clientY,
      width: 0,
      height: 0,
    });
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (!isDragging || !startPoint) return;

    const x = Math.min(startPoint.x, e.clientX);
    const y = Math.min(startPoint.y, e.clientY);
    const width = Math.abs(e.clientX - startPoint.x);
    const height = Math.abs(e.clientY - startPoint.y);

    setCurrentRect({ x, y, width, height });
  };

  const handleMouseUp = () => {
    setIsDragging(false);
  };

  const handleApplyPreset = (w: number, h: number) => {
    const screenW = window.innerWidth;
    const screenH = window.innerHeight;
    const x = Math.max(0, Math.floor((screenW - w) / 2));
    const y = Math.max(0, Math.floor((screenH - h) / 2));

    const rect: RectRegion = {
      x,
      y,
      width: Math.min(w, screenW),
      height: Math.min(h, screenH),
    };
    setCurrentRect(rect);
  };

  const handleSelectFullScreen = () => {
    const fullRect: RectRegion = {
      x: 0,
      y: 0,
      width: window.innerWidth,
      height: window.innerHeight,
    };
    setCurrentRect(fullRect);
  };

  const handleConfirm = () => {
    if (currentRect && currentRect.width > 20 && currentRect.height > 20) {
      invoke('confirm_selection_region', { region: currentRect });
    }
  };

  const handleCancel = () => {
    invoke('hide_selection_overlay');
  };

  const hasSelection = currentRect && currentRect.width > 0 && currentRect.height > 0;
  const screenW = typeof window !== 'undefined' ? window.innerWidth : 1920;
  const screenH = typeof window !== 'undefined' ? window.innerHeight : 1080;

  return (
    <div
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      className="fixed inset-0 select-none cursor-crosshair overflow-hidden"
      style={{
        width: '100vw',
        height: '100vh',
        background: 'transparent',
      }}
    >
      {/* 4 Dimming Backdrop Rectangles around selection box (Leaves selection 100% clear) */}
      {hasSelection ? (
        <>
          {/* Top */}
          <div
            className="absolute bg-black/40 pointer-events-none"
            style={{
              top: 0,
              left: 0,
              width: '100vw',
              height: `${currentRect.y}px`,
            }}
          />
          {/* Bottom */}
          <div
            className="absolute bg-black/40 pointer-events-none"
            style={{
              top: `${currentRect.y + currentRect.height}px`,
              left: 0,
              width: '100vw',
              height: `${Math.max(0, screenH - (currentRect.y + currentRect.height))}px`,
            }}
          />
          {/* Left */}
          <div
            className="absolute bg-black/40 pointer-events-none"
            style={{
              top: `${currentRect.y}px`,
              left: 0,
              width: `${currentRect.x}px`,
              height: `${currentRect.height}px`,
            }}
          />
          {/* Right */}
          <div
            className="absolute bg-black/40 pointer-events-none"
            style={{
              top: `${currentRect.y}px`,
              left: `${currentRect.x + currentRect.width}px`,
              width: `${Math.max(0, screenW - (currentRect.x + currentRect.width))}px`,
              height: `${currentRect.height}px`,
            }}
          />
        </>
      ) : (
        /* Subtle full-screen tint when no drag yet */
        <div className="absolute inset-0 bg-black/25 pointer-events-none" />
      )}

      {/* Top Floating Control / Preset Bar */}
      <div className="preset-bar fixed top-6 left-1/2 -translate-x-1/2 bg-slate-900/95 border border-slate-700/90 px-5 py-3 rounded-2xl shadow-2xl backdrop-blur-md flex items-center gap-3 z-50 text-slate-100">
        <div className="flex items-center gap-2 text-xs font-bold text-slate-100 pr-3 border-r border-slate-700">
          <Crop className="w-4 h-4 text-blue-400" />
          <span>녹화 영역 드래그 지정</span>
        </div>

        {/* Quick Presets */}
        <div className="flex items-center gap-1.5">
          <button
            onClick={handleSelectFullScreen}
            className="flex items-center gap-1 px-2.5 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs rounded-lg font-medium transition"
          >
            <Monitor className="w-3.5 h-3.5" />
            <span>전체 화면</span>
          </button>
          <button
            onClick={() => handleApplyPreset(1920, 1080)}
            className="px-2.5 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs rounded-lg font-medium transition"
          >
            1920×1080 (FHD)
          </button>
          <button
            onClick={() => handleApplyPreset(1280, 720)}
            className="px-2.5 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs rounded-lg font-medium transition"
          >
            1280×720 (HD)
          </button>
          <button
            onClick={() => handleApplyPreset(1080, 1080)}
            className="px-2.5 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs rounded-lg font-medium transition"
          >
            1:1 정방형
          </button>
        </div>

        <div className="flex items-center gap-2 pl-3 border-l border-slate-700 text-xs">
          <span className="text-slate-400 font-mono">
            <kbd className="px-1.5 py-0.5 bg-slate-800 rounded border border-slate-600 text-white font-semibold">Enter</kbd> 확정
          </span>
          <span className="text-slate-400 font-mono">
            <kbd className="px-1.5 py-0.5 bg-slate-800 rounded border border-slate-600 text-white font-semibold">ESC</kbd> 취소
          </span>

          {hasSelection && currentRect.width > 20 && (
            <button
              onClick={handleConfirm}
              className="flex items-center gap-1 px-3 py-1.5 bg-blue-600 hover:bg-blue-500 text-white rounded-lg font-bold shadow-lg shadow-blue-600/30 transition"
            >
              <Check className="w-3.5 h-3.5" />
              <span>영역 확정</span>
            </button>
          )}

          <button
            onClick={handleCancel}
            title="취소"
            className="p-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-400 hover:text-white transition"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* Selected Bounding Box (Border & Live Dimensions) */}
      {hasSelection && (
        <div
          className="absolute border-2 border-blue-400 bg-transparent pointer-events-none shadow-[0_0_15px_rgba(59,130,246,0.5)]"
          style={{
            left: `${currentRect.x}px`,
            top: `${currentRect.y}px`,
            width: `${currentRect.width}px`,
            height: `${currentRect.height}px`,
          }}
        >
          {/* Dimension Tag */}
          <div className="absolute -top-8 left-0 px-2.5 py-1 bg-blue-600 text-white font-mono text-xs font-bold rounded-lg shadow-lg flex items-center gap-1.5 whitespace-nowrap">
            <Sparkles className="w-3 h-3 text-cyan-300" />
            <span>
              {currentRect.width} × {currentRect.height} px (X: {currentRect.x}, Y: {currentRect.y})
            </span>
          </div>

          {/* Corner Handles */}
          <div className="absolute -top-1.5 -left-1.5 w-3 h-3 bg-white border-2 border-blue-600 rounded-sm" />
          <div className="absolute -top-1.5 -right-1.5 w-3 h-3 bg-white border-2 border-blue-600 rounded-sm" />
          <div className="absolute -bottom-1.5 -left-1.5 w-3 h-3 bg-white border-2 border-blue-600 rounded-sm" />
          <div className="absolute -bottom-1.5 -right-1.5 w-3 h-3 bg-white border-2 border-blue-600 rounded-sm" />
        </div>
      )}
    </div>
  );
};
