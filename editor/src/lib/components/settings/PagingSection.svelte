<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { configSchema } from '$lib/schema';
  import SchemaField from '../SchemaField.svelte';

  $: cfg = $profile.parsed as Record<string, unknown> | null;
  $: paging = (cfg?.paging as Record<string, unknown> | undefined) ?? {};
  $: tray = cfg?.tray;

  const inputCls =
    'w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm';
  const labelCls = 'block text-xs text-slate-700 dark:text-slate-400 mb-1';
  const subHeader = 'text-xs uppercase tracking-wide text-slate-500 dark:text-slate-400 font-semibold';

  function resolveRef(s: Record<string, unknown>): Record<string, unknown> {
    if (s.$ref && typeof s.$ref === 'string') {
      const parts = (s.$ref as string).replace('#/', '').split('/');
      let cur: unknown = configSchema;
      for (const p of parts) cur = (cur as Record<string, unknown>)[p];
      return resolveRef(cur as Record<string, unknown>);
    }
    if (Array.isArray(s.anyOf)) {
      const nn = (s.anyOf as Record<string, unknown>[]).find((x) => x.type !== 'null' && !!x);
      if (nn) return resolveRef(nn);
    }
    return s;
  }

  $: traySchema = (() => {
    const props = (configSchema as { properties?: Record<string, unknown> }).properties ?? {};
    const raw = props.tray;
    return raw ? resolveRef(raw as Record<string, unknown>) : null;
  })();

  function setPaging(key: string, val: unknown): void {
    profileActions.patchParsed((c) => {
      const p = ((c as Record<string, unknown>).paging as Record<string, unknown>) ?? {};
      p[key] = val;
      (c as Record<string, unknown>).paging = p;
    });
  }

  function setTray(val: unknown): void {
    profileActions.patchParsed((c) => {
      (c as Record<string, unknown>).tray = val;
    });
  }

  function numInput(e: Event): number | null {
    const v = (e.currentTarget as HTMLInputElement).value;
    return v === '' ? null : Number(v);
  }
</script>

<section
  class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30"
>
  <header class="px-4 py-2 border-b border-slate-200 dark:border-slate-800 text-sm font-semibold">
    Paging & Tray
  </header>
  <div class="p-4 grid grid-cols-1 md:grid-cols-2 gap-6">
    <div class="space-y-3">
      <div class={subHeader}>Paging</div>
      <div>
        <span class={labelCls}>MIDI channel (1-16)</span>
        <input
          type="number"
          min="1"
          max="16"
          class={inputCls}
          value={(paging.channel as number | null) ?? ''}
          on:input={(e) => setPaging('channel', numInput(e))}
        />
      </div>
      <div>
        <span class={labelCls}>Previous note (0-127)</span>
        <input
          type="number"
          min="0"
          max="127"
          class={inputCls}
          value={(paging.prev_note as number | null) ?? ''}
          on:input={(e) => setPaging('prev_note', numInput(e))}
        />
      </div>
      <div>
        <span class={labelCls}>Next note (0-127)</span>
        <input
          type="number"
          min="0"
          max="127"
          class={inputCls}
          value={(paging.next_note as number | null) ?? ''}
          on:input={(e) => setPaging('next_note', numInput(e))}
        />
      </div>
    </div>

    <div class="space-y-3">
      <div class={subHeader}>Tray</div>
      {#if traySchema}
        <SchemaField
          schema={traySchema}
          value={tray}
          path="tray"
          on:change={(e) => setTray(e.detail)}
        />
      {:else}
        <div class="text-xs text-slate-500 italic">No tray schema available.</div>
      {/if}
    </div>
  </div>
</section>
