export type HostPlatform = "macos" | "windows" | "linux";

/**
 * Host OS of the desktop webview. WebView2 always reports "Windows NT";
 * anything else that is not Linux is treated as the macOS WKWebView, which
 * keeps browser-mode development and tests on the macOS wording.
 */
export function detectPlatform(userAgent: string): HostPlatform {
  if (/Windows NT/i.test(userAgent)) return "windows";
  if (/Linux/i.test(userAgent) && !/Android/i.test(userAgent)) return "linux";
  return "macos";
}

export interface PlatformText {
  /** Name of the file browser, e.g. "Show in Finder". */
  fileManager: string;
  /** Name of the OS settings app that holds privacy toggles. */
  systemSettings: string;
}

const TEXT: Record<HostPlatform, PlatformText> = {
  macos: { fileManager: "Finder", systemSettings: "macOS Settings" },
  windows: { fileManager: "File Explorer", systemSettings: "Windows Settings" },
  linux: { fileManager: "Files", systemSettings: "System Settings" },
};

export function platformText(platform: HostPlatform): PlatformText {
  return TEXT[platform];
}

export const hostPlatform: HostPlatform = detectPlatform(
  typeof navigator === "undefined" ? "" : navigator.userAgent,
);

export const hostText: PlatformText = platformText(hostPlatform);
