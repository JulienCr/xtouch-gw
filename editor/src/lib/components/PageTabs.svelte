<script lang="ts">
  import { profile, profileActions } from '$lib/stores/profile';
  import { selectedPage } from '$lib/stores/selection';
  import { getServerPageIndex } from '$lib/stores/live';
  import { api } from '$lib/api';
  import type { PageConfig } from '$lib/generated/types';

  let dragFrom: number | null = null;
  let dragOver: number | null = null;
  let renamingIdx: number | null = null;
  let renameValue = '';
  let menuIdx: number | null = null;

  $: pages = ($profile.parsed?.pages ?? []) as PageConfig[];

  function selectGlobal(): void {
    selectedPage.set(-1);
  }

  function select(i: number): void {
    selectedPage.set(i);
    // Mirror the selection on the X-Touch. Skip when the server is already
    // on this page (e.g. we just received a page_changed event) to avoid
    // a redundant round-trip.
    if (getServerPageIndex() !== i) {
      api.page.setActive(i).catch(() => { /* server not wired or invalid index */ });
    }
  }

  function addPage(): void {
    profileActions.patchParsed((cfg) => {
      if (!cfg.pages) cfg.pages = [];
      const n = cfg.pages.length + 1;
      cfg.pages.push({ name: `Page ${n}`, controls: {} } as PageConfig);
    });
    // Select the newly created page after the patch flushes through.
    setTimeout(() => selectedPage.set((($profile.parsed?.pages ?? []).length - 1)), 0);
  }

  function startRename(i: number): void {
    renamingIdx = i;
    renameValue = pages[i]?.name ?? '';
    menuIdx = null;
  }

  function commitRename(): void {
    if (renamingIdx === null) return;
    const i = renamingIdx;
    const name = renameValue.trim() || pages[i]?.name || `Page ${i + 1}`;
    profileActions.patchParsed((cfg) => {
      const p = cfg.pages?.[i];
      if (p) p.name = name;
    });
    renamingIdx = null;
  }

  function cancelRename(): void {
    renamingIdx = null;
  }

  function onRenameKey(e: KeyboardEvent): void {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitRename();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelRename();
    }
  }

  function duplicate(i: number): void {
    profileActions.patchParsed((cfg) => {
      if (!cfg.pages) return;
      const src = cfg.pages[i];
      if (!src) return;
      const copy = JSON.parse(JSON.stringify(src)) as PageConfig;
      copy.name = `${src.name} copy`;
      cfg.pages.splice(i + 1, 0, copy);
    });
    menuIdx = null;
  }

  function move(i: number, delta: number): void {
    const j = i + delta;
    if (j < 0 || j >= pages.length) return;
    profileActions.patchParsed((cfg) => {
      if (!cfg.pages) return;
      const [item] = cfg.pages.splice(i, 1);
      cfg.pages.splice(j, 0, item);
    });
    // Keep the same logical page focused after move.
    if ($selectedPage === i) selectedPage.set(j);
    else if ($selectedPage === j) selectedPage.set(i);
    menuIdx = null;
  }

  function remove(i: number): void {
    const name = pages[i]?.name ?? `Page ${i + 1}`;
    if (!confirm(`Delete page "${name}"? This cannot be undone.`)) return;
    profileActions.patchParsed((cfg) => {
      if (!cfg.pages) return;
      cfg.pages.splice(i, 1);
    });
    // Adjust selection if the removed page was selected or before it.
    const cur = $selectedPage;
    if (cur === i) selectedPage.set(Math.max(0, i - 1));
    else if (cur > i && cur > 0) selectedPage.set(cur - 1);
    menuIdx = null;
  }

  function onDragStart(e: DragEvent, i: number): void {
    dragFrom = i;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', String(i));
    }
  }

  function onDragOver(e: DragEvent, i: number): void {
    if (dragFrom === null) return;
    e.preventDefault();
    dragOver = i;
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
  }

  function onDrop(e: DragEvent, i: number): void {
    e.preventDefault();
    const from = dragFrom;
    dragFrom = null;
    dragOver = null;
    if (from === null || from === i) return;
    profileActions.patchParsed((cfg) => {
      if (!cfg.pages) return;
      const [item] = cfg.pages.splice(from, 1);
      cfg.pages.splice(i, 0, item);
    });
    if ($selectedPage === from) selectedPage.set(i);
    else if (from < $selectedPage && i >= $selectedPage) selectedPage.set($selectedPage - 1);
    else if (from > $selectedPage && i <= $selectedPage) selectedPage.set($selectedPage + 1);
  }

  function onDragEnd(): void {
    dragFrom = null;
    dragOver = null;
  }

  function openMenu(e: MouseEvent, i: number): void {
    e.preventDefault();
    e.stopPropagation();
    menuIdx = menuIdx === i ? null : i;
  }

  function closeMenu(): void {
    menuIdx = null;
  }
</script>

<div class="flex flex-wrap items-center gap-x-1 gap-y-1 px-2 py-1.5 border-b border-slate-200 dark:border-slate-800 bg-white/60 dark:bg-slate-900/40">
  <!-- Pinned global tab -->
  <button
    type="button"
    class="shrink-0 px-2 py-0.5 rounded-full text-[11px] font-medium transition-colors border border-slate-300 dark:border-slate-700 whitespace-nowrap"
    class:bg-accent={$selectedPage === -1}
    class:text-slate-900={$selectedPage === -1}
    class:text-slate-600={$selectedPage !== -1}
    class:dark:text-slate-300={$selectedPage !== -1}
    class:hover:bg-slate-100={$selectedPage !== -1}
    class:dark:hover:bg-slate-800={$selectedPage !== -1}
    title="All pages (global controls)"
    on:click={selectGlobal}
  >
    ⋆ All pages
  </button>

  <div class="shrink-0 w-px h-5 bg-slate-300 dark:bg-slate-700 mx-1"></div>

  {#each pages as page, i (i)}
    <div
      class="relative shrink-0 group flex items-center"
      class:opacity-50={dragFrom === i}
    >
      <div
        role="tab"
        tabindex="0"
        draggable={renamingIdx !== i}
        on:dragstart={(e) => onDragStart(e, i)}
        on:dragover={(e) => onDragOver(e, i)}
        on:drop={(e) => onDrop(e, i)}
        on:dragend={onDragEnd}
        on:contextmenu={(e) => openMenu(e, i)}
        class="flex items-center gap-1 pl-2 pr-0.5 py-0.5 rounded-full text-[11px] font-medium border transition-colors cursor-pointer whitespace-nowrap"
        class:bg-accent={$selectedPage === i}
        class:text-slate-900={$selectedPage === i}
        class:border-accent={$selectedPage === i}
        class:text-slate-600={$selectedPage !== i}
        class:dark:text-slate-300={$selectedPage !== i}
        class:hover:bg-slate-100={$selectedPage !== i}
        class:dark:hover:bg-slate-800={$selectedPage !== i}
        class:border-slate-300={$selectedPage !== i}
        class:dark:border-slate-700={$selectedPage !== i}
        class:ring-2={dragOver === i && dragFrom !== i}
        class:ring-accent={dragOver === i && dragFrom !== i}
        on:click={() => select(i)}
        on:keydown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            select(i);
          }
        }}
      >
        {#if renamingIdx === i}
          <!-- svelte-ignore a11y-autofocus -->
          <input
            class="bg-white dark:bg-slate-900 text-slate-900 dark:text-slate-100 border border-slate-300 dark:border-slate-700 rounded px-1 py-0.5 text-xs w-28"
            bind:value={renameValue}
            autofocus
            on:keydown={onRenameKey}
            on:blur={commitRename}
            on:click|stopPropagation
          />
        {:else}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <span class="pr-1.5" role="presentation" on:dblclick|stopPropagation={() => startRename(i)}>{page.name || `Page ${i + 1}`}</span>
        {/if}
      </div>

      {#if menuIdx === i}
        <div
          class="absolute z-50 top-full left-0 mt-1 min-w-[10rem] rounded border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-900 shadow-xl py-1 text-xs"
        >
          <button class="w-full text-left px-3 py-1.5 hover:bg-slate-100 dark:hover:bg-slate-800" on:click={() => startRename(i)}>Rename</button>
          <button class="w-full text-left px-3 py-1.5 hover:bg-slate-100 dark:hover:bg-slate-800" on:click={() => duplicate(i)}>Duplicate</button>
          <button class="w-full text-left px-3 py-1.5 hover:bg-slate-100 dark:hover:bg-slate-800 disabled:opacity-40" disabled={i === 0} on:click={() => move(i, -1)}>Move left</button>
          <button class="w-full text-left px-3 py-1.5 hover:bg-slate-100 dark:hover:bg-slate-800 disabled:opacity-40" disabled={i === pages.length - 1} on:click={() => move(i, 1)}>Move right</button>
          <div class="my-1 h-px bg-slate-200 dark:bg-slate-800"></div>
          <button class="w-full text-left px-3 py-1.5 text-rose-600 dark:text-rose-300 hover:bg-rose-50 dark:hover:bg-rose-950/30" on:click={() => remove(i)}>Delete</button>
        </div>
      {/if}
    </div>
  {/each}

  <button
    type="button"
    class="shrink-0 ml-1 px-2 py-0.5 rounded-full text-[11px] text-slate-500 dark:text-slate-400 hover:bg-slate-100 dark:hover:bg-slate-800 border border-dashed border-slate-300 dark:border-slate-700"
    title="Add page"
    on:click={addPage}
  >+</button>
</div>

{#if menuIdx !== null}
  <!-- click-outside backdrop -->
  <button aria-label="Close menu" class="fixed inset-0 z-40 cursor-default" on:click={closeMenu}></button>
{/if}
