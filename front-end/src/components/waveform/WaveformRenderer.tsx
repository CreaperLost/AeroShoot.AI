import React, { useEffect, useRef } from "react";
import { WaveformBucket } from "../../lib/types";

interface WaveformRendererProps {
  buckets: WaveformBucket[];
  currentTimeProgress: number; // 0.0 to 1.0
  className?: string;
  barColor?: string;
  activeBarColor?: string;
  gapColor?: string;
}

export const WaveformRenderer: React.FC<WaveformRendererProps> = ({
  buckets,
  currentTimeProgress,
  className = "w-full h-12",
  barColor = "#3f3f46",
  activeBarColor = "#10b981",
  gapColor = "#57534e",
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const rect = canvas.getBoundingClientRect();

    canvas.width = rect.width * dpr;
    canvas.height = rect.height * dpr;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, rect.width, rect.height);

    if (buckets.length === 0) return;

    const totalBars = buckets.length;
    const barWidth = Math.max(1, (rect.width / totalBars) * 0.7);
    const gap = (rect.width - barWidth * totalBars) / Math.max(1, totalBars - 1);
    const centerY = rect.height / 2;

    for (let i = 0; i < totalBars; i++) {
      const bucket = buckets[i];
      const x = i * (barWidth + gap);
      const progress = i / totalBars;
      if (bucket.gap) {
        ctx.fillStyle = gapColor;
        ctx.globalAlpha = 0.35;
        ctx.fillRect(x, 2, Math.max(1, barWidth), rect.height - 4);
        ctx.globalAlpha = 1;
        continue;
      }

      // Zero amplitude is a 1px hairline, not a decorative min-height bar.
      const barHeight = Math.max(1, bucket.peak * (rect.height * 0.85));
      const y = centerY - barHeight / 2;
      ctx.fillStyle = progress <= currentTimeProgress ? activeBarColor : barColor;
      ctx.beginPath();
      ctx.roundRect(x, y, barWidth, barHeight, 1);
      ctx.fill();
    }
  }, [buckets, currentTimeProgress, barColor, activeBarColor, gapColor]);

  return <canvas ref={canvasRef} className={`w-full h-full block ${className}`} />;
};
