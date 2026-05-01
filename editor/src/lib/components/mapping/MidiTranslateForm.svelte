<script lang="ts">
  import PickerField from '../PickerField.svelte';
  import CaptureButton from '../CaptureButton.svelte';

  export let mapping: Record<string, unknown> = {};
  export let driverOpts: { value: string; label: string }[] = [];

  type MidiSpec = { type: 'cc' | 'note'; channel?: number; cc?: number; note?: number };

  $: app = (mapping.app ?? '') as string;
  $: midi = ((mapping.midi ?? { type: 'cc', channel: 1, cc: 0 }) as MidiSpec);
  $: type = (midi.type === 'note' ? 'note' : 'cc') as 'cc' | 'note';
  $: channel = typeof midi.channel === 'number' ? midi.channel : 1;
  $: numberVal = type === 'note' ? (midi.note ?? 0) : (midi.cc ?? 0);

  function setApp(v: string): void {
    mapping = { ...mapping, app: v };
  }

  function setType(t: 'cc' | 'note'): void {
    const next: MidiSpec = { type: t, channel };
    if (t === 'cc') next.cc = numberVal;
    else next.note = numberVal;
    mapping = { ...mapping, midi: next };
  }

  function setChannel(v: number): void {
    const ch = Math.max(1, Math.min(16, Math.round(v) || 1));
    const next: MidiSpec = { ...midi, type, channel: ch };
    mapping = { ...mapping, midi: next };
  }

  function setNumber(v: number): void {
    const n = Math.max(0, Math.min(127, Math.round(v) || 0));
    const next: MidiSpec = { type, channel };
    if (type === 'cc') next.cc = n;
    else next.note = n;
    mapping = { ...mapping, midi: next };
  }

  function onCaptured(e: CustomEvent<{ type: 'cc' | 'note'; channel: number; cc?: number; note?: number }>): void {
    const d = e.detail;
    const next: MidiSpec = { type: d.type, channel: d.channel };
    if (d.type === 'cc') next.cc = d.cc ?? 0;
    else next.note = d.note ?? 0;
    mapping = { ...mapping, midi: next };
  }
</script>

<div class="space-y-3">
  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">App</div>
    {#if driverOpts.length}
      <PickerField value={app} options={driverOpts} placeholder="select app…" allowFree on:change={(e) => setApp(e.detail)} />
    {:else}
      <input
        class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5"
        value={app}
        on:input={(e) => setApp((e.currentTarget as HTMLInputElement).value)}
      />
    {/if}
  </div>

  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Type</div>
    <div class="inline-flex rounded border border-slate-300 dark:border-slate-700 overflow-hidden">
      <button
        type="button"
        class="px-3 py-1 text-xs"
        class:bg-accent={type === 'cc'}
        class:text-slate-900={type === 'cc'}
        class:text-slate-600={type !== 'cc'}
        class:dark:text-slate-300={type !== 'cc'}
        on:click={() => setType('cc')}
      >CC</button>
      <button
        type="button"
        class="px-3 py-1 text-xs border-l border-slate-300 dark:border-slate-700"
        class:bg-accent={type === 'note'}
        class:text-slate-900={type === 'note'}
        class:text-slate-600={type !== 'note'}
        class:dark:text-slate-300={type !== 'note'}
        on:click={() => setType('note')}
      >Note</button>
    </div>
  </div>

  <div class="grid grid-cols-2 gap-2">
    <div>
      <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Channel (1-16)</div>
      <input
        type="number"
        min="1"
        max="16"
        class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5"
        value={channel}
        on:input={(e) => setChannel(Number((e.currentTarget as HTMLInputElement).value))}
      />
    </div>
    <div>
      <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">{type === 'cc' ? 'CC' : 'Note'} (0-127)</div>
      <input
        type="number"
        min="0"
        max="127"
        class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5"
        value={numberVal}
        on:input={(e) => setNumber(Number((e.currentTarget as HTMLInputElement).value))}
      />
    </div>
  </div>

  <div>
    <CaptureButton kind="midi-in" appName={app} label="Capture from MIDI in →" on:capturedMidi={onCaptured} />
  </div>
</div>
