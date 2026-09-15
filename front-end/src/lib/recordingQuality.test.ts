import { describe, expect, it } from "vitest";
import { DEFAULT_QUALITY, loadRecordingQuality, resolutionSize, saveRecordingQuality } from "./recordingQuality";

const KEY = "aeroshoot.recordingQuality";

describe("recording quality preferences", () => {
  it("defaults to 1080p at 30 fps and 20 Mbps", () => {
    expect(DEFAULT_QUALITY).toEqual({ fps: 30, resolution: "1080p", videoBitrateMbps: 20 });
    expect(loadRecordingQuality()).toEqual(DEFAULT_QUALITY);
  });

  it("round-trips a saved choice", () => {
    saveRecordingQuality({ fps: 60, resolution: "1440p", videoBitrateMbps: 30 });
    expect(loadRecordingQuality()).toEqual({ fps: 60, resolution: "1440p", videoBitrateMbps: 30 });
  });

  it("falls back for unsupported values, including a saved Auto bitrate", () => {
    localStorage.setItem(KEY, JSON.stringify({ fps: 50, resolution: "8K", videoBitrateMbps: null }));
    expect(loadRecordingQuality()).toEqual(DEFAULT_QUALITY);
  });

  it("falls back when the stored value is not JSON", () => {
    localStorage.setItem(KEY, "{not json");
    expect(loadRecordingQuality()).toEqual(DEFAULT_QUALITY);
  });

  it("maps resolutions to encoder sizes", () => {
    expect(resolutionSize("720p")).toEqual({ width: 1280, height: 720 });
    expect(resolutionSize("1440p")).toEqual({ width: 2560, height: 1440 });
    expect(resolutionSize("4K")).toEqual({ width: 3840, height: 2160 });
  });
});
