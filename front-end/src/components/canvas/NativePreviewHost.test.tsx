import { render, waitFor } from "@testing-library/react";
import { StrictMode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  studioPreviewAttach: vi.fn(),
  studioPreviewStatus: vi.fn(),
  studioPreviewLayout: vi.fn(),
  studioPreviewDetach: vi.fn(),
}));
vi.mock("../../lib/ipc", () => ({ api: ipc }));

import { NativePreviewHost } from "./NativePreviewHost";

const status = (generation: number) => ({ attached: true, generation, layoutRevision: 0 });

describe("NativePreviewHost", () => {
  beforeEach(() => {
    // jsdom has no layout observers.
    vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
    ipc.studioPreviewAttach.mockReset();
    ipc.studioPreviewStatus.mockReset();
    ipc.studioPreviewLayout.mockReset().mockResolvedValue(status(2));
    ipc.studioPreviewDetach.mockReset().mockResolvedValue(status(3));
  });

  it("a stale StrictMode mount does not detach the surface its successor attached", async () => {
    // Attach replies arrive only after StrictMode has remounted the host.
    const replies: Array<() => void> = [];
    ipc.studioPreviewAttach.mockImplementation(() => new Promise((resolve) => replies.push(() => resolve(undefined))));
    let generation = 0;
    ipc.studioPreviewStatus.mockImplementation(() => Promise.resolve(status(++generation)));

    render(
      <StrictMode>
        <NativePreviewHost />
      </StrictMode>,
    );
    await waitFor(() => expect(replies).toHaveLength(2));
    replies.forEach((reply) => reply());
    await waitFor(() => expect(ipc.studioPreviewStatus).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(ipc.studioPreviewDetach).not.toHaveBeenCalled();
  });

  it("detaches when the host really unmounts", async () => {
    ipc.studioPreviewAttach.mockResolvedValue(undefined);
    ipc.studioPreviewStatus.mockResolvedValue(status(1));
    const { unmount } = render(<NativePreviewHost />);
    await waitFor(() => expect(ipc.studioPreviewStatus).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    unmount();
    expect(ipc.studioPreviewDetach).toHaveBeenCalledTimes(1);
  });
});
