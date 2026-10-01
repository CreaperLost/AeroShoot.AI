import { describe, expect, it } from "vitest";
import { detectPlatform, platformText } from "./platform";

describe("detectPlatform", () => {
  it("recognizes the WebView2 user agent as Windows", () => {
    expect(
      detectPlatform(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
      ),
    ).toBe("windows");
  });

  it("recognizes the WKWebView user agent as macOS", () => {
    expect(
      detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"),
    ).toBe("macos");
  });

  it("recognizes WebKitGTK on Linux", () => {
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)")).toBe("linux");
  });

  it("keeps jsdom on the macOS wording on every host", () => {
    expect(detectPlatform("Mozilla/5.0 (win32) AppleWebKit/537.36 (KHTML, like Gecko) jsdom/26.1.0")).toBe("macos");
    expect(detectPlatform("")).toBe("macos");
  });
});

describe("platformText", () => {
  it("keeps the existing macOS wording", () => {
    expect(platformText("macos")).toEqual({ fileManager: "Finder", systemSettings: "macOS Settings" });
  });

  it("uses Windows names on Windows", () => {
    expect(platformText("windows")).toEqual({ fileManager: "File Explorer", systemSettings: "Windows Settings" });
  });
});
