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

  $: pickerOpts =
    param.picker === 'obs.scene'
      ? sceneOpts
      : param.picker === 'obs.input'
        ? inputOpts
        : param.picker === 'obs.source'
          ? sourceOpts
          : null;

  $: pickerPlaceholder =
    param.picker === 'obs.scene'
      ? 'scene…'
      : param.picker === 'obs.input'
        ? 'input…'
        : param.picker === 'obs.source'
          ? 'source…'
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
