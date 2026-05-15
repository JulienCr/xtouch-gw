<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import PickerField from '../PickerField.svelte';
  import { inputCls, labelCls } from '$lib/styles';
  import type { ActionParam } from '$lib/api';
  import { profile } from '$lib/stores/profile';

  export let param: ActionParam;
  export let value: unknown;
  export let sceneOpts: { value: string; label: string }[] = [];
  export let inputOpts: { value: string; label: string }[] = [];
  export let sourceOpts: { value: string; label: string }[] = [];
  export let sceneContext: string = '';

  const dispatch = createEventDispatcher<{ change: unknown }>();

  // Target list for the winaudio session_volume / session_mute params.
  // - `auto`: driver picks the next free detected app at runtime.
  // - `pinned:N` (1..=8): the fader slot configured in `winaudio.pinned_apps`;
  //   when a process_name is set we surface it inline so the user doesn't have
  //   to cross-reference YAML.
  // - `discovered:N` (0..=7): legacy FIFO indices kept for backward compat.
  $: pinnedApps = (() => {
    const apps = ($profile.parsed as { winaudio?: { pinned_apps?: Array<{ fader: number; process_name?: string; display_name?: string }> } } | null)
      ?.winaudio?.pinned_apps ?? [];
    const byFader = new Map<number, string>();
    for (const p of apps) {
      const label = p.display_name?.trim() || p.process_name?.trim() || '';
      if (label) byFader.set(p.fader, label);
    }
    return byFader;
  })();

  $: winaudioTargetOpts = (() => {
    const opts: { value: string; label: string }[] = [
      { value: 'auto', label: 'auto (any detected app)' }
    ];
    for (let n = 1; n <= 8; n++) {
      const app = pinnedApps.get(n);
      opts.push({ value: `pinned:${n}`, label: app ? `pinned:${n} (${app})` : `pinned:${n}` });
    }
    for (let n = 0; n < 8; n++) {
      opts.push({ value: `discovered:${n}`, label: `discovered:${n} (legacy)` });
    }
    return opts;
  })();

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
