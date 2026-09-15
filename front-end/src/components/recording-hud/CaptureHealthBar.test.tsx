import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { CaptureHealth } from "../../hooks/useRecording";
import { CaptureHealthBar } from "./CaptureHealthBar";

const idle: CaptureHealth = {
  screenSamples: 0,
  cameraSamples: 0,
  systemAudioSamples: 0,
  micSamples: 0,
  droppedFrames: 0,
  audioBufferUnderflows: 0,
  screenSegments: 0,
  cameraSegments: 0,
  systemAudioSegments: 0,
  micSegments: 0,
};

describe("CaptureHealthBar", () => {
  it("renders when the backend reports missing audio peaks as null", () => {
    // Rust serializes absent peaks as `null`; calling toFixed on it used to
    // unmount the whole studio and leave a black window.
    const health = { ...idle, systemAudioPeakDb: null, micPeakDb: null } as unknown as CaptureHealth;
    render(<CaptureHealthBar health={health} screenEnabled cameraEnabled={false} systemAudioEnabled micEnabled />);
    expect(screen.getByLabelText("Live capture health")).toBeTruthy();
    expect(screen.getAllByLabelText("No level")).toHaveLength(2);
  });

  it("shows saved segments for a receiving source and marks disabled sources off", () => {
    render(
      <CaptureHealthBar
        health={{ ...idle, screenSamples: 120, screenSegments: 2, screenLastSampleAgeMs: 100 }}
        screenEnabled
        cameraEnabled={false}
        systemAudioEnabled={false}
        micEnabled={false}
      />,
    );
    expect(screen.getByText("2 saved")).toBeTruthy();
    expect(screen.getAllByText("off")).toHaveLength(3);
  });
});
