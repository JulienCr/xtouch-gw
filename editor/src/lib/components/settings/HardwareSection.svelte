<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';

  $: cfg = $profile.parsed as Record<string, unknown> | null;
  $: xtouch = (cfg?.xtouch as Record<string, unknown> | undefined) ?? {};
  $: gamepad = (cfg?.gamepad as Record<string, unknown> | undefined) ?? {};
  $: overlayPerApp =
    (xtouch.overlay_per_app as Record<string, Record<string, unknown>> | undefined) ?? {};
  $: overlayEntries = Object.entries(overlayPerApp);
  $: gamepads = ((gamepad.gamepads as Array<Record<string, unknown>> | undefined) ?? []);

  const inputCls =
    'w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm';
  const labelCls = 'block text-xs text-slate-700 dark:text-slate-400 mb-1';
  const subHeader = 'text-xs uppercase tracking-wide text-slate-500 dark:text-slate-400 font-semibold';
  const pillBase =
    'px-3 py-1 rounded-full text-xs border transition-colors cursor-pointer select-none';

  const XTOUCH_MODES = ['mcu', 'ctrl'] as const;
  const OVERLAY_MODES = ['8bit', 'percent', 'off'] as const;
  const PROVIDERS = ['hid', 'xinput', 'gilrs'] as const;
  const INVERT_KEYS = ['lx', 'ly', 'rx', 'ry', 'zl', 'zr'] as const;

  function setXTouch(key: string, val: unknown): void {
    profileActions.patchParsed((c) => {
      const x = ((c as Record<string, unknown>).xtouch as Record<string, unknown>) ?? {};
      x[key] = val;
      (c as Record<string, unknown>).xtouch = x;
    });
  }

  function setOverlayKey(oldKey: string, newKey: string): void {
    if (!newKey || newKey === oldKey) return;
    profileActions.patchParsed((c) => {
      const x = ((c as Record<string, unknown>).xtouch as Record<string, unknown>) ?? {};
      const o = (x.overlay_per_app as Record<string, unknown> | undefined) ?? {};
      const next: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(o)) {
        next[k === oldKey ? newKey : k] = v;
      }
      x.overlay_per_app = next;
      (c as Record<string, unknown>).xtouch = x;
    });
  }

  function setOverlayMode(key: string, mode: string): void {
    profileActions.patchParsed((c) => {
      const x = ((c as Record<string, unknown>).xtouch as Record<string, unknown>) ?? {};
      const o = ((x.overlay_per_app as Record<string, unknown> | undefined) ?? {}) as Record<
        string,
        Record<string, unknown>
      >;
      o[key] = { ...(o[key] ?? {}), mode };
      x.overlay_per_app = o;
      (c as Record<string, unknown>).xtouch = x;
    });
  }

  function addOverlay(): void {
    profileActions.patchParsed((c) => {
      const x = ((c as Record<string, unknown>).xtouch as Record<string, unknown>) ?? {};
      const o = ((x.overlay_per_app as Record<string, unknown> | undefined) ?? {}) as Record<
        string,
        unknown
      >;
      let n = 1;
      while (o[`app${n}`] !== undefined) n++;
      o[`app${n}`] = { mode: '8bit' };
      x.overlay_per_app = o;
      (c as Record<string, unknown>).xtouch = x;
    });
  }

  function removeOverlay(key: string): void {
    profileActions.patchParsed((c) => {
      const x = ((c as Record<string, unknown>).xtouch as Record<string, unknown>) ?? {};
      const o = ((x.overlay_per_app as Record<string, unknown> | undefined) ?? {}) as Record<
        string,
        unknown
      >;
      delete o[key];
      x.overlay_per_app = o;
      (c as Record<string, unknown>).xtouch = x;
    });
  }

  function setGamepad(key: string, val: unknown): void {
    profileActions.patchParsed((c) => {
      const g = ((c as Record<string, unknown>).gamepad as Record<string, unknown>) ?? {};
      g[key] = val;
      (c as Record<string, unknown>).gamepad = g;
    });
  }

  function patchSlot(idx: number, mutator: (slot: Record<string, unknown>) => void): void {
    profileActions.patchParsed((c) => {
      const g = ((c as Record<string, unknown>).gamepad as Record<string, unknown>) ?? {};
      const list = ((g.gamepads as Array<Record<string, unknown>> | undefined) ?? []).map((s) => ({
        ...s
      }));
      const slot = { ...(list[idx] ?? {}) };
      mutator(slot);
      list[idx] = slot;
      g.gamepads = list;
      (c as Record<string, unknown>).gamepad = g;
    });
  }

  function setSlotField(idx: number, key: string, val: unknown): void {
    patchSlot(idx, (s) => {
      s[key] = val;
    });
  }

  function setSlotAnalog(idx: number, key: string, val: number): void {
    patchSlot(idx, (s) => {
      const a = { ...((s.analog as Record<string, unknown> | undefined) ?? {}) };
      a[key] = val;
      s.analog = a;
    });
  }

  function setSlotInvert(idx: number, key: string, val: boolean): void {
    patchSlot(idx, (s) => {
      const a = { ...((s.analog as Record<string, unknown> | undefined) ?? {}) };
      const inv = { ...((a.invert as Record<string, boolean> | undefined) ?? {}) };
      inv[key] = val;
      a.invert = inv;
      s.analog = a;
    });
  }

  function addGamepad(): void {
    profileActions.patchParsed((c) => {
      const g = ((c as Record<string, unknown>).gamepad as Record<string, unknown>) ?? {};
      const list = (g.gamepads as Array<Record<string, unknown>> | undefined) ?? [];
      g.gamepads = [
        ...list,
        {
          product_match: '',
          camera_target: 'dynamic',
          analog: {
            pan_gain: 15,
            zoom_gain: 3,
            deadzone: 0.02,
            gamma: 1.5,
            invert: { lx: false, ly: false, rx: false, ry: false, zl: false, zr: false }
          }
        }
      ];
      (c as Record<string, unknown>).gamepad = g;
    });
  }

  function removeGamepad(idx: number): void {
    profileActions.patchParsed((c) => {
      const g = ((c as Record<string, unknown>).gamepad as Record<string, unknown>) ?? {};
      const list = ((g.gamepads as Array<Record<string, unknown>> | undefined) ?? []).slice();
      list.splice(idx, 1);
      g.gamepads = list;
      (c as Record<string, unknown>).gamepad = g;
    });
  }

  type AnalogSpec = { key: 'pan_gain' | 'zoom_gain' | 'deadzone' | 'gamma'; label: string; min: number; max: number; step: number };
  const ANALOG_SPECS: AnalogSpec[] = [
    { key: 'pan_gain', label: 'Pan gain', min: 0, max: 50, step: 1 },
    { key: 'zoom_gain', label: 'Zoom gain', min: 0, max: 10, step: 0.1 },
    { key: 'deadzone', label: 'Deadzone', min: 0, max: 0.5, step: 0.01 },
    { key: 'gamma', label: 'Gamma', min: 0.5, max: 3, step: 0.1 }
  ];
</script>

<section
  class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30"
>
  <header class="px-4 py-2 border-b border-slate-200 dark:border-slate-800 text-sm font-semibold">
    Hardware
  </header>
  <div class="p-4 space-y-6">
    <!-- X-Touch -->
    <div class="space-y-3">
      <div class={subHeader}>X-Touch</div>
      <div>
        <span class={labelCls}>Mode</span>
        <div class="flex gap-2">
          {#each XTOUCH_MODES as m}
            {@const active = (xtouch.mode as string) === m}
            <button
              type="button"
              class={pillBase}
              class:bg-accent={active}
              class:text-slate-900={active}
              class:font-semibold={active}
              class:border-transparent={active}
              class:border-slate-300={!active}
              class:dark:border-slate-700={!active}
              on:click={() => setXTouch('mode', m)}
            >
              {m}
            </button>
          {/each}
        </div>
      </div>

      <div class="space-y-2">
        <div class="text-xs text-slate-700 dark:text-slate-400">Per-app overlay</div>
        {#if overlayEntries.length === 0}
          <div class="text-xs text-slate-500 italic">No overlays defined.</div>
        {/if}
        {#each overlayEntries as [appKey, entry] (appKey)}
          <div class="flex items-center gap-2">
            <input
              type="text"
              class="{inputCls} flex-1"
              value={appKey}
              on:change={(e) => setOverlayKey(appKey, (e.currentTarget as HTMLInputElement).value)}
            />
            <select
              class="{inputCls} max-w-[8rem]"
              value={(entry?.mode as string) ?? '8bit'}
              on:change={(e) =>
                setOverlayMode(appKey, (e.currentTarget as HTMLSelectElement).value)}
            >
              {#each OVERLAY_MODES as m}
                <option value={m}>{m}</option>
              {/each}
            </select>
            <button
              class="text-xs text-rose-600 hover:bg-rose-100 dark:text-rose-400 dark:hover:bg-rose-950/40 px-2 py-1 rounded"
              on:click={() => removeOverlay(appKey)}
            >
              Remove
            </button>
          </div>
        {/each}
        <button
          class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm"
          on:click={addOverlay}
        >
          + Add overlay
        </button>
      </div>
    </div>

    <!-- Gamepad -->
    <div class="space-y-3">
      <div class={subHeader}>Gamepad</div>
      <div class="flex flex-wrap items-center gap-4">
        <label class="inline-flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={!!gamepad.enabled}
            on:change={(e) =>
              setGamepad('enabled', (e.currentTarget as HTMLInputElement).checked)}
          />
          Enabled
        </label>
        <div class="flex items-center gap-2">
          <span class="text-xs text-slate-700 dark:text-slate-400">Provider</span>
          <div class="flex gap-1">
            {#each PROVIDERS as p}
              {@const active = (gamepad.provider as string) === p}
              <button
                type="button"
                class={pillBase}
                class:bg-accent={active}
                class:text-slate-900={active}
                class:font-semibold={active}
                class:border-transparent={active}
                class:border-slate-300={!active}
                class:dark:border-slate-700={!active}
                on:click={() => setGamepad('provider', p)}
              >
                {p}
              </button>
            {/each}
          </div>
        </div>
      </div>

      <div class="space-y-3">
        {#each gamepads as gp, i (i)}
          {@const analog = (gp.analog as Record<string, unknown> | undefined) ?? {}}
          {@const invert = (analog.invert as Record<string, boolean> | undefined) ?? {}}
          <div
            class="rounded-lg border border-slate-200 dark:border-slate-800 bg-white/60 dark:bg-slate-900/40 p-3 space-y-3"
          >
            <div class="flex items-center justify-between">
              <span class="text-xs text-slate-500">Gamepad #{i + 1}</span>
              <button
                class="text-xs text-rose-600 hover:bg-rose-100 dark:text-rose-400 dark:hover:bg-rose-950/40 px-2 py-1 rounded"
                on:click={() => removeGamepad(i)}
              >
                Remove
              </button>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-2 gap-2">
              <div>
                <span class={labelCls}>Product match</span>
                <input
                  type="text"
                  class={inputCls}
                  value={(gp.product_match as string) ?? ''}
                  on:input={(e) =>
                    setSlotField(i, 'product_match', (e.currentTarget as HTMLInputElement).value)}
                />
              </div>
              <div>
                <span class={labelCls}>Camera target</span>
                <input
                  type="text"
                  class={inputCls}
                  placeholder="dynamic, Jardin, …"
                  value={(gp.camera_target as string) ?? ''}
                  on:input={(e) =>
                    setSlotField(i, 'camera_target', (e.currentTarget as HTMLInputElement).value)}
                />
              </div>
            </div>

            <div class="space-y-2">
              <div class="text-xs text-slate-700 dark:text-slate-400">Analog</div>
              {#each ANALOG_SPECS as spec}
                {@const v = Number(analog[spec.key] ?? 0)}
                <div class="grid grid-cols-[6rem_1fr_5rem] items-center gap-2">
                  <span class="text-xs text-slate-600 dark:text-slate-400">{spec.label}</span>
                  <input
                    type="range"
                    min={spec.min}
                    max={spec.max}
                    step={spec.step}
                    value={v}
                    on:input={(e) =>
                      setSlotAnalog(
                        i,
                        spec.key,
                        Number((e.currentTarget as HTMLInputElement).value)
                      )}
                  />
                  <input
                    type="number"
                    min={spec.min}
                    max={spec.max}
                    step={spec.step}
                    class={inputCls}
                    value={v}
                    on:input={(e) =>
                      setSlotAnalog(
                        i,
                        spec.key,
                        Number((e.currentTarget as HTMLInputElement).value)
                      )}
                  />
                </div>
              {/each}
            </div>

            <div class="space-y-2">
              <div class="text-xs text-slate-700 dark:text-slate-400">Invert</div>
              <div class="grid grid-cols-3 gap-2">
                {#each INVERT_KEYS as k}
                  <label class="inline-flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={!!invert[k]}
                      on:change={(e) =>
                        setSlotInvert(i, k, (e.currentTarget as HTMLInputElement).checked)}
                    />
                    <span class="font-mono text-xs">{k}</span>
                  </label>
                {/each}
              </div>
            </div>
          </div>
        {/each}
        <button
          class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm"
          on:click={addGamepad}
        >
          + Add gamepad
        </button>
      </div>
    </div>
  </div>
</section>
