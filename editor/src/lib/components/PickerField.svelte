<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let value: string = '';
  export let options: { value: string; label: string; meta?: string }[] = [];
  export let placeholder = 'Select…';
  export let disabled = false;
  export let allowFree = false;

  const dispatch = createEventDispatcher<{ change: string }>();

  let open = false;
  let search = '';
  let highlightIdx = 0;
  let inputEl: HTMLInputElement | null = null;

  // Fuzzy subsequence match with scoring. Returns null if not all query chars
  // appear in order; otherwise a score where higher = better (consecutive
  // matches, matches at word boundaries, and earlier matches all rank higher).
  function fuzzyScore(text: string, q: string): number | null {
    if (!q) return 0;
    const t = text.toLowerCase();
    let ti = 0;
    let qi = 0;
    let score = 0;
    let prevMatched = false;
    let firstMatchAt = -1;
    while (ti < t.length && qi < q.length) {
      if (t[ti] === q[qi]) {
        if (firstMatchAt < 0) firstMatchAt = ti;
        if (prevMatched) score += 5;
        const prev = ti > 0 ? t[ti - 1] : '';
        if (ti === 0 || prev === ' ' || prev === '_' || prev === '-' || prev === '.') score += 3;
        score += 1;
        prevMatched = true;
        qi++;
      } else {
        prevMatched = false;
      }
      ti++;
    }
    if (qi < q.length) return null;
    score -= firstMatchAt * 0.1;
    score -= (text.length - q.length) * 0.01;
    return score;
  }

  $: filtered = (() => {
    const q = search.trim().toLowerCase();
    if (!q) return options;
    const scored: { opt: (typeof options)[number]; score: number }[] = [];
    for (const o of options) {
      const s1 = fuzzyScore(o.label, q);
      const s2 = fuzzyScore(o.value, q);
      const best = s1 === null ? s2 : s2 === null ? s1 : Math.max(s1, s2);
      if (best !== null) scored.push({ opt: o, score: best });
    }
    scored.sort((a, b) => b.score - a.score);
    return scored.map((s) => s.opt);
  })();

  $: if (search !== undefined) highlightIdx = 0;

  $: missing = value && !options.some((o) => o.value === value);

  function pick(v: string): void {
    value = v;
    open = false;
    search = '';
    dispatch('change', v);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightIdx = Math.min(filtered.length - 1, highlightIdx + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightIdx = Math.max(0, highlightIdx - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const opt = filtered[highlightIdx];
      if (opt) pick(opt.value);
      else if (allowFree && search) pick(search);
    } else if (e.key === 'Escape') {
      open = false;
      search = '';
    }
  }

  function focus(): void {
    open = true;
    setTimeout(() => inputEl?.focus(), 0);
  }
</script>

<div class="relative">
  <button
    type="button"
    class="w-full flex items-center justify-between px-2 py-1.5 rounded bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 text-left text-sm hover:border-slate-400 dark:hover:border-slate-500 disabled:opacity-50"
    class:border-amber-500={missing}
    {disabled}
    on:click={focus}
  >
    <span class="truncate">
      {#if value}
        {options.find((o) => o.value === value)?.label ?? value}
      {:else}
        <span class="text-slate-400 dark:text-slate-500">{placeholder}</span>
      {/if}
    </span>
    {#if missing}
      <span class="ml-2 text-xs px-1.5 py-0.5 rounded bg-amber-100 text-amber-700 border border-amber-300 dark:bg-amber-900/40 dark:text-amber-300 dark:border-amber-700">missing</span>
    {/if}
    <span class="ml-2 text-slate-500 dark:text-slate-400 text-xs">▾</span>
  </button>

  {#if open}
    <div class="absolute z-50 mt-1 w-full max-h-72 overflow-auto rounded border border-slate-200 bg-white dark:border-slate-700 dark:bg-slate-900 shadow-xl">
      <input
        bind:this={inputEl}
        bind:value={search}
        on:keydown={onKey}
        placeholder="Search…"
        class="w-full px-2 py-1.5 text-sm bg-slate-50 text-slate-900 placeholder-slate-400 dark:bg-slate-950 dark:text-slate-100 dark:placeholder-slate-500 border-b border-slate-200 dark:border-slate-800 outline-none"
      />
      <ul class="py-1">
        {#if allowFree && search && !filtered.some((o) => o.value === search)}
          <li>
            <button
              type="button"
              class="w-full text-left px-2 py-1.5 text-sm hover:bg-slate-100 dark:hover:bg-slate-800 text-amber-600 dark:text-amber-300"
              on:click={() => pick(search)}
            >
              Use "{search}"
            </button>
          </li>
        {/if}
        {#each filtered as opt, i}
          <li>
            <button
              type="button"
              class="w-full text-left px-2 py-1.5 text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
              class:bg-slate-100={i === highlightIdx}
              class:dark:bg-slate-800={i === highlightIdx}
              class:text-accent={opt.value === value}
              on:click={() => pick(opt.value)}
              on:mouseenter={() => (highlightIdx = i)}
            >
              <div class="truncate">{opt.label}</div>
              {#if opt.meta}
                <div class="text-xs text-slate-500 dark:text-slate-400 truncate">{opt.meta}</div>
              {/if}
            </button>
          </li>
        {:else}
          <li class="px-2 py-2 text-xs text-slate-500">No matches</li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

{#if open}
  <button
    aria-label="Close"
    class="fixed inset-0 z-40 cursor-default"
    on:click={() => (open = false)}
  ></button>
{/if}
