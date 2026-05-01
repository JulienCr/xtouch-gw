<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { base } from '$app/paths';
  import { profile, profileActions, isDirty, totalErrors, totalWarnings } from '$lib/stores/profile';
  import { startLive, live } from '$lib/stores/live';
  import { pickerActions } from '$lib/stores/pickers';
  import { api, type ProfileMeta } from '$lib/api';
  import { pushToast } from '$lib/stores/toasts';
  import Toasts from '$lib/components/Toasts.svelte';

  const links = [
    { href: '/surface', label: 'Surface' },
    { href: '/list', label: 'List' },
    { href: '/settings', label: 'Settings' },
    { href: '/profiles', label: 'Profiles' },
    { href: '/history', label: 'History' }
  ];

  $: currentPath = $page.url.pathname.replace(base, '') || '/';

  let profiles: ProfileMeta[] = [];
  let activeName = '';
  let theme: 'dark' | 'light' = 'light';

  async function bootstrap(): Promise<void> {
    startLive();
    pickerActions.refresh();
    try {
      const list = await api.profiles.list();
      profiles = list;
      const active = await api.profiles.active().catch(() => ({ name: list[0]?.name ?? '' }));
      activeName = active.name || list[0]?.name || '';
      if (activeName) await profileActions.load(activeName);
    } catch (e) {
      pushToast('error', `Bootstrap failed: ${e instanceof Error ? e.message : e}`);
    }
  }

  async function switchProfile(name: string): Promise<void> {
    if (!name || name === activeName) return;
    activeName = name;
    await profileActions.load(name);
  }

  async function save(): Promise<void> {
    const r = await profileActions.save();
    if (r.ok) pushToast('success', 'Saved');
    else pushToast('error', r.error ?? 'Save failed');
  }

  function applyTheme(t: 'dark' | 'light'): void {
    theme = t;
    document.documentElement.classList.toggle('dark', t === 'dark');
    try { localStorage.setItem('theme', t); } catch {}
  }
  function toggleTheme(): void {
    applyTheme(theme === 'dark' ? 'light' : 'dark');
  }

  function onKey(e: KeyboardEvent): void {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      if ($isDirty && $totalErrors === 0) save();
    }
  }

  onMount(() => {
    let saved: 'dark' | 'light' | null = null;
    try { saved = localStorage.getItem('theme') as 'dark' | 'light' | null; } catch {}
    applyTheme(saved ?? 'light');
    bootstrap();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  $: canSave = $isDirty && $totalErrors === 0 && !$profile.saving;
</script>

<Toasts />

<div class="min-h-screen flex flex-col">
  <header class="border-b border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/60 backdrop-blur sticky top-0 z-30">
    <div class="max-w-7xl mx-auto px-4 h-14 flex items-center gap-4">
      <div class="flex items-center gap-2">
        <span
          class="h-2.5 w-2.5 rounded-full shadow"
          class:bg-emerald-400={$live.socketConnected}
          class:bg-slate-300={!$live.socketConnected}
          class:dark:bg-slate-600={!$live.socketConnected}
          title={$live.socketConnected ? 'Live: connected' : 'Live: disconnected'}
        ></span>
        <span class="font-semibold tracking-tight">XTouch GW Editor</span>
      </div>

      <nav class="flex items-center gap-1">
        {#each links as link}
          <a
            href="{base}{link.href}"
            class="nav-link"
            class:nav-link-active={currentPath.startsWith(link.href)}
          >
            {link.label}
          </a>
        {/each}
      </nav>

      <div class="ml-auto flex items-center gap-3">
        <select
          bind:value={activeName}
          on:change={() => switchProfile(activeName)}
          class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-100 rounded text-sm pl-2 pr-8 py-1"
          title="Loaded profile"
        >
          {#each profiles as p}<option value={p.name}>{p.name}</option>{/each}
        </select>

        <span class="text-xs flex items-center gap-2">
          {#if $totalErrors > 0}
            <span class="px-2 py-0.5 rounded bg-rose-100 text-rose-700 border border-rose-300 dark:bg-rose-950/50 dark:text-rose-300 dark:border-rose-800">{$totalErrors} errors</span>
          {/if}
          {#if $totalWarnings > 0}
            <span class="px-2 py-0.5 rounded bg-amber-100 text-amber-700 border border-amber-300 dark:bg-amber-950/50 dark:text-amber-300 dark:border-amber-800">{$totalWarnings} warnings</span>
          {/if}
          {#if $totalErrors === 0 && $totalWarnings === 0 && $profile.parsed}
            <span class="text-emerald-600 dark:text-emerald-400">✓ valid</span>
          {/if}
        </span>

        <button
          class="px-3 py-1.5 rounded text-sm font-semibold transition-colors"
          class:bg-accent={canSave}
          class:text-slate-900={canSave}
          class:bg-slate-200={!canSave}
          class:text-slate-400={!canSave}
          class:dark:bg-slate-800={!canSave}
          class:dark:text-slate-500={!canSave}
          disabled={!canSave}
          on:click={save}
          title="Ctrl/Cmd+S"
        >
          {$profile.saving ? 'Saving…' : 'Save'}
        </button>

        <button class="text-xs text-slate-500 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-200" on:click={toggleTheme} title="Toggle theme">
          {theme === 'dark' ? '☾' : '☀'}
        </button>
      </div>
    </div>
  </header>

  <main class="flex-1 max-w-7xl mx-auto w-full px-4 py-6">
    <slot />
  </main>

  <footer class="border-t border-slate-200 dark:border-slate-800 text-xs text-slate-500 py-2 px-4 text-center">
    Editing {$profile.name ?? '(no profile)'}. Saves create timestamped snapshots.
  </footer>
</div>
