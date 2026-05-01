<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import Self from './SchemaField.svelte';
  import { configSchema } from '$lib/schema';

  export let schema: Record<string, unknown>;
  export let value: unknown;
  export let path: string;
  export let label: string | null = null;

  const dispatch = createEventDispatcher<{ change: unknown }>();

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
    if (Array.isArray(s.oneOf)) {
      const nn = (s.oneOf as Record<string, unknown>[]).find((x) => x.type !== 'null' && !!x);
      if (nn) return resolveRef(nn);
    }
    return s;
  }

  $: s = resolveRef(schema);
  $: type = inferType(s, value);
  $: enums = (s.enum as unknown[] | undefined) ?? null;

  function inferType(s: Record<string, unknown>, v: unknown): string {
    if (Array.isArray(s.type)) {
      const nonNull = (s.type as string[]).find((t) => t !== 'null');
      return nonNull ?? 'string';
    }
    if (typeof s.type === 'string') return s.type;
    if (Array.isArray(v)) return 'array';
    if (v && typeof v === 'object') return 'object';
    return 'string';
  }

  function emit(v: unknown): void {
    dispatch('change', v);
  }

  function setProp(key: string, v: unknown): void {
    const cur = (value && typeof value === 'object' ? { ...(value as Record<string, unknown>) } : {}) as Record<
      string,
      unknown
    >;
    if (v === undefined || v === null || v === '') delete cur[key];
    else cur[key] = v;
    emit(cur);
  }

  function setIdx(i: number, v: unknown): void {
    const arr = Array.isArray(value) ? [...value] : [];
    arr[i] = v;
    emit(arr);
  }

  function pushItem(): void {
    const arr = Array.isArray(value) ? [...value] : [];
    const itemSchema = resolveRef((s.items as Record<string, unknown>) ?? {});
    arr.push(defaultFor(itemSchema));
    emit(arr);
  }

  function removeItem(i: number): void {
    const arr = Array.isArray(value) ? [...value] : [];
    arr.splice(i, 1);
    emit(arr);
  }

  function defaultFor(s: Record<string, unknown>): unknown {
    const t = inferType(s, undefined);
    if ('default' in s) return s.default;
    if (t === 'object') return {};
    if (t === 'array') return [];
    if (t === 'boolean') return false;
    if (t === 'number' || t === 'integer') return 0;
    return '';
  }

  $: properties = (s.properties as Record<string, Record<string, unknown>> | undefined) ?? {};
  $: required = (s.required as string[] | undefined) ?? [];
  $: propKeys = Object.keys(properties);
</script>

{#if enums && enums.length}
  <select
    class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm"
    value={(value as string | number | null) ?? ''}
    on:change={(e) => emit((e.currentTarget as HTMLSelectElement).value)}
  >
    <option value="" disabled>Select…</option>
    {#each enums as en}
      <option value={en}>{en}</option>
    {/each}
  </select>
{:else if type === 'boolean'}
  <label class="inline-flex items-center gap-2 text-sm">
    <input
      type="checkbox"
      checked={!!value}
      on:change={(e) => emit((e.currentTarget as HTMLInputElement).checked)}
    />
    {label ?? ''}
  </label>
{:else if type === 'number' || type === 'integer'}
  <input
    type="number"
    class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm"
    value={(value as number | null) ?? ''}
    on:input={(e) => {
      const v = (e.currentTarget as HTMLInputElement).value;
      emit(v === '' ? null : Number(v));
    }}
  />
{:else if type === 'array'}
  <div class="space-y-2">
    {#each Array.isArray(value) ? value : [] as item, i}
      <div class="flex items-start gap-2 rounded border border-slate-200 dark:border-slate-800 p-2">
        <div class="flex-1">
          <Self
            schema={(s.items as Record<string, unknown>) ?? {}}
            value={item}
            path={`${path}[${i}]`}
            on:change={(e) => setIdx(i, e.detail)}
          />
        </div>
        <button class="text-xs text-rose-600 hover:bg-rose-100 dark:text-rose-400 dark:hover:bg-rose-950/40 px-2 py-1 rounded" on:click={() => removeItem(i)}>
          Remove
        </button>
      </div>
    {/each}
    <button class="text-xs px-2 py-1 rounded border border-slate-300 dark:border-slate-700" on:click={pushItem}>+ Add</button>
  </div>
{:else if type === 'object' && propKeys.length}
  <div class="space-y-2">
    {#each propKeys as k}
      {@const fieldSchema = properties[k]}
      <div>
        <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">
          {k}{required.includes(k) ? ' *' : ''}
          {#if fieldSchema?.description}
            <span class="text-slate-500 dark:text-slate-600">— {fieldSchema.description}</span>
          {/if}
        </div>
        <Self
          schema={fieldSchema}
          value={(value as Record<string, unknown> | null)?.[k]}
          path={`${path}.${k}`}
          on:change={(e) => setProp(k, e.detail)}
        />
      </div>
    {/each}
  </div>
{:else if type === 'object'}
  <textarea
    class="w-full font-mono text-xs bg-white border border-slate-300 text-slate-900 dark:bg-slate-900 dark:border-slate-700 dark:text-slate-100 rounded p-2 h-24"
    value={value ? JSON.stringify(value, null, 2) : ''}
    on:input={(e) => {
      try {
        emit(JSON.parse((e.currentTarget as HTMLTextAreaElement).value));
      } catch {
        /* ignore */
      }
    }}
  ></textarea>
{:else}
  <input
    type="text"
    class="w-full bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1.5 text-sm"
    value={(value as string | null) ?? ''}
    on:input={(e) => emit((e.currentTarget as HTMLInputElement).value)}
  />
{/if}
