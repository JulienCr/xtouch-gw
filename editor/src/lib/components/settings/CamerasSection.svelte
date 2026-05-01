<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { pickers } from '$lib/stores/pickers';
  import PickerField from '../PickerField.svelte';

  $: cfg = $profile.parsed as Record<string, unknown> | null;
  $: obs = (cfg?.obs as Record<string, unknown> | undefined) ?? {};
  $: cc = (obs.camera_control as Record<string, unknown> | undefined) ?? {};
  $: cameras = ((cc.cameras as Array<Record<string, unknown>> | undefined) ?? []);
  $: splits = ((cc.splits as Record<string, unknown> | undefined) ?? {});

  $: sceneOptions = ($pickers.obsScenes ?? []).map((s) => ({
    value: s.name,
    label: s.name
  }));
  $: inputOptions = ($pickers.obsInputs ?? []).map((i) => ({
    value: i.name,
    label: i.name,
    meta: typeof i.kind === 'string' ? (i.kind as string) : undefined
  }));

  const inputCls =
    'w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm';
  const labelCls = 'block text-xs text-slate-700 dark:text-slate-400 mb-1';

  function patchCC(mutator: (cc: Record<string, unknown>) => void): void {
    profileActions.patchParsed((c) => {
      const o = ((c as Record<string, unknown>).obs as Record<string, unknown>) ?? {};
      const cur = ((o.camera_control as Record<string, unknown> | undefined) ?? {}) as Record<
        string,
        unknown
      >;
      mutator(cur);
      o.camera_control = cur;
      (c as Record<string, unknown>).obs = o;
    });
  }

  function setDefaultCamera(val: string): void {
    patchCC((cur) => {
      cur.default_camera = val || null;
    });
  }

  function patchCamera(idx: number, key: string, val: unknown): void {
    patchCC((cur) => {
      const list = ((cur.cameras as Array<Record<string, unknown>> | undefined) ?? []).map((c) => ({
        ...c
      }));
      list[idx] = { ...(list[idx] ?? {}), [key]: val };
      cur.cameras = list;
    });
  }

  function addCamera(): void {
    patchCC((cur) => {
      const list = (cur.cameras as Array<Record<string, unknown>> | undefined) ?? [];
      cur.cameras = [
        ...list,
        { id: '', scene: '', source: '', split_source: '', enable_ptz: false }
      ];
    });
  }

  function removeCamera(idx: number): void {
    patchCC((cur) => {
      const list = ((cur.cameras as Array<Record<string, unknown>> | undefined) ?? []).slice();
      list.splice(idx, 1);
      cur.cameras = list;
    });
  }

  function moveCamera(idx: number, delta: number): void {
    patchCC((cur) => {
      const list = ((cur.cameras as Array<Record<string, unknown>> | undefined) ?? []).slice();
      const j = idx + delta;
      if (j < 0 || j >= list.length) return;
      [list[idx], list[j]] = [list[j], list[idx]];
      cur.cameras = list;
    });
  }

  function setSplit(side: 'left' | 'right', val: string): void {
    patchCC((cur) => {
      const s = ((cur.splits as Record<string, unknown> | undefined) ?? {}) as Record<
        string,
        unknown
      >;
      s[side] = val;
      cur.splits = s;
    });
  }
</script>

<section
  class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30"
>
  <header class="px-4 py-2 border-b border-slate-200 dark:border-slate-800 text-sm font-semibold">
    Cameras
  </header>
  <div class="p-4 space-y-5">
    <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
      <div>
        <span class={labelCls}>Default camera</span>
        <select
          class={inputCls}
          value={(cc.default_camera as string) ?? ''}
          on:change={(e) => setDefaultCamera((e.currentTarget as HTMLSelectElement).value)}
        >
          <option value="">(first)</option>
          {#each cameras as cam}
            <option value={(cam.id as string) ?? ''}>{(cam.id as string) ?? ''}</option>
          {/each}
        </select>
      </div>
      <div>
        <span class={labelCls}>Splits — left scene</span>
        <PickerField
          value={(splits.left as string) ?? ''}
          options={sceneOptions}
          allowFree
          placeholder="Select scene…"
          on:change={(e) => setSplit('left', e.detail)}
        />
      </div>
      <div>
        <span class={labelCls}>Splits — right scene</span>
        <PickerField
          value={(splits.right as string) ?? ''}
          options={sceneOptions}
          allowFree
          placeholder="Select scene…"
          on:change={(e) => setSplit('right', e.detail)}
        />
      </div>
    </div>

    <div class="space-y-3">
      <div class="text-xs text-slate-700 dark:text-slate-400">Cameras</div>
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
        {#each cameras as cam, i (i)}
          <div
            class="rounded-lg border border-slate-200 dark:border-slate-800 bg-white/60 dark:bg-slate-900/40 p-3 space-y-2"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="text-xs text-slate-500">Camera #{i + 1}</span>
              <div class="flex items-center gap-1">
                <button
                  class="text-xs px-2 py-1 rounded border border-slate-300 dark:border-slate-700 disabled:opacity-40"
                  disabled={i === 0}
                  title="Move up"
                  on:click={() => moveCamera(i, -1)}
                >
                  ↑
                </button>
                <button
                  class="text-xs px-2 py-1 rounded border border-slate-300 dark:border-slate-700 disabled:opacity-40"
                  disabled={i === cameras.length - 1}
                  title="Move down"
                  on:click={() => moveCamera(i, 1)}
                >
                  ↓
                </button>
                <button
                  class="text-xs text-rose-600 hover:bg-rose-100 dark:text-rose-400 dark:hover:bg-rose-950/40 px-2 py-1 rounded"
                  on:click={() => removeCamera(i)}
                >
                  Remove
                </button>
              </div>
            </div>
            <div>
              <span class={labelCls}>ID</span>
              <input
                type="text"
                class={inputCls}
                value={(cam.id as string) ?? ''}
                on:input={(e) =>
                  patchCamera(i, 'id', (e.currentTarget as HTMLInputElement).value)}
              />
            </div>
            <div>
              <span class={labelCls}>Scene</span>
              <PickerField
                value={(cam.scene as string) ?? ''}
                options={sceneOptions}
                allowFree
                placeholder="Select scene…"
                on:change={(e) => patchCamera(i, 'scene', e.detail)}
              />
            </div>
            <div>
              <span class={labelCls}>Source</span>
              <PickerField
                value={(cam.source as string) ?? ''}
                options={inputOptions}
                allowFree
                placeholder="Select input…"
                on:change={(e) => patchCamera(i, 'source', e.detail)}
              />
            </div>
            <div>
              <span class={labelCls}>Split source</span>
              <PickerField
                value={(cam.split_source as string) ?? ''}
                options={inputOptions}
                allowFree
                placeholder="Select input…"
                on:change={(e) => patchCamera(i, 'split_source', e.detail)}
              />
            </div>
            <label class="inline-flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={!!cam.enable_ptz}
                on:change={(e) =>
                  patchCamera(i, 'enable_ptz', (e.currentTarget as HTMLInputElement).checked)}
              />
              Enable PTZ
            </label>
          </div>
        {/each}
      </div>
      <button
        class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm"
        on:click={addCamera}
      >
        + Add camera
      </button>
    </div>
  </div>
</section>
