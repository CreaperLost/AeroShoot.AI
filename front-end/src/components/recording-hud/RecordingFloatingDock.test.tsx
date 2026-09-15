import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { RecordingFloatingDock } from "./RecordingFloatingDock";

const handlers = () => ({ onStart: vi.fn(), onPause: vi.fn(), onResume: vi.fn(), onStop: vi.fn() });

describe("RecordingFloatingDock", () => {
  it("disables Record and explains why when recording cannot start", () => {
    render(
      <RecordingFloatingDock
        sessionState="idle"
        elapsedMs={0}
        canStart={false}
        disabledReason="Turn on at least one source with permission to record."
        {...handlers()}
      />,
    );
    const record = screen.getByRole("button", { name: /record/i }) as HTMLButtonElement;
    expect(record.disabled).toBe(true);
    expect(screen.getByText("Turn on at least one source with permission to record.")).toBeTruthy();
  });

  it("shows the timer, Pause and a clickable Stop while recording", () => {
    const actions = handlers();
    render(<RecordingFloatingDock sessionState="recording" elapsedMs={65_430} canStart {...actions} />);
    expect(screen.getByText("01:05.43")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Pause recording" }));
    fireEvent.click(screen.getByRole("button", { name: /stop & edit/i }));
    expect(actions.onPause).toHaveBeenCalledTimes(1);
    expect(actions.onStop).toHaveBeenCalledTimes(1);
  });

  it("offers Resume while paused", () => {
    const actions = handlers();
    render(<RecordingFloatingDock sessionState="paused" elapsedMs={1_000} canStart {...actions} />);
    fireEvent.click(screen.getByRole("button", { name: "Resume recording" }));
    expect(actions.onResume).toHaveBeenCalledTimes(1);
  });

  it("offers Retry Stop for a session that failed but still owns its project", () => {
    const actions = handlers();
    render(<RecordingFloatingDock sessionState="error" elapsedMs={0} canStart={false} sessionOwned {...actions} />);
    fireEvent.click(screen.getByRole("button", { name: /retry stop/i }));
    expect(actions.onStop).toHaveBeenCalledTimes(1);
  });
});
