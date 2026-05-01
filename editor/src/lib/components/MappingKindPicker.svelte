<script lang="ts" context="module">
  export type MappingKind = 'action' | 'passthrough' | 'midi-translate' | 'raw';
</script>

<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let kind: MappingKind = 'action';

  const dispatch = createEventDispatcher<{ change: MappingKind }>();

  const opts: { value: MappingKind; label: string; hint: string }[] = [
    { value: 'action', label: 'Action', hint: 'Call a driver action with params' },
    { value: 'passthrough', label: 'Passthrough', hint: 'Forward MIDI raw to app port' },
    { value: 'midi-translate', label: 'MIDI translate', hint: 'Forward as fixed CC/Note' },
    { value: 'raw', label: 'Raw YAML', hint: 'Edit JSON directly' }
  ];

  function pick(v: MappingKind): void {
    if (kind === v) return;
    kind = v;
    dispatch('change', v);
  }
</script>

<div class="flex flex-wrap gap-1 p-1 rounded-md bg-slate-100 dark:bg-slate-800/60 border border-slate-200 dark:border-slate-800">
  {#each opts as o}
    <button
      type="button"
      class="px-2.5 py-1 rounded text-xs font-medium transition-colors"
      class:bg-accent={kind === o.value}
      class:text-slate-900={kind === o.value}
      class:text-slate-600={kind !== o.value}
      class:dark:text-slate-300={kind !== o.value}
      class:hover:bg-white={kind !== o.value}
      class:dark:hover:bg-slate-700={kind !== o.value}
      title={o.hint}
      on:click={() => pick(o.value)}
    >{o.label}</button>
  {/each}
</div>
