<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { pickers } from '$lib/stores/pickers';
  import PickerField from '../PickerField.svelte';
  import { inputCls, labelCls, subHeader } from '$lib/styles';

  $: cfg = $profile.parsed as Record<string, unknown> | null;
  $: midi = (cfg?.midi as Record<string, unknown> | undefined) ?? {};
  $: obs = (cfg?.obs as Record<string, unknown> | undefined) ?? {};
  $: midiApps = ((midi.apps as Array<Record<string, unknown>> | undefined) ?? []);

  $: midiInputOptions = ($pickers.midi.inputs ?? []).map((p) => ({ value: p, label: p }));
  $: midiOutputOptions = ($pickers.midi.outputs ?? []).map((p) => ({ value: p, label: p }));

  let showPassword = false;

  const setMidi = (key: string, val: unknown) => profileActions.patchAt(['midi', key], val);
  const setObs = (key: string, val: unknown) => profileActions.patchAt(['obs', key], val);
  const patchApp = (idx: number, key: string, val: unknown) =>
    profileActions.patchAt(['midi', 'apps', idx, key], val);

  function addApp(): void {
    profileActions.patchParsed((c) => {
      const m = ((c as Record<string, unknown>).midi as Record<string, unknown>) ?? {};
      const apps = (m.apps as Array<Record<string, unknown>> | undefined) ?? [];
      m.apps = [...apps, { name: '', input_port: '', output_port: '' }];
      (c as Record<string, unknown>).midi = m;
    });
  }

  function removeApp(idx: number): void {
    profileActions.patchParsed((c) => {
      const m = ((c as Record<string, unknown>).midi as Record<string, unknown>) ?? {};
      const apps = ((m.apps as Array<Record<string, unknown>> | undefined) ?? []).slice();
      apps.splice(idx, 1);
      m.apps = apps;
    });
  }
</script>

<section
  class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30"
>
  <header class="px-4 py-2 border-b border-slate-200 dark:border-slate-800 text-sm font-semibold">
    Connections
  </header>
  <div class="p-4 space-y-6">
    <!-- MIDI -->
    <div class="space-y-3">
      <div class={subHeader}>MIDI</div>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div>
          <span class={labelCls}>Input port</span>
          <PickerField
            value={(midi.input_port as string) ?? ''}
            options={midiInputOptions}
            allowFree
            placeholder="Select input port…"
            on:change={(e) => setMidi('input_port', e.detail)}
          />
        </div>
        <div>
          <span class={labelCls}>Output port</span>
          <PickerField
            value={(midi.output_port as string) ?? ''}
            options={midiOutputOptions}
            allowFree
            placeholder="Select output port…"
            on:change={(e) => setMidi('output_port', e.detail)}
          />
        </div>
      </div>

      <div class="space-y-2">
        <div class="text-xs text-slate-700 dark:text-slate-400">Apps</div>
        {#each midiApps as app, i (i)}
          <div
            class="rounded-lg border border-slate-200 dark:border-slate-800 bg-white/60 dark:bg-slate-900/40 p-3 space-y-2"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="text-xs text-slate-500">App #{i + 1}</span>
              <button
                class="text-xs text-rose-600 hover:bg-rose-100 dark:text-rose-400 dark:hover:bg-rose-950/40 px-2 py-1 rounded"
                on:click={() => removeApp(i)}
              >
                Remove
              </button>
            </div>
            <div class="grid grid-cols-1 md:grid-cols-3 gap-2">
              <div>
                <span class={labelCls}>Name</span>
                <input
                  type="text"
                  class={inputCls}
                  value={(app.name as string) ?? ''}
                  on:input={(e) =>
                    patchApp(i, 'name', (e.currentTarget as HTMLInputElement).value)}
                />
              </div>
              <div>
                <span class={labelCls}>Input port</span>
                <PickerField
                  value={(app.input_port as string) ?? ''}
                  options={midiInputOptions}
                  allowFree
                  placeholder="Select…"
                  on:change={(e) => patchApp(i, 'input_port', e.detail)}
                />
              </div>
              <div>
                <span class={labelCls}>Output port</span>
                <PickerField
                  value={(app.output_port as string) ?? ''}
                  options={midiOutputOptions}
                  allowFree
                  placeholder="Select…"
                  on:change={(e) => patchApp(i, 'output_port', e.detail)}
                />
              </div>
            </div>
          </div>
        {/each}
        <button
          class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm"
          on:click={addApp}
        >
          + Add app
        </button>
      </div>
    </div>

    <!-- OBS -->
    <div class="space-y-3">
      <div class={subHeader}>OBS</div>
      <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div class="md:col-span-1">
          <span class={labelCls}>Host</span>
          <input
            type="text"
            class={inputCls}
            value={(obs.host as string) ?? ''}
            on:input={(e) => setObs('host', (e.currentTarget as HTMLInputElement).value)}
          />
        </div>
        <div>
          <span class={labelCls}>Port</span>
          <input
            type="number"
            class={inputCls}
            value={(obs.port as number | null) ?? ''}
            on:input={(e) => {
              const v = (e.currentTarget as HTMLInputElement).value;
              setObs('port', v === '' ? null : Number(v));
            }}
          />
        </div>
        <div>
          <span class={labelCls}>Password</span>
          <div class="flex items-stretch gap-1">
            {#if showPassword}
              <input
                type="text"
                class={inputCls}
                value={(obs.password as string) ?? ''}
                on:input={(e) => setObs('password', (e.currentTarget as HTMLInputElement).value)}
              />
            {:else}
              <input
                type="password"
                class={inputCls}
                value={(obs.password as string) ?? ''}
                on:input={(e) => setObs('password', (e.currentTarget as HTMLInputElement).value)}
              />
            {/if}
            <button
              type="button"
              class="px-2 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-xs"
              on:click={() => (showPassword = !showPassword)}
              title={showPassword ? 'Hide' : 'Show'}
            >
              {showPassword ? 'Hide' : 'Show'}
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</section>
