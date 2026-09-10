import React, { useRef, useState } from "react";
import { X, Scissors, Check, Sliders, AlertCircle } from "lucide-react";
import { useProjectStore } from "../../stores/projectStore";
import { api } from "../../lib/ipc";
import { OpenedProject, SilenceConfig } from "../../lib/types";

function preferredAudioTrackId(project: OpenedProject | null): string | undefined {
  if (!project) return undefined;
  const mic = project.tracks.find((track) => track.descriptor.trackType === "mic_audio");
  const system = project.tracks.find((track) => track.descriptor.trackType === "system_audio");
  return (mic ?? system)?.descriptor.id;
}

function errorMessage(err: unknown): string {
  if (err instanceof Error && err.message) return err.message;
  if (typeof err === "string" && err.trim()) return err;
  return "Silence detection failed.";
}

export const SilenceModal: React.FC = () => {
  const {
    isSilenceModalOpen,
    setIsSilenceModalOpen,
    activeSilenceBlocks,
    setSilenceBlocks,
    toggleSilenceBlock,
    applySilenceCuts,
    openedProject,
    applyOpenedProject,
    silenceAnalysis,
  } = useProjectStore();

  const [config, setConfig] = useState<SilenceConfig>({
    thresholdDb: -38,
    minDurationMs: 400,
    paddingMs: 50,
  });

  const [isDetecting, setIsDetecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [diagnostics, setDiagnostics] = useState<string[]>([]);
  const detectGeneration = useRef(0);

  if (!isSilenceModalOpen) return null;

  const audioTrackId = preferredAudioTrackId(openedProject);

  const handleRunDetection = async () => {
    if (!openedProject) {
      setError("Open a project to detect silence.");
      return;
    }
    if (!audioTrackId) {
      setError("No microphone or system audio track is available.");
      return;
    }
    const analyzedHandle = openedProject.projectHandle;
    const analyzedRevision = openedProject.revision;
    const generation = ++detectGeneration.current;
    setIsDetecting(true);
    setError(null);
    setDiagnostics([]);
    try {
      const result = await api.detectSilence(analyzedHandle, audioTrackId, config);
      if (generation !== detectGeneration.current) return;
      const current = useProjectStore.getState().openedProject;
      if (
        !current ||
        current.projectHandle !== analyzedHandle ||
        current.revision !== analyzedRevision
      ) {
        return;
      }
      setSilenceBlocks(result.suggestions, {
        projectHandle: analyzedHandle,
        revision: analyzedRevision,
      });
      setDiagnostics(result.diagnostics ?? []);
      if (result.suggestions.length === 0 && (result.diagnostics?.length ?? 0) > 0) {
        setError(result.diagnostics.join(" · "));
      }
    } catch (err) {
      if (generation !== detectGeneration.current) return;
      setSilenceBlocks([]);
      setDiagnostics([]);
      setError(errorMessage(err));
    } finally {
      if (generation === detectGeneration.current) setIsDetecting(false);
    }
  };

  const selectedCount = activeSilenceBlocks.filter((b) => b.selected).length;
  const totalTrimDurationMs = activeSilenceBlocks
    .filter((b) => b.selected)
    .reduce((acc, b) => acc + b.durationMs, 0);

  return (
    <div className="fixed inset-0 z-50 bg-black/70 backdrop-blur-sm flex items-center justify-center p-4 animate-in fade-in duration-150 select-none">
      <div className="w-full max-w-lg bg-studio-900 border border-studio-700 rounded-2xl shadow-2xl overflow-hidden flex flex-col max-h-[85vh]">
        {/* Modal Header */}
        <div className="px-6 py-4 border-b border-studio-800 flex items-center justify-between bg-studio-850">
          <div className="flex items-center space-x-2.5">
            <div className="p-2 rounded-lg bg-emerald-600/20 text-emerald-400 border border-emerald-500/30">
              <Scissors className="w-5 h-5" />
            </div>
            <div>
              <h3 className="text-sm font-semibold text-white">AI Smart Jump Cuts</h3>
              <p className="text-xs text-studio-400">
                DSP silence detection across synchronized tracks
              </p>
            </div>
          </div>

          <button
            onClick={() => setIsSilenceModalOpen(false)}
            className="p-1.5 rounded-lg text-studio-400 hover:text-white hover:bg-studio-700 transition-colors"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Modal Content */}
        <div className="p-6 space-y-6 overflow-y-auto flex-1">
          {/* DSP Configuration Parameters */}
          <div className="space-y-4 bg-studio-850 p-4 rounded-xl border border-studio-800">
            <div className="flex items-center space-x-2 text-xs font-semibold uppercase tracking-wider text-studio-300">
              <Sliders className="w-3.5 h-3.5 text-emerald-400" />
              <span>Detection Sensitivity</span>
            </div>

            {/* Threshold Slider */}
            <div className="space-y-1.5">
              <div className="flex justify-between text-xs">
                <span className="text-studio-400">Silence Threshold</span>
                <span className="font-mono text-emerald-400">{config.thresholdDb} dBFS</span>
              </div>
              <input
                type="range"
                min={-60}
                max={-20}
                value={config.thresholdDb}
                onChange={(e) => setConfig({ ...config, thresholdDb: Number(e.target.value) })}
                className="w-full accent-emerald-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
              />
              <div className="flex justify-between text-[10px] text-studio-500">
                <span>-60 dB (Very Sensitive)</span>
                <span>-20 dB (Aggressive)</span>
              </div>
            </div>

            {/* Min Duration Slider */}
            <div className="space-y-1.5">
              <div className="flex justify-between text-xs">
                <span className="text-studio-400">Min Silence Duration</span>
                <span className="font-mono text-emerald-400">{config.minDurationMs} ms</span>
              </div>
              <input
                type="range"
                min={200}
                max={1500}
                step={50}
                value={config.minDurationMs}
                onChange={(e) => setConfig({ ...config, minDurationMs: Number(e.target.value) })}
                className="w-full accent-emerald-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
              />
            </div>

            {/* Syllable Buffer */}
            <div className="space-y-1.5">
              <div className="flex justify-between text-xs">
                <span className="text-studio-400">Speech Padding Buffer</span>
                <span className="font-mono text-emerald-400">±{config.paddingMs} ms</span>
              </div>
              <input
                type="range"
                min={20}
                max={150}
                step={10}
                value={config.paddingMs}
                onChange={(e) => setConfig({ ...config, paddingMs: Number(e.target.value) })}
                className="w-full accent-emerald-500 h-1.5 bg-studio-800 rounded-lg cursor-pointer"
              />
              <p className="text-[10px] text-studio-400">
                Preserves milliseconds around words to avoid clipping consonants.
              </p>
            </div>

            <button
              onClick={() => void handleRunDetection()}
              disabled={isDetecting || !openedProject || !audioTrackId}
              className="w-full py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white text-xs font-semibold shadow-md transition-all"
            >
              {isDetecting ? "Scanning Audio Waveforms..." : "Scan & Preview Cuts"}
            </button>
          </div>

          {error && (
            <div className="p-4 rounded-xl bg-rose-950/40 border border-rose-600/40 flex items-start space-x-3 text-rose-200 text-xs">
              <AlertCircle className="w-4 h-4 text-rose-400 shrink-0 mt-0.5" />
              <span>{error}</span>
            </div>
          )}

          {diagnostics.length > 0 && !error && (
            <div className="p-4 rounded-xl bg-amber-950/30 border border-amber-600/30 text-amber-200 text-xs space-y-1">
              {diagnostics.map((item) => (
                <p key={item}>{item}</p>
              ))}
            </div>
          )}

          {/* Detected Blocks Preview */}
          {activeSilenceBlocks.length > 0 && (
            <div className="space-y-2">
              <div className="flex items-center justify-between text-xs">
                <span className="font-semibold text-studio-300">
                  Detected Silence Blocks ({activeSilenceBlocks.length})
                </span>
                <span className="text-[11px] font-mono text-studio-400">
                  Total dead air: {(totalTrimDurationMs / 1000).toFixed(1)}s
                </span>
              </div>

              <div className="space-y-1.5 max-h-48 overflow-y-auto">
                {activeSilenceBlocks.map((block) => (
                  <div
                    key={block.id}
                    onClick={() => toggleSilenceBlock(block.id)}
                    className={`flex items-center justify-between p-2.5 rounded-lg border text-xs cursor-pointer transition-colors ${
                      block.selected
                        ? "bg-rose-950/40 border-rose-600/40 text-rose-200"
                        : "bg-studio-850/50 border-studio-800 text-studio-400 opacity-60"
                    }`}
                  >
                    <div className="flex items-center space-x-2.5">
                      <div
                        className={`w-4 h-4 rounded flex items-center justify-center border ${
                          block.selected
                            ? "bg-rose-600 border-rose-500 text-white"
                            : "border-studio-600"
                        }`}
                      >
                        {block.selected && <Check className="w-3 h-3" />}
                      </div>
                      <span className="font-mono">
                        {(block.startUs / 1_000_000).toFixed(2)}s →{" "}
                        {(block.endUs / 1_000_000).toFixed(2)}s
                      </span>
                    </div>

                    <span className="font-mono text-[11px]">
                      -{(block.durationMs / 1000).toFixed(2)}s
                    </span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {activeSilenceBlocks.length === 0 && !isDetecting && !error && (
            <div className="p-4 rounded-xl bg-studio-850/60 border border-studio-800 flex items-center space-x-3 text-studio-400 text-xs">
              <AlertCircle className="w-4 h-4 text-studio-500 shrink-0" />
              <span>
                Click &quot;Scan &amp; Preview Cuts&quot; to analyze the open project&apos;s microphone or system audio.
              </span>
            </div>
          )}
        </div>

        {/* Modal Footer */}
        <div className="px-6 py-4 border-t border-studio-800 bg-studio-850 flex items-center justify-between">
          <button
            onClick={() => setIsSilenceModalOpen(false)}
            className="px-4 py-2 rounded-lg text-xs font-medium text-studio-400 hover:text-white transition-colors"
          >
            Cancel
          </button>

          <button
            disabled={selectedCount === 0 || !openedProject || !silenceAnalysis}
            onClick={() => {
              if (!openedProject || !silenceAnalysis) return;
              if (
                silenceAnalysis.projectHandle !== openedProject.projectHandle ||
                silenceAnalysis.revision !== openedProject.revision
              ) {
                setSilenceBlocks([]);
                setError("Silence suggestions are stale after a later edit. Scan again.");
                return;
              }
              const cuts = activeSilenceBlocks
                .filter((block) => block.selected)
                .map((block) => ({ startUs: block.startUs, endUs: block.endUs }));
              void api
                .projectRippleCuts(
                  silenceAnalysis.projectHandle,
                  silenceAnalysis.revision,
                  cuts,
                )
                .then((next) => {
                  applyOpenedProject(next);
                  applySilenceCuts();
                  setError(null);
                })
                .catch((err) => setError(errorMessage(err)));
            }}
            className="flex items-center space-x-2 px-5 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 text-white text-xs font-semibold shadow-lg transition-all"
          >
            <Scissors className="w-3.5 h-3.5" />
            <span>Apply {selectedCount} Cuts (Ripple Delete)</span>
          </button>
        </div>
      </div>
    </div>
  );
};
