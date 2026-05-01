<script lang="ts">
  import { base } from '$app/paths';
  import { live } from '$lib/stores/live';
  import { selectedControl } from '$lib/stores/selection';

  export let variant: 'faceoff' | 'xbox' = 'faceoff';
  export let slot = 1;

  let host: HTMLDivElement | null = null;
  let svgEl: SVGSVGElement | null = null;
  let loaded = false;
  let loadedSrc = '';
  let loadedSlot = 0;

  $: src = variant === 'xbox' ? `${base}/gamepad-xbox.svg` : `${base}/gamepad-faceoff.svg`;

  async function load(): Promise<void> {
    if (!host) return;
    const res = await fetch(src);
    const text = await res.text();
    if (!host) return;
    host.innerHTML = text;
    svgEl = host.querySelector('svg');
    if (svgEl) {
      svgEl.setAttribute('class', 'w-full h-auto select-none');
      svgEl.removeAttribute('width');
      svgEl.removeAttribute('height');
      remap(svgEl);
      attach(svgEl);
    }
    loaded = true;
  }

  function remap(root: SVGSVGElement): void {
    // SVGs hardcode "gamepad1.*" — rewrite the slot prefix if needed.
    const target = `gamepad${slot}.`;
    root.querySelectorAll<SVGElement>('[id^="gamepad1."]').forEach((el) => {
      const id = el.id.replace(/^gamepad1\./, target);
      el.setAttribute('data-control', id);
      el.classList.add('gp-control');
    });
  }

  function attach(root: SVGSVGElement): void {
    root.addEventListener('click', (e) => {
      const t = (e.target as Element | null)?.closest('[data-control]') as SVGElement | null;
      if (!t) return;
      selectedControl.set(t.getAttribute('data-control')!);
    });
  }

  // Animate sticks: translate the inner stick element by axis values.
  $: if (svgEl) {
    const cur = $live.axes;
    const lx = cur[`gamepad${slot}.axis.lx`] ?? 0;
    const ly = cur[`gamepad${slot}.axis.ly`] ?? 0;
    const rx = cur[`gamepad${slot}.axis.rx`] ?? 0;
    const ry = cur[`gamepad${slot}.axis.ry`] ?? 0;
    transformStick('left', lx, ly);
    transformStick('right', rx, ry);
  }

  function transformStick(side: 'left' | 'right', x: number, y: number): void {
    if (!svgEl) return;
    const el = svgEl.querySelector<SVGElement>(`[data-control="gamepad${slot}.stick.${side}"]`);
    if (!el) return;
    const px = Math.max(-1, Math.min(1, x)) * 18;
    const py = Math.max(-1, Math.min(1, y)) * 18;
    const inner = el.querySelector<SVGElement>(`[data-control^="gamepad${slot}.axis."]`);
    if (inner) inner.setAttribute('transform', `translate(${px}, ${py})`);
  }

  $: if (svgEl && $live.lastTouched) {
    const id = $live.lastTouched.control_id;
    const el = svgEl.querySelector<SVGElement>(`[data-control="${CSS.escape(id)}"]`);
    if (el) {
      el.classList.add('is-active');
      window.setTimeout(() => el.classList.remove('is-active'), 500);
    }
  }

  $: if (svgEl) {
    svgEl.querySelectorAll<SVGElement>('.is-selected').forEach((el) => el.classList.remove('is-selected'));
    if ($selectedControl) {
      svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape($selectedControl)}"]`).forEach((el) => el.classList.add('is-selected'));
    }
  }

  // Only (re)load when the SVG source or slot prefix actually changes —
  // not on every reactive tick (e.g. live axis updates) which would
  // re-fetch the SVG in a tight loop.
  $: if (host && src && (src !== loadedSrc || slot !== loadedSlot)) {
    loadedSrc = src;
    loadedSlot = slot;
    load();
  }
</script>

<div class="p-3">
  <div class="mb-2 flex items-center gap-2 text-xs text-slate-700 dark:text-slate-400">
    <span>Slot {slot}</span>
    <select bind:value={variant} class="bg-white border border-slate-300 text-slate-900 dark:bg-slate-800 dark:border-slate-700 dark:text-slate-200 rounded text-xs">
      <option value="faceoff">Faceoff Pro</option>
      <option value="xbox">Xbox</option>
    </select>
    {#if !loaded}<span class="text-slate-500 dark:text-slate-400">loading…</span>{/if}
  </div>
  <!-- SVG art has baked-in dark fills; keep a dark backing panel in both modes so the surface stays legible. -->
  <div class="bg-slate-900 dark:bg-slate-950 rounded-xl p-4">
    <div bind:this={host} class="gp-host"></div>
  </div>
</div>

<style>
  :global(.gp-host .gp-control) {
    cursor: pointer;
  }
  :global(.gp-host .gp-control:hover) {
    filter: brightness(1.3);
  }
  :global(.gp-host .gp-control.is-active) {
    filter: brightness(1.9) drop-shadow(0 0 4px rgba(250, 204, 21, 0.8));
  }
  :global(.gp-host .gp-control.is-selected) {
    stroke: #38bdf8 !important;
    stroke-width: 3 !important;
  }
</style>
