<script lang="ts">
  import { toasts } from '$lib/stores/toasts';
</script>

<div class="fixed top-4 right-4 z-[100] flex flex-col gap-2 pointer-events-none">
  {#each $toasts as t (t.id)}
    <div
      class="pointer-events-auto px-3 py-2 rounded shadow-lg border text-sm min-w-[240px]"
      class:bg-emerald-100={t.kind === 'success'}
      class:border-emerald-300={t.kind === 'success'}
      class:text-emerald-700={t.kind === 'success'}
      class:dark:bg-emerald-950={t.kind === 'success'}
      class:dark:border-emerald-700={t.kind === 'success'}
      class:dark:text-emerald-200={t.kind === 'success'}
      class:bg-rose-100={t.kind === 'error'}
      class:border-rose-300={t.kind === 'error'}
      class:text-rose-700={t.kind === 'error'}
      class:dark:bg-rose-950={t.kind === 'error'}
      class:dark:border-rose-700={t.kind === 'error'}
      class:dark:text-rose-200={t.kind === 'error'}
      class:bg-white={t.kind === 'info'}
      class:border-slate-200={t.kind === 'info'}
      class:text-slate-800={t.kind === 'info'}
      class:dark:bg-slate-900={t.kind === 'info'}
      class:dark:border-slate-700={t.kind === 'info'}
      class:dark:text-slate-200={t.kind === 'info'}
    >
      {t.message}
    </div>
  {/each}
</div>
