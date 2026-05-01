<script lang="ts">
  import XTouchSurface from '$lib/components/XTouchSurface.svelte';
  import GamepadSurface from '$lib/components/GamepadSurface.svelte';
  import MappingEditor from '$lib/components/MappingEditor.svelte';
  import PageTabs from '$lib/components/PageTabs.svelte';
</script>

<div class="grid grid-cols-1 xl:grid-cols-3 gap-4">
  <div class="xl:col-span-2 space-y-4">
    <section class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
      <header class="px-4 py-2 border-b border-slate-200 text-sm text-slate-700 dark:border-slate-800 dark:text-slate-300">X-Touch</header>
      <PageTabs />
      <XTouchSurface />
    </section>
    <section class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
      <header class="px-4 py-2 border-b border-slate-200 text-sm text-slate-700 dark:border-slate-800 dark:text-slate-300">Gamepad</header>
      <GamepadSurface />
    </section>
  </div>
  <aside class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
    <header class="px-4 py-2 border-b border-slate-200 text-sm text-slate-700 dark:border-slate-800 dark:text-slate-300">Mapping</header>
    <MappingEditor />
  </aside>
</div>
