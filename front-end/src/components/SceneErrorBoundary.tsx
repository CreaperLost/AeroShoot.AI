import React from "react";
import { api } from "../lib/ipc";

interface State {
  error?: Error;
  stopping: boolean;
  stopMessage?: string;
}

/**
 * Without a boundary, any render error unmounts the whole studio and leaves an
 * empty black window — including while a recording is running. Show the error
 * and keep Stop reachable so the native session can always be finalized.
 */
export class SceneErrorBoundary extends React.Component<{ children: React.ReactNode }, State> {
  state: State = { stopping: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("[SceneErrorBoundary]", error, info.componentStack);
  }

  private stop = async () => {
    this.setState({ stopping: true, stopMessage: undefined });
    try {
      const result = await api.stopRecording();
      this.setState({ stopMessage: result.projectPath ? `Saved to ${result.projectPath}` : "Recording stopped." });
    } catch (err) {
      this.setState({ stopMessage: String(err) });
    } finally {
      this.setState({ stopping: false });
    }
  };

  render() {
    const { error, stopping, stopMessage } = this.state;
    if (!error) return this.props.children;
    return (
      <div role="alert" className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
        <p className="text-sm font-semibold text-rose-300">The recorder view crashed.</p>
        <p className="max-w-xl break-words font-mono text-xs text-studio-400">{error.message}</p>
        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => void this.stop()}
            disabled={stopping}
            className="rounded-xl bg-rose-600 px-4 py-2 text-sm font-semibold text-white hover:bg-rose-500 disabled:opacity-50"
          >
            {stopping ? "Stopping…" : "Stop recording"}
          </button>
          <button
            type="button"
            onClick={() => this.setState({ error: undefined, stopMessage: undefined })}
            className="rounded-xl border border-studio-700 bg-studio-800 px-4 py-2 text-sm text-studio-200 hover:bg-studio-700"
          >
            Try again
          </button>
        </div>
        {stopMessage && <p className="max-w-xl break-words text-xs text-studio-300">{stopMessage}</p>}
      </div>
    );
  }
}
