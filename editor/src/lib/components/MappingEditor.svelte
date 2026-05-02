<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { selectedControl, selectedPage } from '$lib/stores/selection';
  import { pickers, pickerActions } from '$lib/stores/pickers';
  import CaptureButton from './CaptureButton.svelte';
  import MappingKindPicker, { type MappingKind } from './MappingKindPicker.svelte';
  import ActionMappingForm from './mapping/ActionMappingForm.svelte';
  import PassthroughForm from './mapping/PassthroughForm.svelte';
  import MidiTranslateForm from './mapping/MidiTranslateForm.svelte';
  import RawYamlForm from './mapping/RawYamlForm.svelte';
  import LcdStripEditor from './LcdStripEditor.svelte';
  import { onMount } from 'svelte';

  type Mapping = {
    app?: string;
    action?: string | null;
    params?: unknown[];
    indicator?: unknown;
    midi?: { type?: string; channel?: number; cc?: number; note?: number } | null;
    overlay?: { mode?: string } | null;
    [k: string]: unknown;
  };

  let mapping: Mapping = {};
  let originalControlId = '';
  let originalScope = ''; // tracks "page:<idx>" or "global"
  let kind: MappingKind = 'action';

  $: pageIdx = $selectedPage;
  $: controlId = $selectedControl ?? '';
  $: scopeKey = pageIdx === -1 ? 'global' : `page:${pageIdx}`;

  $: existing = (() => {
    const cfg = $profile.parsed;
    if (!cfg || !controlId) return null;
    if (pageIdx === -1) {
      return ((cfg.pages_global?.controls?.[controlId] ?? null) as Mapping | null);
    }
    return ((cfg.pages?.[pageIdx]?.controls?.[controlId] ?? null) as Mapping | null);
  })();

  function inferKind(m: Mapping | null | undefined): MappingKind {
    if (!m || Object.keys(m).length === 0) return 'action';
    const midi = m.midi as { type?: string } | undefined;
    if (midi?.type === 'passthrough') return 'passthrough';
    if (midi?.type === 'cc' || midi?.type === 'note') return 'midi-translate';
    if (m.action !== undefined && m.action !== null) return 'action';
    if (m.app && !m.midi) return 'action';
    return 'raw';
  }

  $: if (controlId && (originalControlId !== controlId || originalScope !== scopeKey)) {
    originalControlId = controlId;
    originalScope = scopeKey;
    mapping = existing ? (JSON.parse(JSON.stringify(existing)) as Mapping) : { app: '', action: '', params: [] };
    kind = inferKind(mapping);
  }

  $: driverOpts = $pickers.drivers.map((d) => ({ value: d.name, label: d.name }));

  function changeKind(newKind: MappingKind): void {
    if (newKind === kind) return;
    const preserved: Mapping = {};
    if (mapping.app !== undefined) preserved.app = mapping.app;
    if (mapping.overlay !== undefined) preserved.overlay = mapping.overlay;
    if (mapping.indicator !== undefined) preserved.indicator = mapping.indicator;

    if (newKind === 'action') {
      mapping = { ...preserved, action: '', params: [] };
    } else if (newKind === 'passthrough') {
      mapping = { ...preserved, midi: { type: 'passthrough' } };
    } else if (newKind === 'midi-translate') {
      mapping = { ...preserved, midi: { type: 'cc', channel: 1, cc: 0 } };
    } else {
      mapping = { ...mapping };
    }
    kind = newKind;
  }

  // ----- Overlay shared field -----
  type OverlayMode = '8bit' | 'percent' | 'off' | '';
  $: overlayMode = ((mapping.overlay?.mode ?? '') as OverlayMode);

  function setOverlayMode(v: OverlayMode): void {
    if (!v) {
      const next = { ...mapping };
      delete next.overlay;
      mapping = next;
    } else {
      mapping = { ...mapping, overlay: { ...(mapping.overlay ?? {}), mode: v } };
    }
  }

  // ----- Persistence -----
  function patchTarget(mutator: (controls: Record<string, Mapping>) => void): void {
    profileActions.patchParsed((cfg) => {
      if (pageIdx === -1) {
        if (!cfg.pages_global) cfg.pages_global = { controls: {} };
        if (!cfg.pages_global.controls) cfg.pages_global.controls = {};
        mutator(cfg.pages_global.controls as Record<string, Mapping>);
      } else {
        const p = cfg.pages?.[pageIdx] as { controls?: Record<string, Mapping> } | undefined;
        if (!p) return;
        if (!p.controls) p.controls = {};
        mutator(p.controls);
      }
    });
  }

  function save(): void {
    if (!controlId) return;
    patchTarget((controls) => {
      controls[controlId] = JSON.parse(JSON.stringify(mapping));
    });
  }

  function remove(): void {
    if (!controlId) return;
    patchTarget((controls) => {
      delete controls[controlId];
    });
    selectedControl.set(null);
  }

  function cancel(): void {
    mapping = existing ? (JSON.parse(JSON.stringify(existing)) as Mapping) : { app: '', action: '', params: [] };
    kind = inferKind(mapping);
  }

  function onCaptured(e: CustomEvent<string>): void {
    selectedControl.set(e.detail);
  }

  onMount(() => {
    pickerActions.refresh();
  });
</script>

<div class="p-3 space-y-3 text-sm">
  {#if !controlId}
    <div class="text-slate-500 dark:text-slate-400 text-xs">Click a control on the surface or list to edit its mapping.</div>
  {:else if /^lcd(\d+)$/.test(controlId) && pageIdx >= 0}
    <LcdStripEditor stripIdx={Number(controlId.match(/^lcd(\d+)$/)?.[1] ?? 1)} />
  {:else}
    <div class="flex items-center gap-2">
      <div class="flex-1">
        <div class="text-xs text-slate-500 dark:text-slate-400">
          Trigger {pageIdx === -1 ? '(global)' : ''}
        </div>
        <div class="font-mono text-sm">{controlId}</div>
      </div>
      <CaptureButton kind="any" label="Capture →" on:captured={onCaptured} />
    </div>

    <MappingKindPicker {kind} on:change={(e) => changeKind(e.detail)} />

    {#if kind === 'action'}
      <ActionMappingForm bind:mapping {driverOpts} />
    {:else if kind === 'passthrough'}
      <PassthroughForm bind:mapping {driverOpts} />
    {:else if kind === 'midi-translate'}
      <MidiTranslateForm bind:mapping {driverOpts} />
    {:else}
      <RawYamlForm bind:mapping />
    {/if}

    <div class="rounded border border-slate-200 dark:border-slate-800 p-2">
      <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Overlay</div>
      <select
        class="w-full bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 rounded px-2 py-1.5 text-xs"
        value={overlayMode}
        on:change={(e) => setOverlayMode((e.currentTarget as HTMLSelectElement).value as OverlayMode)}
      >
        <option value="">(none)</option>
        <option value="8bit">8bit</option>
        <option value="percent">percent</option>
        <option value="off">off</option>
      </select>
    </div>

    <div class="flex items-center gap-2 pt-2">
      <button class="px-3 py-1.5 rounded bg-accent text-slate-900 text-sm font-semibold" on:click={save}>Save mapping</button>
      <button class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm" on:click={cancel}>Revert</button>
      {#if existing}
        <button class="ml-auto px-3 py-1.5 rounded text-rose-700 hover:bg-rose-100 dark:text-rose-300 dark:hover:bg-rose-950/30 text-sm" on:click={remove}>Delete</button>
      {/if}
    </div>
  {/if}
</div>
