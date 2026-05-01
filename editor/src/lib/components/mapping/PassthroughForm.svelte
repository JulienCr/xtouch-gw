<script lang="ts">
  import PickerField from '../PickerField.svelte';

  export let mapping: Record<string, unknown> = {};
  export let driverOpts: { value: string; label: string }[] = [];

  $: app = (mapping.app ?? '') as string;

  function setApp(v: string): void {
    mapping = { ...mapping, app: v, midi: { type: 'passthrough' } };
  }

  // Ensure midi.type stays 'passthrough' on this form.
  $: if (!mapping.midi || (mapping.midi as { type?: string }).type !== 'passthrough') {
    mapping = { ...mapping, midi: { type: 'passthrough' } };
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
  <div class="text-xs text-slate-500 dark:text-slate-400 italic">
    The raw MIDI message from this control will be forwarded to the app's output port unchanged.
  </div>
</div>
