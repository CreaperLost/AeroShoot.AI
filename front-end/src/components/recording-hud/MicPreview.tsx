import React, { useEffect, useRef, useState } from "react";

/**
 * Renders a live waveform for the currently selected microphone by opening
 * a short-lived `getUserMedia` probe and piping it through an AnalyserNode.
 *
 * The signal is also routed through a GainNode so the user can preview
 * exactly the loudness the slider will produce on the recorded track. The
 * gain node is connected to a `gain = 0` final node, not to
 * `audioContext.destination` — there is deliberately no audible output so
 * the user does not get feedback while they fiddle with the slider.
 *
 * The component is intentionally read-only: it does not own the gain
 * value, it only consumes it. The owning settings store decides what the
 * slider should be.
 */
export interface MicPreviewProps {
  /** `deviceId` returned by `enumerateDevices` for an `audioinput`. */
  deviceId: string | null;
  /** Current slider value in decibels. 0 dB = unity. Clamped to ±24 dB. */
  gainDb: number;
  /** Canvas width in CSS pixels. Defaults to a compact 36px. */
  width?: number;
  /** Canvas height in CSS pixels. Defaults to a compact 14px. */
  height?: number;
  /** Extra classes for the wrapping container. */
  className?: string;
  /** Called whenever a fresh peak (in dBFS, post-gain) is measured. */
  onPeakDbChange?: (peakDb: number) => void;
}

const CLAMP_MIN_DB = -24;
const CLAMP_MAX_DB = 24;
const RENDER_FPS = 30;
const ANALYSER_FFT = 1024;
const RENDER_INTERVAL_MS = 1000 / RENDER_FPS;

export const MicPreview: React.FC<MicPreviewProps> = ({
  deviceId,
  gainDb,
  width = 36,
  height = 14,
  className,
  onPeakDbChange,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const gainRef = useRef<GainNode | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const rafRef = useRef<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState(false);

  const clampedGain = Math.max(CLAMP_MIN_DB, Math.min(CLAMP_MAX_DB, gainDb));
  const linearGain = Math.pow(10, clampedGain / 20);

  // Open / close the probe stream when the device changes.
  useEffect(() => {
    let cancelled = false;

    if (!deviceId) {
      teardown();
      setActive(false);
      return () => {};
    }

    setError(null);
    openProbe(deviceId).then(
      () => {
        if (cancelled) return;
        setActive(true);
        startRenderLoop();
      },
      (err: unknown) => {
        if (cancelled) return;
        const message =
          err instanceof Error ? err.message : "Failed to access microphone";
        setError(message);
        setActive(false);
      },
    );

    return () => {
      cancelled = true;
      teardown();
      setActive(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceId]);

  // Reflect the latest gain on the monitor node. We use `setTargetAtTime`
  // so the slider feels smooth instead of zipping; the captured audio
  // applies the same gain on the native side, but that path is instant.
  useEffect(() => {
    if (!gainRef.current) return;
    const ctx = audioContextRef.current;
    if (!ctx) return;
    gainRef.current.gain.setTargetAtTime(
      linearGain,
      ctx.currentTime,
      0.02,
    );
  }, [linearGain]);

  function teardown() {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    try {
      sourceRef.current?.disconnect();
    } catch {
      /* noop */
    }
    try {
      gainRef.current?.disconnect();
    } catch {
      /* noop */
    }
    try {
      analyserRef.current?.disconnect();
    } catch {
      /* noop */
    }
    sourceRef.current = null;
    gainRef.current = null;
    analyserRef.current = null;
    if (streamRef.current) {
      streamRef.current.getTracks().forEach((track) => track.stop());
      streamRef.current = null;
    }
    if (audioContextRef.current && audioContextRef.current.state !== "closed") {
      void audioContextRef.current.close();
    }
    audioContextRef.current = null;
  }

  async function openProbe(id: string) {
    const supportsGetUserMedia =
      typeof navigator !== "undefined" &&
      typeof navigator.mediaDevices?.getUserMedia === "function";
    if (!supportsGetUserMedia) {
      throw new Error("Audio preview is only available in a browser context.");
    }
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: { exact: id },
        // We want the raw mic so the user can see what is actually coming
        // in. The browser-side echo/noise suppression would lie about
        // amplitude and make the waveform useless for level-setting.
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
      },
      video: false,
    });
    const AudioCtor: typeof AudioContext =
      (window as unknown as { webkitAudioContext?: typeof AudioContext })
        .webkitAudioContext ?? window.AudioContext;
    const ctx = new AudioCtor();
    const source = ctx.createMediaStreamSource(stream);
    const monitorGain = ctx.createGain();
    monitorGain.gain.value = linearGain;
    const analyser = ctx.createAnalyser();
    analyser.fftSize = ANALYSER_FFT;
    analyser.smoothingTimeConstant = 0.2;

    source.connect(monitorGain);
    monitorGain.connect(analyser);
    // Deliberately do NOT connect to ctx.destination — no audible monitor.

    streamRef.current = stream;
    audioContextRef.current = ctx;
    sourceRef.current = source;
    gainRef.current = monitorGain;
    analyserRef.current = analyser;
  }

  function startRenderLoop() {
    const canvas = canvasRef.current;
    const analyser = analyserRef.current;
    if (!canvas || !analyser) return;
    const ctx2d = canvas.getContext("2d");
    if (!ctx2d) return;

    // Match the backing-store pixel density so the wave stays crisp on
    // HiDPI displays.
    const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    const targetW = Math.round(width * dpr);
    const targetH = Math.round(height * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }
    ctx2d.scale(dpr, dpr);

    const buffer = new Uint8Array(analyser.fftSize);
    let lastFrame = 0;

    const draw = (now: number) => {
      rafRef.current = requestAnimationFrame(draw);
      if (now - lastFrame < RENDER_INTERVAL_MS) return;
      lastFrame = now;

      analyser.getByteTimeDomainData(buffer);

      // Background
      ctx2d.clearRect(0, 0, width, height);
      ctx2d.fillStyle = "rgba(15, 23, 42, 0.85)"; // studio-950 with alpha
      ctx2d.fillRect(0, 0, width, height);

      // Centre line
      ctx2d.strokeStyle = "rgba(148, 163, 184, 0.25)"; // studio-400
      ctx2d.lineWidth = 1;
      ctx2d.beginPath();
      ctx2d.moveTo(0, height / 2);
      ctx2d.lineTo(width, height / 2);
      ctx2d.stroke();

      // Waveform
      ctx2d.strokeStyle = active ? "#fbbf24" : "#94a3b8"; // amber-400 / studio-400
      ctx2d.lineWidth = 1;
      ctx2d.beginPath();
      const step = buffer.length / width;
      for (let x = 0; x < width; x += 1) {
        const sampleIndex = Math.min(buffer.length - 1, Math.floor(x * step));
        // 128 = silence, 0 / 255 = extremes.
        const v = (buffer[sampleIndex] - 128) / 128;
        const y = height / 2 - v * (height / 2 - 1);
        if (x === 0) ctx2d.moveTo(x + 0.5, y);
        else ctx2d.lineTo(x + 0.5, y);
      }
      ctx2d.stroke();

      // Peak (used by the dropdown read-out).
      let peak = 0;
      for (let i = 0; i < buffer.length; i += 1) {
        const v = Math.abs((buffer[i] - 128) / 128);
        if (v > peak) peak = v;
      }
      const peakDb = peak > 0 ? 20 * Math.log10(peak) : -Infinity;
      if (onPeakDbChange) onPeakDbChange(peakDb);
    };
    rafRef.current = requestAnimationFrame(draw);
  }

  return (
    <div
      className={className}
      title={
        error
          ? `Mic preview unavailable: ${error}`
          : active
            ? `Mic preview active (${clampedGain >= 0 ? "+" : ""}${clampedGain} dB)`
            : "Mic preview idle"
      }
    >
      <canvas
        ref={canvasRef}
        style={{ width: `${width}px`, height: `${height}px`, display: "block" }}
        className="rounded border border-studio-700/60 bg-studio-950/80"
      />
    </div>
  );
};

export const MIC_GAIN_RANGE = { min: CLAMP_MIN_DB, max: CLAMP_MAX_DB } as const;
