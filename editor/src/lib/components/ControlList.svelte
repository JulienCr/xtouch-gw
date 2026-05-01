<script lang="ts">
  import { profile } from '$lib/stores/profile';
  import { selectedControl, selectedPage } from '$lib/stores/selection';
  import type { AppConfig } from '$lib/generated/types';

  interface Row {
    pageIdx: number;
    pageName: string;
    controlId: string;
    app: string;
    action: string;
    paramsStr: string;
    feedback: boolean;
  }

  let search = '';
  let sortKey: keyof Row = 'pageName';
  let sortAsc = true;
  let selected = new Set<string>();
  let bulkParam = 'action';
  let bulkValue = '';

  $: rows = buildRows($profile.parsed);
  $: filtered = filterRows(rows, search);
  $: sorted = sortRows(filtered, sortKey, sortAsc);

  function buildRows(cfg: AppConfig | null): Row[] {
    if (!cfg) return [];
    const out: Row[] = [];
    cfg.pages?.forEach((page, idx) => {
      const ctrls = (page as { controls?: Record<string, unknown> }).controls ?? {};
      Object.entries(ctrls).forEach(([cid, raw]) => {
        const m = (raw ?? {}) as { app?: string; action?: string; params?: unknown[]; indicator?: unknown };
        out.push({
          pageIdx: idx,
          pageName: page.name,
          controlId: cid,
          app: m.app ?? '',
          action: m.action ?? '',
          paramsStr: m.params ? JSON.stringify(m.params) : '',
          feedback: !!m.indicator
        });
      });
    });
    return out;
  }

  function filterRows(rs: Row[], q: string): Row[] {
    const s = q.trim().toLowerCase();
    if (!s) return rs;
    return rs.filter((r) =>
      [r.pageName, r.controlId, r.app, r.action, r.paramsStr].some((v) => v.toLowerCase().includes(s))
    );
  }

  function sortRows(rs: Row[], key: keyof Row, asc: boolean): Row[] {
    const copy = [...rs];
    copy.sort((a, b) => {
      const av = a[key];
      const bv = b[key];
      if (av === bv) return 0;
      const cmp = av < bv ? -1 : 1;
      return asc ? cmp : -cmp;
    });
    return copy;
  }

  function toggleSort(key: keyof Row): void {
    if (sortKey === key) sortAsc = !sortAsc;
    else {
      sortKey = key;
      sortAsc = true;
    }
  }

  function rowKey(r: Row): string {
    return `${r.pageIdx}:${r.controlId}`;
  }

  function openMapping(r: Row): void {
    selectedPage.set(r.pageIdx);
    selectedControl.set(r.controlId);
  }

  function toggleSel(r: Row, ev: Event): void {
    ev.stopPropagation();
    const k = rowKey(r);
    if (selected.has(k)) selected.delete(k);
    else selected.add(k);
    selected = new Set(selected);
  }

  function applyBulk(): void {
    if (!bulkParam || selected.size === 0) return;
    profile.update((p) => {
      if (!p.parsed) return p;
      const cloned = JSON.parse(JSON.stringify(p.parsed));
      for (const k of selected) {
        const [pageIdx, ctrl] = k.split(':');
        const c = cloned.pages?.[+pageIdx]?.controls?.[ctrl];
        if (!c) continue;
        if (bulkParam === 'app') c.app = bulkValue;
        else if (bulkParam === 'action') c.action = bulkValue;
      }
      // re-yamlize via profileActions to trigger validation
      return { ...p, parsed: cloned };
    });
    // Trigger setBody by re-serializing
    import('js-yaml').then((y) => {
      profile.update((p) => {
        if (!p.parsed) return p;
        const body = y.default.dump(p.parsed, { noRefs: true, lineWidth: 120 });
        return { ...p, body };
      });
    });
  }
</script>

<div class="p-3 space-y-3">
  <div class="flex items-center gap-2">
    <input
      type="search"
      bind:value={search}
      placeholder="Search controls…"
      class="flex-1 px-3 py-1.5 rounded bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 text-sm"
    />
    <span class="text-xs text-slate-500 dark:text-slate-400">{sorted.length} / {rows.length} controls</span>
  </div>

  {#if selected.size > 0}
    <div class="flex items-center gap-2 p-2 rounded bg-slate-100/70 border border-slate-200 dark:bg-slate-800/60 dark:border-slate-700 text-sm">
      <span class="text-slate-700 dark:text-slate-300">{selected.size} selected</span>
      <select bind:value={bulkParam} class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-900 dark:border-slate-700 dark:text-slate-100 rounded px-2 py-1 text-xs">
        <option value="app">app</option>
        <option value="action">action</option>
      </select>
      <input
        bind:value={bulkValue}
        placeholder="new value"
        class="flex-1 bg-white border border-slate-300 text-slate-900 placeholder-slate-400 dark:bg-slate-900 dark:border-slate-700 dark:text-slate-100 dark:placeholder-slate-500 rounded px-2 py-1 text-xs"
      />
      <button class="px-3 py-1 rounded bg-accent text-slate-900 text-xs font-semibold" on:click={applyBulk}>
        Apply
      </button>
      <button class="px-2 py-1 rounded text-xs text-slate-500 dark:text-slate-400" on:click={() => (selected = new Set())}>
        Clear
      </button>
    </div>
  {/if}

  <div class="overflow-auto rounded border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
    <table class="w-full text-sm">
      <thead class="bg-slate-100 text-slate-700 dark:bg-slate-900 dark:text-slate-400 text-left">
        <tr>
          <th class="w-8 px-2 py-1.5"></th>
          {#each [['pageName', 'Page'], ['controlId', 'Control'], ['app', 'App'], ['action', 'Action'], ['paramsStr', 'Params'], ['feedback', 'Feedback']] as [k, label]}
            <th class="px-2 py-1.5 cursor-pointer hover:text-slate-900 dark:hover:text-slate-200" on:click={() => toggleSort(k as keyof Row)}>
              {label}{sortKey === k ? (sortAsc ? ' ▲' : ' ▼') : ''}
            </th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each sorted as r (rowKey(r))}
          <tr
            class="border-t border-slate-200 hover:bg-slate-100 dark:border-slate-800 dark:hover:bg-slate-900/60 cursor-pointer"
            class:bg-slate-100={$selectedControl === r.controlId && $selectedPage === r.pageIdx}
            class:dark:bg-slate-800={$selectedControl === r.controlId && $selectedPage === r.pageIdx}
            on:click={() => openMapping(r)}
          >
            <td class="px-2 py-1.5">
              <input type="checkbox" checked={selected.has(rowKey(r))} on:click={(e) => toggleSel(r, e)} />
            </td>
            <td class="px-2 py-1.5 text-slate-500 dark:text-slate-400">{r.pageName}</td>
            <td class="px-2 py-1.5 font-mono text-xs">{r.controlId}</td>
            <td class="px-2 py-1.5">{r.app}</td>
            <td class="px-2 py-1.5">{r.action}</td>
            <td class="px-2 py-1.5 font-mono text-xs text-slate-500 dark:text-slate-400 truncate max-w-xs">{r.paramsStr}</td>
            <td class="px-2 py-1.5">
              {#if r.feedback}<span class="text-emerald-600 dark:text-emerald-400">●</span>{:else}<span class="text-slate-400 dark:text-slate-600">—</span>{/if}
            </td>
          </tr>
        {:else}
          <tr><td colspan="7" class="px-2 py-6 text-center text-slate-500 dark:text-slate-400">No controls. Load a profile.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>
