import { useEffect, useRef, useCallback } from "react";
import { useProjectStore } from "../stores/projectStore";
import { api } from "../lib/ipc";


export function formatTimeUs(timeUs: number): string {
  const totalSeconds = timeUs / 1_000_000;
  const mins = Math.floor(totalSeconds / 60);
  const secs = Math.floor(totalSeconds % 60);
  const ms = Math.floor((totalSeconds % 1) * 100);

  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(mins)}:${pad(secs)}.${pad(ms)}`;
}

export function useTimeline() {
  const {
    openedProject,
    currentTimeUs,
    durationUs,
    isPlaying,
    setCurrentTimeUs,
    setIsPlaying,
    applyPlaybackStatus,
  } = useProjectStore();

  const handle = openedProject?.projectHandle;

  useEffect(() => {
    if (!handle) {
      setIsPlaying(false);
      return;
    }
    let cancelled = false;
    const poll = async () => {
      try {
        const status = await api.playbackStatus(handle);
        if (!cancelled) applyPlaybackStatus(status);
      } catch {
        if (!cancelled) setIsPlaying(false);
      }
    };
    void poll();
    const timer = window.setInterval(() => {
      void poll();
    }, 50);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [handle, setCurrentTimeUs, setIsPlaying, applyPlaybackStatus]);

  const interpolationRef = useRef<number | null>(null);
  useEffect(() => {
    if (interpolationRef.current) {
      cancelAnimationFrame(interpolationRef.current);
      interpolationRef.current = null;
    }
    // Display smoothing only. Rust playback_status is the media clock.
  }, [isPlaying, currentTimeUs]);

  const togglePlayPause = useCallback(() => {
    if (!handle) return;
    const action = isPlaying ? api.playbackPause(handle) : api.playbackPlay(handle);
    void action
      .then((status) => applyPlaybackStatus(status))
      .catch((err) => console.warn("[Timeline] playback toggle failed:", err));
  }, [handle, isPlaying, setCurrentTimeUs, setIsPlaying, applyPlaybackStatus]);

  const seekToUs = useCallback(
    (timeUs: number) => {
      if (!handle) {
        setCurrentTimeUs(Math.max(0, Math.min(timeUs, durationUs)));
        return;
      }
      void api
        .playbackSeek(handle, Math.max(0, Math.min(timeUs, durationUs)))
        .then((status) => applyPlaybackStatus(status))
        .catch((err) => console.warn("[Timeline] seek failed:", err));
    },
    [handle, durationUs, setCurrentTimeUs, setIsPlaying, applyPlaybackStatus],
  );

  const seekRelativeUs = useCallback(
    (offsetUs: number) => {
      seekToUs(currentTimeUs + offsetUs);
    },
    [currentTimeUs, seekToUs],
  );

  return {
    currentTimeUs,
    durationUs,
    isPlaying,
    togglePlayPause,
    seekToUs,
    seekRelativeUs,
    formattedTime: formatTimeUs(currentTimeUs),
    formattedDuration: formatTimeUs(durationUs),
  };
}
