import { describe, expect, it } from "vitest";
import {
  cameraMaximum,
  cameraOptions,
  cameraResolutionSize,
  effectiveCameraQuality,
  DEFAULT_CAMERA_QUALITY,
  DEFAULT_QUALITY,
  loadCameraQuality,
  loadRecordingQuality,
  megabytesPerMinute,
  resolutionSize,
  saveCameraQuality,
  saveRecordingQuality,
} from "./recordingQuality";

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

describe("camera quality preferences", () => {
  it("defaults to 720p at 30 fps and 6 Mbps, separately from the screen", () => {
    expect(DEFAULT_CAMERA_QUALITY).toEqual({ resolution: "720p", fps: 30, bitrateMbps: 6 });
    saveRecordingQuality({ fps: 60, resolution: "4K", videoBitrateMbps: 30 });
    expect(loadCameraQuality()).toEqual(DEFAULT_CAMERA_QUALITY);
  });

  it("round-trips a saved choice and rejects unsupported values", () => {
    saveCameraQuality({ resolution: "1080p", fps: 24, bitrateMbps: 10 });
    expect(loadCameraQuality()).toEqual({ resolution: "1080p", fps: 24, bitrateMbps: 10 });
    localStorage.setItem("aeroshoot.cameraQuality", JSON.stringify({ resolution: "4K", fps: 120, bitrateMbps: 99 }));
    expect(loadCameraQuality()).toEqual(DEFAULT_CAMERA_QUALITY);
  });

  it("maps camera resolutions to even 16:9 encoder sizes", () => {
    expect(cameraResolutionSize("480p")).toEqual({ width: 854, height: 480 });
    expect(cameraResolutionSize("1080p")).toEqual({ width: 1920, height: 1080 });
    expect(megabytesPerMinute(6)).toBe(45);
  });
});

describe("camera options", () => {
  const usbCamera = [
    { width: 640, height: 480, fps: 60 },
    { width: 1280, height: 720, fps: 60 },
    { width: 1920, height: 1080, fps: 30 },
  ];

  it("offers every setting when the camera's modes are unknown", () => {
    expect(cameraOptions(undefined, "1080p")).toEqual({
      resolutions: ["480p", "720p", "1080p"],
      frameRates: [24, 30, 60],
    });
  });

  it("offers only the frame rates the camera delivers at the resolution", () => {
    expect(cameraOptions(usbCamera, "1080p").frameRates).toEqual([24, 30]);
    expect(cameraOptions(usbCamera, "720p").frameRates).toEqual([24, 30, 60]);
  });

  it("never upscales a camera smaller than the choice", () => {
    const vga = [{ width: 640, height: 480, fps: 30 }];
    expect(cameraOptions(vga, "1080p").resolutions).toEqual(["480p"]);
    expect(effectiveCameraQuality(vga, { resolution: "1080p", fps: 60, bitrateMbps: 6 })).toEqual({
      resolution: "480p",
      fps: 30,
      bitrateMbps: 6,
    });
  });

  it("counts 29.97 fps as 30", () => {
    const ntsc = [{ width: 1920, height: 1080, fps: 29 }];
    expect(cameraOptions(ntsc, "1080p").frameRates).toEqual([24, 30]);
  });

  it("describes the camera's best mode", () => {
    expect(cameraMaximum(usbCamera)).toBe("1080p at 30 fps");
    expect(cameraMaximum([])).toBeUndefined();
  });
});
