<script lang="ts">
  import { profile } from '$lib/stores/profile';
  import ConnectionsSection from './settings/ConnectionsSection.svelte';
  import HardwareSection from './settings/HardwareSection.svelte';
  import CamerasSection from './settings/CamerasSection.svelte';
  import PagingSection from './settings/PagingSection.svelte';

  $: cfg = $profile.parsed;
</script>

<div class="p-3 space-y-3">
  {#if !cfg}
    <div class="text-sm text-slate-500">No profile loaded.</div>
  {:else}
    <ConnectionsSection />
    <HardwareSection />
    <CamerasSection />
    <PagingSection />
  {/if}
</div>
