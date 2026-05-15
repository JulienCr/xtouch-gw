<script lang="ts">
  import GamepadSurface from '$lib/components/GamepadSurface.svelte';
  import MappingEditor from '$lib/components/MappingEditor.svelte';
  import PageTabs from '$lib/components/PageTabs.svelte';
  import { profile } from '$lib/stores/profile';
  import { detectVariant, type GamepadVariant } from '$lib/gamepad-variant';

  let currentSlot = 1;
  let manualOverride: GamepadVariant | '' = '';

  $: gamepads = (($profile.parsed as { gamepad?: { gamepads?: Array<{ product_match?: string }> } } | null)
    ?.gamepad?.gamepads ?? []) as Array<{ product_match?: string }>;
  $: slotCount = Math.max(1, gamepads.length);
  $: slotOptions = Array.from({ length: slotCount }, (_, i) => i + 1);
  // Clamp currentSlot when the profile reload shrinks the gamepad list.
  $: if (currentSlot > slotCount) currentSlot = slotCount;
  $: cfg = gamepads[currentSlot - 1];
  $: autoVariant = detectVariant(cfg?.product_match);
  $: variant = (manualOverride || autoVariant) as GamepadVariant;

  function slotLabel(n: number): string {
    const match = gamepads[n - 1]?.product_match;
    return match ? `${n} — ${match}` : String(n);
  }
</script>

<div class="grid grid-cols-1 xl:grid-cols-3 gap-4">
  <div class="xl:col-span-2 space-y-4">
    <section class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
      <header class="px-4 py-2 border-b border-slate-200 text-sm text-slate-700 dark:border-slate-800 dark:text-slate-300 flex items-center gap-3">
        <span>Gamepad</span>
        <span class="ml-auto flex items-center gap-3 text-xs">
          <label class="flex items-center gap-1.5">
            <span class="text-slate-500 dark:text-slate-400">Slot</span>
            <select
              bind:value={currentSlot}
              class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-200 rounded text-xs px-1.5 py-0.5"
            >
              {#each slotOptions as n}
                <option value={n}>{slotLabel(n)}</option>
              {/each}
            </select>
          </label>
          <label class="flex items-center gap-1.5">
            <span class="text-slate-500 dark:text-slate-400">Visual</span>
            <select
              bind:value={manualOverride}
              class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-200 rounded text-xs px-1.5 py-0.5"
              title="Override the auto-detected gamepad artwork (UI only, not persisted)"
            >
              <option value="">Auto ({autoVariant})</option>
              <option value="switch2pro">Switch 2 Pro</option>
              <option value="xbox">Xbox</option>
            </select>
          </label>
        </span>
      </header>
      <PageTabs />
      <GamepadSurface {variant} slot={currentSlot} />
    </section>
  </div>
  <aside class="rounded-xl border border-slate-200 bg-white/70 dark:border-slate-800 dark:bg-slate-900/30">
    <header class="px-4 py-2 border-b border-slate-200 text-sm text-slate-700 dark:border-slate-800 dark:text-slate-300">Mapping</header>
    <MappingEditor />
  </aside>
</div>
