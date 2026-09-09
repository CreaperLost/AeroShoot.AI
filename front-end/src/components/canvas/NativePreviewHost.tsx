import { useEffect, useRef, useState } from "react";
import { useProjectStore } from "../../stores/projectStore";
import { api } from "../../lib/ipc";
import { PreviewHitMode, PreviewStatus } from "../../lib/types";

interface NativePreviewHostProps {
  windowLabel?: string;
  hitMode?: PreviewHitMode;
  className?: string;
}

export function NativePreviewHost({
  windowLabel = "main",
  hitMode = "consume",
  className = "w-full max-w-4xl aspect-video",
}: NativePreviewHostProps) {
  const previewAvailable = useProjectStore(s => s.previewAvailable);
  const playbackError = useProjectStore(s => s.playbackError);
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [status, setStatus] = useState<PreviewStatus | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let cancelled = false;
    let revision = 0;
    let generation: number | undefined;
    let animation = 0;
    let lastGeometry = "";
    let sending = false;
    const update = () => {
      if (cancelled) return;
      animation = requestAnimationFrame(update);
      const el = hostRef.current;
      if (!el || generation === undefined || sending) return;
      const rect = el.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      let left = Math.max(0, rect.left), top = Math.max(0, rect.top);
      let right = Math.min(window.innerWidth, rect.right), bottom = Math.min(window.innerHeight, rect.bottom);
      let ancestor = el.parentElement;
      while (ancestor) {
        const style = getComputedStyle(ancestor);
        const bounds = ancestor.getBoundingClientRect();
        if (/(auto|scroll|hidden|clip)/.test(style.overflowX)) { left = Math.max(left, bounds.left); right = Math.min(right, bounds.right); }
        if (/(auto|scroll|hidden|clip)/.test(style.overflowY)) { top = Math.max(top, bounds.top); bottom = Math.min(bottom, bounds.bottom); }
        ancestor = ancestor.parentElement;
      }
      const clip: [number, number, number, number] = [left - rect.left, top - rect.top, Math.max(0, right - left), Math.max(0, bottom - top)];
      const visible = !document.hidden && clip[2] > 0 && clip[3] > 0;
      const viewport = { windowLabel, x: rect.left, y: rect.top, width: rect.width, height: rect.height,
        backingScale: window.devicePixelRatio || 1, visible, occluded: !visible, generation, clip };
      const key = JSON.stringify(viewport);
      if (key === lastGeometry) return;
      sending = true;
      void api.previewLayout({ ...viewport, revision: ++revision })
        .then(next => { if (!cancelled) { lastGeometry = key; setStatus(next); setError(undefined); } })
        .catch(err => { if (!cancelled) setError(String(err)); })
        .finally(() => { sending = false; });
    };
    void api.previewAttach(windowLabel, hitMode).then(attached => {
      generation = attached.generation;
      if (cancelled) { void api.previewDetach(windowLabel, generation).catch(() => undefined); return; }
      setStatus(attached); setError(undefined);
      update();
    }).catch(err => { if (!cancelled) setError(String(err)); });
    return () => {
      cancelled = true;
      cancelAnimationFrame(animation);
      if (generation !== undefined) void api.previewDetach(windowLabel, generation).catch(() => undefined);
    };
  }, [windowLabel, hitMode]);

  return (
    <div className="w-full flex flex-col items-center gap-2">
      <div
        ref={hostRef}
        data-native-preview-host
        className={`pointer-events-none rounded-xl border border-studio-800 bg-black/50 ${className}`}
      />
      <p className="text-xs text-studio-400 max-w-md text-center">
        {error || playbackError
          ? error || playbackError
          : status?.attached
            ? previewAvailable ? "" : "Loading project preview…"
            : "Native preview surface is not attached."}
      </p>
    </div>
  );
}
