import { describe, expect, it } from "vitest";
import { loadCountdownSeconds, useSettingsStore } from "./settingsStore";

describe("countdown setting", () => {
  it("defaults to 3 seconds and ignores unknown stored values", () => {
    expect(loadCountdownSeconds()).toBe(3);
    localStorage.setItem("aeroshoot.countdownSeconds", "7");
    expect(loadCountdownSeconds()).toBe(3);
  });

  it("remembers the chosen countdown, including Off", () => {
    useSettingsStore.getState().setCountdownSeconds(0);
    expect(useSettingsStore.getState().countdownSeconds).toBe(0);
    expect(loadCountdownSeconds()).toBe(0);
  });
});
