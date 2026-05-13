<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import PickerField from '../PickerField.svelte';
  import { inputCls, labelCls } from '$lib/styles';
  import type { ActionParam } from '$lib/api';

  export let param: ActionParam;
  export let value: unknown;
  export let sceneOpts: { value: string; label: string }[] = [];
  export let inputOpts: { value: string; label: string }[] = [];
  export let sourceOpts: { value: string; label: string }[] = [];
  export let sceneContext: string = '';

  const dispatch = createEventDispatcher<{ change: unknown }>();

  // Static target list for the winaudio session_volume / session_mute params.
  // Pinned slots map 1..=8 to `winaudio.pinned_apps[i].fader`. Discovered
  // slots 0..=7 are the legacy FIFO indices; `auto` is the modern form
  // (driver picks the next free detected app at runtime).
  const winaudioTargetOpts: { value: string; label: string }[] = [
    { value: 'auto', label: 'auto (any detected app)' },
    { value: 'pinned:1', label: 'pinned:1' },
    { value: 'pinned:2', label: 'pinned:2' },
    { value: 'pinned:3', label: 'pinned:3' },
    { value: 'pinned:4', label: 'pinned:4' },
    { value: 'pinned:5', label: 'pinned:5' },
    { value: 'pinned:6', label: 'pinned:6' },
    { value: 'pinned:7', label: 'pinned:7' },
    { value: 'pinned:8', label: 'pinned:8' },
    { value: 'discovered:0', label: 'discovered:0 (legacy)' },
    { value: 'discovered:1', label: 'discovered:1 (legacy)' },
    { value: 'discovered:2', label: 'discovered:2 (legacy)' },
    { value: 'discovered:3', label: 'discovered:3 (legacy)' },
    { value: 'discovered:4', label: 'discovered:4 (legacy)' },
    { value: 'discovered:5', label: 'discovered:5 (legacy)' },
    { value: 'discovered:6', label: 'discovered:6 (legacy)' },
    { value: 'discovered:7', label: 'discovered:7 (legacy)' }
  ];

  $: pickerOpts =
    param.picker === 'obs.scene'
      ? sceneOpts
      : param.picker === 'obs.input'
        ? inputOpts
        : param.picker === 'obs.source'
          ? sourceOpts
          : param.picker === 'winaudio.target'
            ? winaudioTargetOpts
            : null;

  $: pickerPlaceholder =
    param.picker === 'obs.scene'
      ? 'scene…'
      : param.picker === 'obs.input'
        ? 'input…'
        : param.picker === 'obs.source'
          ? 'source…'
          : param.picker === 'winaudio.target'
            ? 'auto, pinned:N, discovered:N'
            : '';

  $: label =
    param.picker === 'obs.source' ? `${param.name} (in ${sceneContext || 'any scene'})` : param.name;
</script>

{#if param.kind === 'boolean'}
  <label class="flex items-center gap-2 text-xs">
    <input
      type="checkbox"
      checked={!!value}
      on:change={(e) => dispatch('change', (e.currentTarget as HTMLInputElement).checked)}
    />
    {param.name}
  </label>
{:else}
  <div>
    <div class={labelCls}>{label}</div>
    {#if pickerOpts}
      <PickerField
        value={(value ?? '') as string}
        options={pickerOpts}
        allowFree
        placeholder={pickerPlaceholder}
        on:change={(e) => dispatch('change', e.detail)}
      />
    {:else if param.kind === 'number' || param.kind === 'integer'}
      <input
        type="number"
        class={inputCls}
        value={(value ?? '') as number}
        on:input={(e) => dispatch('change', Number((e.currentTarget as HTMLInputElement).value))}
      />
    {:else}
      <input
        class={inputCls}
        value={(value ?? '') as string}
        on:input={(e) => dispatch('change', (e.currentTarget as HTMLInputElement).value)}
      />
    {/if}
  </div>
{/if}
