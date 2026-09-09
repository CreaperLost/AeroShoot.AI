import { create } from "zustand";
import { Track, ZoomKeyframe, SilenceBlock, ProjectManifest, OpenedProject, WaveformPage, PlaybackStatus, studioTrackType } from "../lib/types";

interface ProjectStore {
  openedProject: OpenedProject | null;
  playbackGeneration: number;
  playbackError: string | null;
  previewAvailable: boolean;
  applyPlaybackStatus: (status: PlaybackStatus) => void;
  loadOpenedProject: (project: OpenedProject) => void;
  clearProject: () => void;
  manifest: ProjectManifest | null;
  tracks: Track[];
  zoomKeyframes: ZoomKeyframe[];
  currentTimeUs: number;
  durationUs: number;
  isPlaying: boolean;
  activeSilenceBlocks: SilenceBlock[];
  isSilenceModalOpen: boolean;

  // Actions
  setManifest: (manifest: ProjectManifest | null) => void;
  setCurrentTimeUs: (timeUs: number) => void;
  setIsPlaying: (isPlaying: boolean) => void;
  togglePlayPause: () => void;
  addZoomKeyframe: (keyframe: Omit<ZoomKeyframe, "id">) => void;
  removeZoomKeyframe: (id: string) => void;
  updateZoomKeyframe: (id: string, updates: Partial<ZoomKeyframe>) => void;
  setSilenceBlocks: (blocks: SilenceBlock[]) => void;
  toggleSilenceBlock: (id: string) => void;
  applySilenceCuts: () => void;
  setIsSilenceModalOpen: (open: boolean) => void;
  setTrackWaveform: (trackId: string, waveform: WaveformPage) => void;
  applyOpenedProject: (project: OpenedProject) => void;
}

export const useProjectStore = create<ProjectStore>((set, get) => ({
  openedProject: null,
  playbackGeneration: 0,
  playbackError: null,
  previewAvailable: false,
  applyPlaybackStatus: (status) => set((state) => {
    if (state.openedProject?.projectHandle !== status.projectHandle || status.generation < state.playbackGeneration) return state;
    return { currentTimeUs: Math.min(status.positionUs, state.durationUs), isPlaying: status.state === "playing", playbackGeneration: status.generation, playbackError: status.error, previewAvailable: status.previewAvailable };
  }),
  loadOpenedProject: (project) =>
    set({
      openedProject: project,
      playbackGeneration: 0, playbackError: null, previewAvailable: false,
      manifest: project.manifest,
      durationUs: project.editedDurationUs,
      currentTimeUs: 0,
      isPlaying: false,
      zoomKeyframes: [],
      activeSilenceBlocks: [],
      isSilenceModalOpen: false,
      tracks: project.tracks.map(({ descriptor, segmentCount, availableSegmentCount }) => ({
        id: descriptor.id,
        trackType: studioTrackType(descriptor.trackType),
        name: `${descriptor.id} · ${availableSegmentCount}/${segmentCount} segments available`,
        muted: false,
        volume: 1,
        intervals: [],
      })),
    }),
  clearProject: () =>
    set({
      openedProject: null, playbackError: null, previewAvailable: false,
      manifest: null,
      tracks: [],
      durationUs: 0,
      currentTimeUs: 0,
      isPlaying: false,
      zoomKeyframes: [],
      activeSilenceBlocks: [],
      isSilenceModalOpen: false,
    }),
  manifest: null,
  tracks: [],
  zoomKeyframes: [],
  currentTimeUs: 0,
  durationUs: 0,
  isPlaying: false,
  activeSilenceBlocks: [],
  isSilenceModalOpen: false,

  setManifest: (manifest) =>
    set({
      manifest,
      durationUs: manifest ? manifest.durationUs : 0,
    }),

  setCurrentTimeUs: (timeUs) => {
    const { durationUs } = get();
    const clamped = Math.max(0, Math.min(timeUs, durationUs));
    set({ currentTimeUs: clamped });
  },

  setIsPlaying: (isPlaying) => set({ isPlaying }),

  togglePlayPause: () => set((state) => ({ isPlaying: !state.isPlaying })),

  addZoomKeyframe: (keyframe) => {
    const newK: ZoomKeyframe = {
      ...keyframe,
      id: "zk-" + Math.random().toString(36).substring(2, 9),
    };
    set((state) => ({
      zoomKeyframes: [...state.zoomKeyframes, newK].sort((a, b) => a.tUs - b.tUs),
    }));
  },

  removeZoomKeyframe: (id) =>
    set((state) => ({
      zoomKeyframes: state.zoomKeyframes.filter((k) => k.id !== id),
    })),

  updateZoomKeyframe: (id, updates) =>
    set((state) => ({
      zoomKeyframes: state.zoomKeyframes.map((k) => (k.id === id ? { ...k, ...updates } : k)),
    })),

  setSilenceBlocks: (blocks) => set({ activeSilenceBlocks: blocks }),

  toggleSilenceBlock: (id) =>
    set((state) => ({
      activeSilenceBlocks: state.activeSilenceBlocks.map((b) =>
        b.id === id ? { ...b, selected: !b.selected } : b
      ),
    })),

  applySilenceCuts: () => {
    const { activeSilenceBlocks } = get();
    const selectedBlocks = activeSilenceBlocks.filter((b) => b.selected);
    if (selectedBlocks.length === 0) return;
    set({
      activeSilenceBlocks: [],
      isSilenceModalOpen: false,
    });
  },

  setIsSilenceModalOpen: (open) => set({ isSilenceModalOpen: open }),

  setTrackWaveform: (trackId, waveform) =>
    set((state) => ({
      tracks: state.tracks.map((track) =>
        track.id === trackId ? { ...track, waveform } : track
      ),
    })),

  applyOpenedProject: (project) =>
    set((state) => state.openedProject?.projectHandle !== project.projectHandle ? state : ({
      openedProject: project,
      manifest: project.manifest,
      durationUs: project.editedDurationUs,
      currentTimeUs: Math.min(state.currentTimeUs, project.editedDurationUs),
      tracks: project.tracks.map(({ descriptor, segmentCount, availableSegmentCount }) => {
        const existing = state.tracks.find((track) => track.id === descriptor.id);
        return {
          id: descriptor.id,
          trackType: studioTrackType(descriptor.trackType),
          name: `${descriptor.id} · ${availableSegmentCount}/${segmentCount} segments available`,
          muted: existing?.muted ?? false,
          volume: existing?.volume ?? 1,
          intervals: existing?.intervals ?? [],
          waveform: existing?.waveform,
        };
      }),
    })),
}));
