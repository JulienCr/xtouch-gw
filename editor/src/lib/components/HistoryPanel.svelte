<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, type Snapshot, type ProfileMeta } from '$lib/api';
  import { profileActions } from '$lib/stores/profile';
  import { pushToast } from '$lib/stores/toasts';

  let profiles: ProfileMeta[] = [];
  let activeProfile = '';
  let snapshots: Snapshot[] = [];
  let selected: Snapshot | null = null;
  let snapshotBody = '';
  let currentBody = '';
  let diffEl: HTMLDivElement | null = null;
  let diffEditor: { dispose(): void; setModel: (m: unknown) => void } | null = null;
  let confirmRestore: Snapshot | null = null;

  async function refreshProfiles(): Promise<void> {
    profiles = await api.profiles.list().catch(() => []);
    if (!activeProfile && profiles.length) activeProfile = profiles[0].name;
  }

  $: if (activeProfile) refreshSnapshots();

  async function refreshSnapshots(): Promise<void> {
    try {
      snapshots = await api.profiles.history(activeProfile);
      const cur = await api.profiles.get(activeProfile).catch(() => ({ body: '' }));
      currentBody = cur.body;
    } catch (e) {
      pushToast('error', `History load failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function pick(s: Snapshot): Promise<void> {
    selected = s;
    try {
      const r = await api.profiles.historyRead(activeProfile, s.timestamp);
      snapshotBody = r.body;
      mountDiff();
    } catch (e) {
      pushToast('error', `Snapshot load failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  // Re-emit YAML through a canonical dump so the diff ignores cosmetic
  // changes (comments, quote style, indentation, key order). What the user
  // sees is the data-level delta, not the textual one.
  async function normalizeYaml(body: string): Promise<string> {
    const yaml = (await import('js-yaml')).default;
    try {
      const parsed = yaml.load(body);
      return yaml.dump(parsed ?? {}, {
        noRefs: true,
        sortKeys: true,
        lineWidth: 120,
        noCompatMode: true,
        quotingType: '"'
      });
    } catch {
      return body;
    }
  }

  async function mountDiff(): Promise<void> {
    if (!diffEl) return;
    const monaco = await import('monaco-editor');
    if (!diffEditor) {
      const isDark = typeof document !== 'undefined' && document.documentElement.classList.contains('dark');
      diffEditor = monaco.editor.createDiffEditor(diffEl, {
        readOnly: true,
        renderSideBySide: true,
        theme: isDark ? 'vs-dark' : 'vs',
        automaticLayout: true,
        minimap: { enabled: false }
      }) as unknown as typeof diffEditor;
    }
    const [origText, modText] = await Promise.all([
      normalizeYaml(snapshotBody),
      normalizeYaml(currentBody)
    ]);
    const original = monaco.editor.createModel(origText, 'yaml');
    const modified = monaco.editor.createModel(modText, 'yaml');
    (diffEditor as { setModel: (m: { original: unknown; modified: unknown }) => void }).setModel({
      original,
      modified
    });
  }

  async function restore(s: Snapshot): Promise<void> {
    try {
      await api.profiles.historyRestore(activeProfile, s.timestamp);
      pushToast('success', `Restored ${s.timestamp}`);
      confirmRestore = null;
      await refreshSnapshots();
      await profileActions.load(activeProfile);
    } catch (e) {
      pushToast('error', `Restore failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  onMount(refreshProfiles);
  onDestroy(() => diffEditor?.dispose());
</script>

<div class="p-3 space-y-3">
  <div class="flex items-center gap-2">
    <label class="text-xs text-slate-700 dark:text-slate-400">Profile</label>
    <select bind:value={activeProfile} class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 rounded px-2 py-1 text-sm">
      {#each profiles as p}<option value={p.name}>{p.name}</option>{/each}
    </select>
    <button class="px-2 py-1 text-xs rounded border border-slate-300 dark:border-slate-700" on:click={refreshSnapshots}>Refresh</button>
  </div>

  <div class="grid grid-cols-3 gap-3 min-h-[400px]">
    <div class="col-span-1 rounded border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30 overflow-hidden">
      <div class="px-3 py-1.5 bg-slate-100 text-xs text-slate-700 dark:bg-slate-900 dark:text-slate-400">Snapshots</div>
      <div class="overflow-auto max-h-[600px]">
        {#each snapshots as s}
          <button
            class="w-full text-left px-3 py-2 text-sm border-b border-slate-200 hover:bg-slate-100 dark:border-slate-800 dark:hover:bg-slate-800/60"
            class:bg-slate-100={selected?.timestamp === s.timestamp}
            class:dark:bg-slate-800={selected?.timestamp === s.timestamp}
            on:click={() => pick(s)}
          >
            <div class="font-mono text-xs">{s.timestamp}</div>
            {#if s.size}<div class="text-xs text-slate-500 dark:text-slate-400">{s.size} bytes</div>{/if}
          </button>
        {:else}
          <div class="px-3 py-3 text-sm text-slate-500 dark:text-slate-400">No snapshots.</div>
        {/each}
      </div>
    </div>

    <div class="col-span-2 rounded border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30 overflow-hidden flex flex-col">
      <div class="px-3 py-1.5 bg-slate-100 text-xs text-slate-700 dark:bg-slate-900 dark:text-slate-400 flex items-center gap-2">
        <span>Diff: {selected ? `${selected.timestamp} → current` : '(select a snapshot)'}</span>
        <span class="ml-auto"></span>
        {#if selected}
          <button class="px-2 py-1 text-xs rounded bg-amber-700 hover:bg-amber-600 text-white" on:click={() => (confirmRestore = selected)}>
            Restore
          </button>
        {/if}
      </div>
      <div bind:this={diffEl} class="flex-1 min-h-[400px] bg-white dark:bg-slate-950"></div>
    </div>
  </div>

  {#if confirmRestore}
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div class="bg-white text-slate-900 border border-slate-200 dark:bg-slate-900 dark:text-slate-100 dark:border-slate-700 rounded-lg p-4 max-w-md w-full space-y-3">
        <h3 class="font-semibold">Restore snapshot?</h3>
        <p class="text-sm text-slate-500 dark:text-slate-400">A new snapshot of the current state will be saved before restore.</p>
        <div class="flex justify-end gap-2">
          <button class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700" on:click={() => (confirmRestore = null)}>Cancel</button>
          <button class="px-3 py-1.5 rounded bg-amber-700 text-white" on:click={() => restore(confirmRestore!)}>Restore</button>
        </div>
      </div>
    </div>
  {/if}
</div>
