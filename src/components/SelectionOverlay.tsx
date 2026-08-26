import React, { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Check, X, Crop, Monitor, Sparkles } from 'lucide-react';
import type { RectRegion, ScreenCaptureInfo } from '../types';

export const SelectionOverlay: React.FC = () => {
  const [screenshot, setScreenshot] = useState<ScreenCaptureInfo | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [startPoint, setStartPoint] = useState<{ x: number; y: number } | null>(null);
  const [currentRect, setCurrentRect] = useState<RectRegion | null>(null);

  // Fetch or listen for the screenshot captured just before showing the overlay
  useEffect(() => {
    let isMounted = true;

    invoke<ScreenCaptureInfo | null>('get_selection_screen_capture')
      .then((data) => {
        if (isMounted && data) {
          setScreenshot(data);
        }
      })
      .catch((err) => {
        console.error('Failed to get selection screen capture:', err);
      });

    const unlistenPromise = listen<ScreenCaptureInfo>('selection_screen_captured', (event) => {
      if (isMounted && event.payload) {
        setScreenshot(event.payload);
      }
    });

    return () => {
      isMounted = false;
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  const screenW = typeof window !== 'undefined' ? window.innerWidth : 1920;
  const screenH = typeof window !== 'undefined' ? window.innerHeight : 1080;

  const scaleX = screenshot && screenW > 0 ? screenshot.physical_width / screenW : (typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1);
  const scaleY = screenshot && screenH > 0 ? screenshot.physical_height / screenH : (typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1);

  // Converts CSS logical viewport rectangle to physical pixel rectangle for FFmpeg
  const getPhysicalRegion = useCallback(
    (rect: RectRegion | null): RectRegion | null => {
      if (!rect || rect.width <= 0 || rect.height <= 0) return null;
      const x = Math.max(0, Math.round(rect.x * scaleX));
      const y = Math.max(0, Math.round(rect.y * scaleY));
      const width = Math.max(2, Math.round(rect.width * scaleX));
      const height = Math.max(2, Math.round(rect.height * scaleY));
      return { x, y, width, height };
    },
    [scaleX, scaleY]
  );

  const handleConfirm = useCallback(() => {
    const physical = getPhysicalRegion(currentRect);
    if (physical && physical.width > 20 && physical.height > 20) {
      invoke('confirm_selection_region', { region: physical });
    } else {
      // Confirm full screen with exact physical dimensions
      const fullPhysWidth = screenshot?.physical_width || Math.round(screenW * scaleX);
      const fullPhysHeight = screenshot?.physical_height || Math.round(screenH * scaleY);
      const fullRect: RectRegion = {
        x: 0,
        y: 0,
        width: fullPhysWidth,
        height: fullPhysHeight,
      };
      invoke('confirm_selection_region', { region: fullRect });
    }
  }, [currentRect, getPhysicalRegion, screenshot, screenW, screenH, scaleX, scaleY]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        invoke('hide_selection_overlay');
      } else if (e.key === 'Enter') {
        handleConfirm();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleConfirm]);

  const handleMouseDown = (e: React.MouseEvent) => {
    if (
      (e.target as HTMLElement).closest('.preset-bar') ||
      (e.target as HTMLElement).closest('.action-popup')
    ) {
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

  const handleApplyPreset = (physW: number, physH: number) => {
    const cssW = Math.min(physW / scaleX, screenW);
    const cssH = Math.min(physH / scaleY, screenH);
    const x = Math.max(0, Math.floor((screenW - cssW) / 2));
    const y = Math.max(0, Math.floor((screenH - cssH) / 2));

    setCurrentRect({
      x,
      y,
      width: Math.round(cssW),
      height: Math.round(cssH),
    });
  };

  const handleSelectFullScreen = () => {
    setCurrentRect({
      x: 0,
      y: 0,
      width: screenW,
      height: screenH,
    });
  };

  const handleCancel = () => {
    invoke('hide_selection_overlay');
  };

  const hasSelection = currentRect && currentRect.width > 0 && currentRect.height > 0;
  const physicalRect = getPhysicalRegion(currentRect);

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
      {/* 1. Captured Desktop Screen Backdrop (Ensures video/YouTube is 100% visible with 0 occlusion) */}
      {screenshot?.image_data_url && (
        <img
          src={screenshot.image_data_url}
          alt="Desktop"
          className="absolute inset-0 w-full h-full object-fill pointer-events-none select-none z-0"
          draggable={false}
        />
      )}

      {/* 2. Dimming Backdrop Rectangles around selection box (Leaves selected crop area 100% clear) */}
      {hasSelection ? (
        <>
          {/* Top */}
          <div
            className="absolute bg-black/45 pointer-events-none z-10"
            style={{
              top: 0,
              left: 0,
              width: '100vw',
              height: `${currentRect.y}px`,
            }}
          />
          {/* Bottom */}
          <div
            className="absolute bg-black/45 pointer-events-none z-10"
            style={{
              top: `${currentRect.y + currentRect.height}px`,
              left: 0,
              width: '100vw',
              height: `${Math.max(0, screenH - (currentRect.y + currentRect.height))}px`,
            }}
          />
          {/* Left */}
          <div
            className="absolute bg-black/45 pointer-events-none z-10"
            style={{
              top: `${currentRect.y}px`,
              left: 0,
              width: `${currentRect.x}px`,
              height: `${currentRect.height}px`,
            }}
          />
          {/* Right */}
          <div
            className="absolute bg-black/45 pointer-events-none z-10"
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
        <div className="absolute inset-0 bg-black/25 pointer-events-none z-10" />
      )}

      {/* 3. Top Floating Control / Preset Bar */}
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
            onClick={() => {
              const minDim = Math.min(
                screenshot?.physical_width || 1080,
                screenshot?.physical_height || 1080
              );
              handleApplyPreset(minDim, minDim);
            }}
            className="px-2.5 py-1.5 bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs rounded-lg font-medium transition"
          >
            1:1 정방형
          </button>
        </div>

        <div className="flex items-center gap-2 pl-3 border-l border-slate-700 text-xs">
          <span className="text-slate-400 font-mono">
            <kbd className="px-1.5 py-0.5 bg-slate-800 rounded border border-slate-600 text-white font-semibold">
              Enter
            </kbd>{' '}
            확정
          </span>
          <span className="text-slate-400 font-mono">
            <kbd className="px-1.5 py-0.5 bg-slate-800 rounded border border-slate-600 text-white font-semibold">
              ESC
            </kbd>{' '}
            취소
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

      {/* 4. Selected Bounding Box (Border & Live Physical Dimensions) */}
      {hasSelection && physicalRect && (
        <div
          className="absolute border-2 border-blue-400 bg-transparent pointer-events-none shadow-[0_0_15px_rgba(59,130,246,0.5)] z-20"
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
              {physicalRect.width} × {physicalRect.height} px (X: {physicalRect.x}, Y:{' '}
              {physicalRect.y})
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
