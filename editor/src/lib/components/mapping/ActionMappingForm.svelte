<script lang="ts">
  import PickerField from '../PickerField.svelte';
  import ParamField from './ParamField.svelte';
  import { pickers, pickerActions } from '$lib/stores/pickers';
  import { inputCls } from '$lib/styles';
  import type { ActionDescriptor, ActionParam } from '$lib/api';

  export let mapping: Record<string, unknown> = {};
  export let driverOpts: { value: string; label: string }[] = [];

  let actions: ActionDescriptor[] = [];

  $: app = (mapping.app ?? '') as string;
  $: action = (mapping.action ?? '') as string;
  $: if (app) loadActions(app);

  async function loadActions(name: string): Promise<void> {
    actions = await pickerActions.actionsFor(name);
  }

  $: actionDesc = actions.find((a) => a.name === action) ?? null;
  $: actionParams = (actionDesc?.params ?? []) as ActionParam[];

  function paramValue(idx: number): unknown {
    const arr = (mapping.params ?? []) as unknown[];
    return arr[idx];
  }

  function setParam(idx: number, val: unknown): void {
    const arr = ((mapping.params ?? []) as unknown[]).slice();
    arr[idx] = val;
    mapping = { ...mapping, params: arr };
  }

  // Precomputed scene context per param index (for `obs.source` pickers,
  // shows the source list of the nearest preceding `obs.scene` param).
  // Built reactively from `mapping.params` so Svelte tracks the dep — a
  // function call would hide it through the function body.
  $: paramScenes = (() => {
    const arr = (mapping.params ?? []) as unknown[];
    const out: string[] = [];
    let current = '';
    for (let i = 0; i < actionParams.length; i++) {
      out.push(current);
      if (actionParams[i]?.picker === 'obs.scene' && typeof arr[i] === 'string') {
        current = arr[i] as string;
      }
    }
    return out;
  })();

  let sourceOptionsByScene: Record<string, { value: string; label: string }[]> = {};

  async function ensureSources(scene: string): Promise<void> {
    if (!scene || sourceOptionsByScene[scene]) return;
    const list = await pickerActions.sourcesFor(scene);
    sourceOptionsByScene = {
      ...sourceOptionsByScene,
      [scene]: list.map((s) => ({ value: s.name, label: s.name }))
    };
  }

  $: paramScenes.forEach((sc, i) => {
    if (actionParams[i]?.picker === 'obs.source') ensureSources(sc);
  });

  $: sceneOpts = $pickers.obsScenes.map((s) => ({ value: s.name, label: s.name }));
  $: inputOpts = $pickers.obsInputs.map((s) => ({ value: s.name, label: s.name }));
  $: actionOpts = actions.map((a) => ({ value: a.name, label: a.name, meta: a.description }));
</script>

<div class="space-y-3">
  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">App</div>
    <PickerField value={app} options={driverOpts} placeholder="select app…" allowFree on:change={(e) => (mapping = { ...mapping, app: e.detail })} />
  </div>

  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Action</div>
    <PickerField value={action ?? ''} options={actionOpts} placeholder="select action…" allowFree on:change={(e) => (mapping = { ...mapping, action: e.detail })} />
  </div>

  {#if actionParams.length}
    <div class="rounded border border-slate-200 dark:border-slate-800 p-2 space-y-2">
      <div class="text-xs text-slate-700 dark:text-slate-400">Params</div>
      {#each actionParams as p, idx}
        <ParamField
          param={p}
          value={paramValue(idx)}
          {sceneOpts}
          {inputOpts}
          sourceOpts={sourceOptionsByScene[paramScenes[idx] ?? ''] ?? []}
          sceneContext={paramScenes[idx] ?? ''}
          on:change={(e) => setParam(idx, e.detail)}
        />
      {/each}
    </div>
  {:else if mapping.params}
    <div class="rounded border border-slate-200 dark:border-slate-800 p-2">
      <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Params (raw JSON)</div>
      <textarea
        class="{inputCls} font-mono text-xs h-20"
        value={JSON.stringify(mapping.params)}
        on:input={(e) => {
          try {
            mapping = { ...mapping, params: JSON.parse((e.currentTarget as HTMLTextAreaElement).value) };
          } catch {
            /* ignore */
          }
        }}
      ></textarea>
    </div>
  {/if}
</div>
