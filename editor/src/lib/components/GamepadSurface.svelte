<script lang="ts">
  import { base } from '$app/paths';
  import { live } from '$lib/stores/live';
  import { selectedControl, selectedPage } from '$lib/stores/selection';
  import { profile } from '$lib/stores/profile';
  import type { GamepadVariant } from '$lib/gamepad-variant';

  export let variant: GamepadVariant = 'xbox';
  export let slot = 1;

  // Raw SVG IDs in the new artwork map to canonical control ids consumed by
  // the Rust input layer (src/input/gamepad/{buttons,axis}.rs).
  // Both xbox and switch2pro share the same set of invizCircle ids.
  const ID_MAP: Record<string, (n: number) => string> = {
    aButton: (n) => `gamepad${n}.btn.a`,
    bButton: (n) => `gamepad${n}.btn.b`,
    xButton: (n) => `gamepad${n}.btn.x`,
    yButton: (n) => `gamepad${n}.btn.y`,
    leftBumper: (n) => `gamepad${n}.btn.lb`,
    rightBumper: (n) => `gamepad${n}.btn.rb`,
    leftTrigger: (n) => `gamepad${n}.btn.lt`,
    rightTrigger: (n) => `gamepad${n}.btn.rt`,
    backButton: (n) => `gamepad${n}.btn.minus`,
    startButton: (n) => `gamepad${n}.btn.plus`,
    dpadUp: (n) => `gamepad${n}.dpad.up`,
    dpadDown: (n) => `gamepad${n}.dpad.down`,
    dpadLeft: (n) => `gamepad${n}.dpad.left`,
    dpadRight: (n) => `gamepad${n}.dpad.right`,
    leftStick: (n) => `gamepad${n}.btn.l3`,
    leftStickClick: (n) => `gamepad${n}.btn.l3`,
    rightStick: (n) => `gamepad${n}.btn.r3`,
    rightStickClick: (n) => `gamepad${n}.btn.r3`
  };

  let host: HTMLDivElement | null = null;
  let svgEl: SVGSVGElement | null = null;
  let loaded = false;
  let loadedSrc = '';
  let loadedSlot = 0;

  // Indicators created in remap() that follow live stick axes.
  let leftStickIndicator: SVGCircleElement | null = null;
  let rightStickIndicator: SVGCircleElement | null = null;
  let leftStickCenter = { x: 0, y: 0 };
  let rightStickCenter = { x: 0, y: 0 };
  let stickRadiusSvg = 0;

  $: src =
    variant === 'switch2pro'
      ? `${base}/gamepad-switch2-pro.svg`
      : `${base}/gamepad-xbox.svg`;

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
      // Keep the SVG style attribute (some assets bake width/margins) but strip
      // the inline width so it scales with the container.
      const style = svgEl.getAttribute('style') ?? '';
      svgEl.setAttribute('style', style.replace(/width:[^;]+;?/i, ''));
      remap(svgEl);
      attach(svgEl);
    }
    loaded = true;
  }

  function remap(root: SVGSVGElement): void {
    leftStickIndicator = null;
    rightStickIndicator = null;
    // Only invizCircles carry a data-control: they are simultaneously the
    // click hit-test region and the highlight surface. The rest of the SVG
    // is purely decorative.
    root.querySelectorAll<SVGElement>('.invizCircle').forEach((el) => {
      const map = ID_MAP[el.id];
      if (!map) return;
      const ctrl = map(slot);
      el.setAttribute('data-control', ctrl);
      el.classList.add('gp-control');
      // Spawn an animated indicator sibling for stick hit-test circles.
      if (
        (el.id === 'leftStick' || el.id === 'leftStickClick') &&
        el.tagName.toLowerCase() === 'circle'
      ) {
        leftStickIndicator = makeStickIndicator(el as SVGCircleElement);
        const c = el as SVGCircleElement;
        leftStickCenter = { x: parseFloat(c.getAttribute('cx') ?? '0'), y: parseFloat(c.getAttribute('cy') ?? '0') };
        stickRadiusSvg = parseFloat(c.getAttribute('r') ?? '0');
      }
      if (
        (el.id === 'rightStick' || el.id === 'rightStickClick') &&
        el.tagName.toLowerCase() === 'circle'
      ) {
        rightStickIndicator = makeStickIndicator(el as SVGCircleElement);
        const c = el as SVGCircleElement;
        rightStickCenter = { x: parseFloat(c.getAttribute('cx') ?? '0'), y: parseFloat(c.getAttribute('cy') ?? '0') };
        stickRadiusSvg = parseFloat(c.getAttribute('r') ?? '0');
      }
    });
  }

  function makeStickIndicator(invizCircle: SVGCircleElement): SVGCircleElement {
    const ns = 'http://www.w3.org/2000/svg';
    const dot = document.createElementNS(ns, 'circle');
    dot.setAttribute('cx', invizCircle.getAttribute('cx') ?? '0');
    dot.setAttribute('cy', invizCircle.getAttribute('cy') ?? '0');
    dot.setAttribute('r', String(parseFloat(invizCircle.getAttribute('r') ?? '0') * 0.22));
    dot.setAttribute('fill', '#38bdf8');
    dot.setAttribute('fill-opacity', '0.65');
    dot.setAttribute('stroke', '#bae6fd');
    dot.setAttribute('stroke-width', '1');
    dot.setAttribute('pointer-events', 'none');
    dot.classList.add('gp-stick-dot');
    invizCircle.parentNode?.insertBefore(dot, invizCircle);
    return dot;
  }

  function attach(root: SVGSVGElement): void {
    root.addEventListener('click', (e) => {
      const t = (e.target as Element | null)?.closest('[data-control]') as SVGElement | null;
      if (!t) return;
      selectedControl.set(t.getAttribute('data-control')!);
    });
  }

  // Animate sticks: translate the visible indicator by axis values (clamped),
  // never the invizCircle itself (we want the click hit-test fixed).
  $: if (svgEl) {
    const cur = $live.axes;
    const lx = cur[`gamepad${slot}.axis.lx`] ?? 0;
    const ly = cur[`gamepad${slot}.axis.ly`] ?? 0;
    const rx = cur[`gamepad${slot}.axis.rx`] ?? 0;
    const ry = cur[`gamepad${slot}.axis.ry`] ?? 0;
    moveStick(leftStickIndicator, leftStickCenter, lx, ly);
    moveStick(rightStickIndicator, rightStickCenter, rx, ry);
  }

  function moveStick(
    dot: SVGCircleElement | null,
    center: { x: number; y: number },
    x: number,
    y: number
  ): void {
    if (!dot) return;
    const travel = stickRadiusSvg * 0.55;
    const cx = center.x + Math.max(-1, Math.min(1, x)) * travel;
    const cy = center.y + Math.max(-1, Math.min(1, y)) * travel;
    dot.setAttribute('cx', cx.toFixed(2));
    dot.setAttribute('cy', cy.toFixed(2));
  }

  // Live highlight: brief glow on touched controls.
  $: if (svgEl && $live.lastTouched) {
    const id = $live.lastTouched.control_id;
    if (id?.startsWith(`gamepad${slot}.`)) {
      const els = svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape(id)}"]`);
      els.forEach((el) => {
        el.classList.add('is-active');
        window.setTimeout(() => el.classList.remove('is-active'), 500);
      });
    }
  }

  // Mappings on the currently selected page (skip when on global -1).
  $: pageMappedKeys = (() => {
    const cfg = $profile.parsed as { pages?: Array<{ controls?: Record<string, unknown> }> } | null;
    if (!cfg || $selectedPage === -1) return new Set<string>();
    return new Set(Object.keys(cfg.pages?.[$selectedPage]?.controls ?? {}));
  })();

  $: globalMappedKeys = (() => {
    const cfg = $profile.parsed as { pages_global?: { controls?: Record<string, unknown> } } | null;
    return new Set(Object.keys(cfg?.pages_global?.controls ?? {}));
  })();

  function applyMappingClass(keys: Set<string>, cls: string): void {
    if (!svgEl) return;
    svgEl.querySelectorAll<SVGElement>(`.${cls}`).forEach((el) => el.classList.remove(cls));
    for (const k of keys) {
      if (!k.startsWith(`gamepad${slot}.`)) continue;
      svgEl
        .querySelectorAll<SVGElement>(`[data-control="${CSS.escape(k)}"]`)
        .forEach((el) => el.classList.add(cls));
    }
  }

  $: if (svgEl) applyMappingClass(pageMappedKeys, 'is-mapped');
  $: if (svgEl) applyMappingClass(globalMappedKeys, 'is-global-mapped');

  // Persistent selection highlight.
  $: if (svgEl) {
    svgEl.querySelectorAll<SVGElement>('.is-selected').forEach((el) => el.classList.remove('is-selected'));
    if ($selectedControl) {
      svgEl
        .querySelectorAll<SVGElement>(`[data-control="${CSS.escape($selectedControl)}"]`)
        .forEach((el) => el.classList.add('is-selected'));
    }
  }

  // Reload only when the SVG source or slot prefix actually changes.
  $: if (host && src && (src !== loadedSrc || slot !== loadedSlot)) {
    loadedSrc = src;
    loadedSlot = slot;
    load();
  }
</script>

<div class="p-3">
  {#if !loaded}
    <div class="text-sm text-slate-500 dark:text-slate-400">Loading gamepad surface…</div>
  {/if}
  <div class="bg-slate-900 dark:bg-slate-950 rounded-xl p-4">
    <div bind:this={host} class="gp-host"></div>
  </div>
</div>

<style>
  /* invizCircles ship with opacity:0 in xbox SVG via embedded <style>; the
     switch2pro SVG omits that block, so enforce the default here. */
  :global(.gp-host .invizCircle) {
    fill: #fff;
    opacity: 0;
  }
  :global(.gp-host .gp-control) {
    cursor: pointer;
    transition: opacity 120ms ease, fill 120ms ease;
  }
  :global(.gp-host .gp-control:hover) {
    fill: #38bdf8;
    opacity: 0.25;
  }
  :global(.gp-host .gp-control.is-mapped) {
    fill: #4ade80;
    opacity: 0.32;
  }
  :global(.gp-host .gp-control.is-global-mapped) {
    fill: #fb923c;
    opacity: 0.32;
  }
  :global(.gp-host .gp-control.is-active) {
    fill: #facc15;
    opacity: 0.7;
    filter: drop-shadow(0 0 4px rgba(250, 204, 21, 0.8));
  }
  :global(.gp-host .gp-control.is-selected) {
    fill: #38bdf8;
    opacity: 0.55;
    stroke: #bae6fd;
    stroke-width: 2;
  }
  :global(.gp-host .gp-stick-dot) {
    pointer-events: none;
  }
</style>
