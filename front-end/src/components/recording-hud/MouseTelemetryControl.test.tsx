import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  mouseTelemetryPermission: vi.fn(),
  openSystemPrivacySettings: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ({ api: ipc }));

import { useSettingsStore } from "../../stores/settingsStore";
import { MouseTelemetryControl } from "./MouseTelemetryControl";

describe("MouseTelemetryControl", () => {
  beforeEach(() => {
    ipc.mouseTelemetryPermission.mockReset();
    ipc.openSystemPrivacySettings.mockReset().mockResolvedValue({ opened: true });
    useSettingsStore.setState({ captureMouse: true });
  });

  it("offers to allow Input Monitoring when tracking is on without permission", async () => {
    ipc.mouseTelemetryPermission.mockResolvedValue({ supported: true, authorized: false });
    render(<MouseTelemetryControl disabled={false} />);
    fireEvent.click(await screen.findByRole("button", { name: "Allow Input Monitoring" }));
    await waitFor(() => expect(ipc.openSystemPrivacySettings).toHaveBeenCalledWith("InputMonitoring"));
    expect(ipc.mouseTelemetryPermission).toHaveBeenCalledWith(true);
  });

  it("reports logging once permission is granted", async () => {
    ipc.mouseTelemetryPermission.mockResolvedValue({ supported: true, authorized: true });
    render(<MouseTelemetryControl disabled={false} />);
    expect(await screen.findByText("Records the pointer separately so the editor can redraw it smoothly.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Allow Input Monitoring" })).toBeNull();
  });

  it("turning tracking off is saved and removes the permission prompt", async () => {
    ipc.mouseTelemetryPermission.mockResolvedValue({ supported: true, authorized: false });
    render(<MouseTelemetryControl disabled={false} />);
    await screen.findByRole("button", { name: "Allow Input Monitoring" });
    const toggle = screen.getByRole("switch", { name: "Track mouse for editing" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    fireEvent.click(toggle);
    expect(useSettingsStore.getState().captureMouse).toBe(false);
    expect(localStorage.getItem("aeroshoot.captureMouse")).toBe("false");
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(screen.getByText("The cursor stays in the video as recorded.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Allow Input Monitoring" })).toBeNull();
  });
});
