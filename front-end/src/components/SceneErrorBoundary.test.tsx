import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({ stopRecording: vi.fn() }));
vi.mock("../lib/ipc", () => ({ api: ipc }));

import { SceneErrorBoundary } from "./SceneErrorBoundary";

function Crash(): null {
  throw new Error("Cannot read properties of null (reading 'toFixed')");
}

describe("SceneErrorBoundary", () => {
  it("shows the error and a working Stop instead of a blank window", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    ipc.stopRecording.mockResolvedValue({ projectPath: "/tmp/Untitled.aero" });
    render(
      <SceneErrorBoundary>
        <Crash />
      </SceneErrorBoundary>,
    );
    expect(screen.getByText("The recorder view crashed.")).toBeTruthy();
    expect(screen.getByText("Cannot read properties of null (reading 'toFixed')")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Stop recording" }));
    expect(await screen.findByText("Saved to /tmp/Untitled.aero")).toBeTruthy();
    expect(ipc.stopRecording).toHaveBeenCalledTimes(1);
  });
});
