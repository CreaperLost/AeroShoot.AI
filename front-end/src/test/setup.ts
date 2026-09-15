import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

// Recent Node versions define their own global `localStorage`, which is
// unusable without `--localstorage-file` and shadows jsdom's. Use jsdom's.
const dom = (globalThis as { jsdom?: { window: Window } }).jsdom;
if (dom) {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: dom.window.localStorage,
  });
}

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.restoreAllMocks();
});
