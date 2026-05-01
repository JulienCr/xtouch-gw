<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { selectedPage } from '$lib/stores/selection';

  export let stripIdx: number; // 1-based

  const COLORS: Array<{ id: number; name: string; bg: string; fg: string }> = [
    { id: 0, name: 'off', bg: '#1f2937', fg: '#9ca3af' },
    { id: 1, name: 'red', bg: '#dc2626', fg: '#fff' },
    { id: 2, name: 'green', bg: '#16a34a', fg: '#fff' },
    { id: 3, name: 'yellow', bg: '#eab308', fg: '#000' },
    { id: 4, name: 'blue', bg: '#2563eb', fg: '#fff' },
    { id: 5, name: 'magenta', bg: '#c026d3', fg: '#fff' },
    { id: 6, name: 'cyan', bg: '#06b6d4', fg: '#000' },
    { id: 7, name: 'white', bg: '#f3f4f6', fg: '#000' }
  ];

  $: pageIdx = $selectedPage;
  $: page = (pageIdx >= 0 ? ($profile.parsed?.pages?.[pageIdx] ?? null) : null) as
    | { lcd?: { labels?: string[]; colors?: number[] } }
    | null;

  $: i0 = stripIdx - 1;
  $: label = (page?.lcd?.labels?.[i0] ?? '') as string;
  $: colorId = (page?.lcd?.colors?.[i0] ?? 0) as number;
  $: [line1, line2] = (() => {
    const parts = (label ?? '').split('\n');
    return [parts[0] ?? '', parts[1] ?? ''];
  })();

  function setLines(l1: string, l2: string): void {
    const next = l2 ? `${l1}\n${l2}` : l1;
    profileActions.patchParsed((cfg) => {
      const p = cfg.pages?.[pageIdx];
      if (!p) return;
      if (!p.lcd) p.lcd = { labels: [], colors: [] };
      if (!p.lcd.labels) p.lcd.labels = [];
      while (p.lcd.labels.length < 8) p.lcd.labels.push('');
      p.lcd.labels[i0] = next;
    });
  }

  function setColor(v: number): void {
    profileActions.patchParsed((cfg) => {
      const p = cfg.pages?.[pageIdx];
      if (!p) return;
      if (!p.lcd) p.lcd = { labels: [], colors: [] };
      if (!p.lcd.colors) p.lcd.colors = [];
      while (p.lcd.colors.length < 8) p.lcd.colors.push(0);
      p.lcd.colors[i0] = v;
    });
  }
</script>

<div class="space-y-3 text-sm">
  <div>
    <div class="text-xs text-slate-500 dark:text-slate-400">LCD strip</div>
    <div class="font-mono text-sm">lcd{stripIdx}</div>
  </div>

  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Line 1</div>
    <input
      class="w-full bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 rounded px-2 py-1.5 font-mono text-sm"
      maxlength="7"
      value={line1}
      on:input={(e) => setLines((e.currentTarget as HTMLInputElement).value, line2)}
    />
  </div>

  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Line 2</div>
    <input
      class="w-full bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 rounded px-2 py-1.5 font-mono text-sm"
      maxlength="7"
      value={line2}
      on:input={(e) => setLines(line1, (e.currentTarget as HTMLInputElement).value)}
    />
  </div>

  <div>
    <div class="text-xs text-slate-700 dark:text-slate-400 mb-1">Color</div>
    <div class="flex flex-wrap gap-1.5">
      {#each COLORS as opt}
        <button
          type="button"
          class="w-7 h-7 rounded border-2"
          class:border-sky-400={colorId === opt.id}
          class:border-transparent={colorId !== opt.id}
          style="background:{opt.bg}"
          title={opt.name}
          on:click={() => setColor(opt.id)}
        ></button>
      {/each}
    </div>
  </div>
</div>
