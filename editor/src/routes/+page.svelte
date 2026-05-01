<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { base } from '$app/paths';

  onMount(() => {
    goto(`${base}/surface`, { replaceState: true });
  });
</script>

<div class="p-8 text-slate-400">Redirecting to /surface…</div>
