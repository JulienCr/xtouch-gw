import { writable, get } from 'svelte/store';
import {
  api,
  type ObsScene,
  type ObsSource,
  type ObsInput,
  type DriverDescriptor,
  type ActionDescriptor
} from '$lib/api';

export interface PickerState {
  obsConnected: boolean;
  obsScenes: ObsScene[];
  obsSources: Record<string, ObsSource[]>;
  obsInputs: ObsInput[];
  midi: { inputs: string[]; outputs: string[] };
  drivers: DriverDescriptor[];
  driverActions: Record<string, ActionDescriptor[]>;
  loading: boolean;
  error: string | null;
}

const initial: PickerState = {
  obsConnected: false,
  obsScenes: [],
  obsSources: {},
  obsInputs: [],
  midi: { inputs: [], outputs: [] },
  drivers: [],
  driverActions: {},
  loading: false,
  error: null
};

export const pickers = writable<PickerState>(initial);

export const pickerActions = {
  async refresh(): Promise<void> {
    pickers.update((p) => ({ ...p, loading: true, error: null }));
    try {
      const [scenesRes, inputsRes, midi, drivers] = await Promise.all([
        api.obs.scenes().catch(() => ({ connected: false, scenes: [] })),
        api.obs.inputs().catch(() => ({ connected: false, inputs: [] })),
        api.midi.ports().catch(() => ({ inputs: [], outputs: [] })),
        api.drivers.list().catch(() => [] as DriverDescriptor[])
      ]);
      pickers.update((p) => ({
        ...p,
        obsConnected: scenesRes.connected || inputsRes.connected,
        obsScenes: scenesRes.scenes,
        obsInputs: inputsRes.inputs,
        midi,
        drivers,
        loading: false
      }));
    } catch (e) {
      pickers.update((p) => ({
        ...p,
        loading: false,
        error: e instanceof Error ? e.message : String(e)
      }));
    }
  },

  async sourcesFor(scene: string): Promise<ObsSource[]> {
    if (!scene) return [];
    const cur = get(pickers);
    if (cur.obsSources[scene]) return cur.obsSources[scene];
    try {
      const { sources, connected } = await api.obs.sources(scene);
      pickers.update((p) => ({
        ...p,
        obsConnected: p.obsConnected || connected,
        obsSources: { ...p.obsSources, [scene]: sources }
      }));
      return sources;
    } catch {
      return [];
    }
  },

  async actionsFor(driver: string): Promise<ActionDescriptor[]> {
    if (!driver) return [];
    const cur = get(pickers);
    if (cur.driverActions[driver]) return cur.driverActions[driver];
    const actions = await api.drivers.actions(driver);
    pickers.update((p) => ({
      ...p,
      driverActions: { ...p.driverActions, [driver]: actions }
    }));
    return actions;
  }
};
