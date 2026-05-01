<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import { onceCapture } from '$lib/stores/live';
  import type { LiveEvent } from '$lib/api';

  export let kind: 'control' | 'axis' | 'button' | 'any' | 'midi-in' = 'any';
  export let label = 'Capture';
  /** For kind="midi-in": the app whose MIDI input port we should listen on. */
  export let appName: string = '';

  type MidiCaptured = { type: 'cc' | 'note'; channel: number; cc?: number; note?: number };

  // Events:
  //   captured       — payload is the captured control_id string (kind != midi-in)
  //   capturedMidi   — payload is the MIDI descriptor (kind = midi-in)
  const dispatch = createEventDispatcher<{ captured: string; capturedMidi: MidiCaptured }>();

  let listening = false;
  let unsub: (() => void) | null = null;

  function matches(ev: LiveEvent): boolean {
    if (ev.kind !== 'hw_event' || !ev.control_id) return false;
    if (kind === 'axis') {
      if (!ev.control_id.includes('.axis.')) return false;
      const v = typeof ev.value === 'number' ? Math.abs(ev.value) : 0;
      return v > 0.5;
    }
    if (kind === 'button') {
      if (ev.control_id.includes('.axis.')) return false;
      return (ev.value ?? 0) > 0;
    }
    return true;
  }

  // Best-effort MIDI-input capture: the live socket currently surfaces
  // hardware events as `hw_event` with a `control_id` (e.g. "qlc.cc.1.77",
  // "obs.note.1.42"). We try to pattern-match those when the appName is known.
  // TODO: replace with a dedicated `midi_in` event kind from the backend that
  //       carries raw {app, type, channel, cc/note, value} fields. The backend
  //       does not currently emit such events for app input ports.
  function tryParseMidi(ev: LiveEvent): MidiCaptured | null {
    if (ev.kind !== 'hw_event' || !ev.control_id) return null;
    const id = ev.control_id;
    // Heuristic patterns: "<app>.cc.<ch>.<cc>" or "<app>.note.<ch>.<note>"
    const parts = id.split('.');
    if (parts.length < 4) return null;
    const [app, t, chStr, numStr] = parts;
    if (appName && app !== appName) return null;
    const ch = Number(chStr);
    const num = Number(numStr);
    if (!Number.isFinite(ch) || !Number.isFinite(num)) return null;
    if (t === 'cc') return { type: 'cc', channel: ch, cc: num };
    if (t === 'note') return { type: 'note', channel: ch, note: num };
    return null;
  }

  function start(): void {
    if (listening) return;
    listening = true;
    unsub = onceCapture((ev) => {
      if (kind === 'midi-in') {
        const m = tryParseMidi(ev);
        if (!m) return false;
        dispatch('capturedMidi', m);
        stop();
        return true;
      }
      if (!matches(ev)) return false;
      dispatch('captured', ev.control_id!);
      stop();
      return true;
    });
    window.addEventListener('keydown', onKey);
  }

  function stop(): void {
    listening = false;
    unsub?.();
    unsub = null;
    window.removeEventListener('keydown', onKey);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') stop();
  }

  onDestroy(() => stop());
</script>

<button
  type="button"
  class="px-2 py-1 text-xs rounded border transition-colors"
  class:bg-accent={!listening}
  class:text-slate-900={!listening}
  class:border-accent={!listening}
  class:bg-amber-500={listening}
  class:text-white={listening}
  class:border-amber-300={listening}
  class:animate-pulse={listening}
  on:click={() => (listening ? stop() : start())}
>
  {#if listening}
    {kind === 'midi-in' ? 'Send a MIDI message…' : 'Press a control…'} (Esc to cancel)
  {:else}
    {label}
  {/if}
</button>
