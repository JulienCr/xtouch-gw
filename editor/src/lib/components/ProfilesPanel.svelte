<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ProfileMeta } from '$lib/api';
  import { profileActions } from '$lib/stores/profile';
  import { pushToast } from '$lib/stores/toasts';

  let profiles: ProfileMeta[] = [];
  let activeName: string | null = null;
  let loading = false;
  let error: string | null = null;
  let confirmDelete: string | null = null;

  async function refresh(): Promise<void> {
    loading = true;
    error = null;
    try {
      const [list, active] = await Promise.all([
        api.profiles.list(),
        api.profiles.active().catch(() => ({ name: '' }))
      ]);
      profiles = list;
      activeName = active?.name ?? null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function newProfile(): Promise<void> {
    const name = prompt('New profile name?');
    if (!name) return;
    try {
      await api.profiles.create(name);
      pushToast('success', `Created ${name}`);
      await refresh();
    } catch (e) {
      pushToast('error', `Create failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function dup(name: string): Promise<void> {
    const newName = prompt('Duplicate as?', `${name}-copy`);
    if (!newName) return;
    try {
      await api.profiles.duplicate(name, newName);
      pushToast('success', `Duplicated ${name} → ${newName}`);
      await refresh();
    } catch (e) {
      pushToast('error', `Duplicate failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function rename(name: string): Promise<void> {
    const newName = prompt('Rename to?', name);
    if (!newName || newName === name) return;
    try {
      await api.profiles.rename(name, newName);
      pushToast('success', `Renamed ${name} → ${newName}`);
      await refresh();
    } catch (e) {
      pushToast('error', `Rename failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function del(name: string): Promise<void> {
    try {
      await api.profiles.delete(name);
      pushToast('success', `Deleted ${name}`);
      confirmDelete = null;
      await refresh();
    } catch (e) {
      pushToast('error', `Delete failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function activate(name: string): Promise<void> {
    try {
      await api.profiles.activate(name);
      pushToast('success', `Activated ${name}`);
      activeName = name;
      await profileActions.load(name);
    } catch (e) {
      pushToast('error', `Activate failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  function open(name: string): void {
    profileActions.load(name);
  }

  onMount(refresh);
</script>

<div class="p-3 space-y-3">
  <div class="flex items-center gap-2">
    <button class="px-3 py-1.5 rounded bg-accent text-slate-900 text-sm font-semibold" on:click={newProfile}>New</button>
    <button class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700 text-sm" on:click={refresh}>Refresh</button>
    {#if loading}<span class="text-xs text-slate-500 dark:text-slate-400">loading…</span>{/if}
  </div>

  {#if error}
    <div class="p-2 rounded bg-rose-100 border border-rose-300 text-sm text-rose-700 dark:bg-rose-950/40 dark:border-rose-800 dark:text-rose-300">{error}</div>
  {/if}

  <div class="rounded border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30 overflow-hidden">
    <table class="w-full text-sm">
      <thead class="bg-slate-100 text-left text-slate-700 dark:bg-slate-900 dark:text-slate-400">
        <tr>
          <th class="px-3 py-1.5">Name</th>
          <th class="px-3 py-1.5">Modified</th>
          <th class="px-3 py-1.5 w-72"></th>
        </tr>
      </thead>
      <tbody>
        {#each profiles as p}
          <tr class="border-t border-slate-200 dark:border-slate-800">
            <td class="px-3 py-1.5">
              <button class="hover:text-accent text-left" on:click={() => open(p.name)}>{p.name}</button>
              {#if p.name === activeName}
                <span class="ml-2 text-xs px-1.5 py-0.5 rounded bg-emerald-100 text-emerald-700 border border-emerald-300 dark:bg-emerald-900/40 dark:text-emerald-300 dark:border-emerald-700">active</span>
              {/if}
            </td>
            <td class="px-3 py-1.5 text-xs text-slate-500 dark:text-slate-400 font-mono">{p.mtime ?? ''}</td>
            <td class="px-3 py-1.5 text-xs">
              <button class="px-2 py-1 rounded border border-slate-300 dark:border-slate-700 mr-1" on:click={() => activate(p.name)}>Activate</button>
              <button class="px-2 py-1 rounded border border-slate-300 dark:border-slate-700 mr-1" on:click={() => dup(p.name)}>Duplicate</button>
              <button class="px-2 py-1 rounded border border-slate-300 dark:border-slate-700 mr-1" on:click={() => rename(p.name)}>Rename</button>
              <button class="px-2 py-1 rounded text-rose-700 hover:bg-rose-100 dark:text-rose-300 dark:hover:bg-rose-950/30" on:click={() => (confirmDelete = p.name)}>Delete</button>
            </td>
          </tr>
        {:else}
          <tr><td colspan="3" class="px-3 py-6 text-center text-slate-500 dark:text-slate-400">No profiles found.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>

  {#if confirmDelete}
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div class="bg-white text-slate-900 border border-slate-200 dark:bg-slate-900 dark:text-slate-100 dark:border-slate-700 rounded-lg p-4 max-w-md w-full space-y-3">
        <h3 class="font-semibold">Delete "{confirmDelete}"?</h3>
        <p class="text-sm text-slate-500 dark:text-slate-400">This cannot be undone (history is also deleted).</p>
        <div class="flex justify-end gap-2">
          <button class="px-3 py-1.5 rounded border border-slate-300 dark:border-slate-700" on:click={() => (confirmDelete = null)}>Cancel</button>
          <button class="px-3 py-1.5 rounded bg-rose-700 text-white" on:click={() => del(confirmDelete!)}>Delete</button>
        </div>
      </div>
    </div>
  {/if}
</div>
