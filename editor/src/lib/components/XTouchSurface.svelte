<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { base } from '$app/paths';
  import { live } from '$lib/stores/live';
  import { selectedControl, selectedPage } from '$lib/stores/selection';
  import { profile } from '$lib/stores/profile';

  // Generic per-strip IDs in the SVG (one occurrence per channel strip).
  // We rewrite each instance to `<base><index>` in document order.
  // The 9th occurrence (master strip) gets the suffix "_master".
  const STRIP_IDS: Record<string, string> = {
    'fader-track': 'fader_track',
    'fader-thumb': 'fader_thumb',
    'fader-graduation': 'fader_graduation',
    vpot: 'vpot',
    lcd: 'lcd',
    'button-select': 'select',
    'button-mute': 'mute',
    'button-solo': 'solo',
    'button-rec': 'rec'
  };

  // MCU LCD palette: 0=off, 1=red, 2=green, 3=yellow, 4=blue, 5=magenta, 6=cyan, 7=white.
  const LCD_BG: string[] = ['#161616', '#7f1d1d', '#14532d', '#713f12', '#1e3a8a', '#581c87', '#155e75', '#1f2937'];
  const LCD_FG: string[] = ['#9ca3af', '#fecaca', '#bbf7d0', '#fef08a', '#bfdbfe', '#f5d0fe', '#a5f3fc', '#f3f4f6'];

  // Single-instance buttons map raw SVG ids to canonical YAML control ids.
  // The default rule is `button-foo-bar` → `foo_bar`. A few SVG ids have no
  // dash but split into two canonical words (or are not `button-` prefixed):
  // override those explicitly.
  const SINGLETON_OVERRIDES: Record<string, string> = {
    'button-globalview': 'global_view',
    'button-audiotracks': 'audio_tracks',
    'button-audioinst': 'audio_inst',
    'button-miditracks': 'midi_tracks',
    jog_wheel: 'jog_wheel'
  };

  function singletonIdFor(raw: string): string | null {
    const override = SINGLETON_OVERRIDES[raw];
    if (override) return override;
    if (raw.startsWith('button-')) return raw.slice(7).replace(/-/g, '_');
    return null;
  }

  let host: HTMLDivElement | null = null;
  let svgEl: SVGSVGElement | null = null;
  let loaded = false;
  let error: string | null = null;

  async function loadSvg(): Promise<void> {
    try {
      const res = await fetch(`${base}/xtouch.svg`);
      const text = await res.text();
      if (!host) return;
      host.innerHTML = text;
      svgEl = host.querySelector('svg');
      if (svgEl) {
        svgEl.setAttribute('class', 'w-full h-auto select-none');
        svgEl.removeAttribute('width');
        svgEl.removeAttribute('height');
        rewriteIds(svgEl);
        attachHandlers(svgEl);
      }
      loaded = true;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function rewriteIds(root: SVGSVGElement): void {
    const counters: Record<string, number> = {};
    // Walk in document order; rewrite generic strip ids to indexed canonical ids.
    const all = root.querySelectorAll<SVGElement>('[id]');
    all.forEach((el) => {
      const raw = el.id;
      if (STRIP_IDS[raw]) {
        counters[raw] = (counters[raw] ?? 0) + 1;
        const idx = counters[raw];
        const baseName = STRIP_IDS[raw];
        // 9 occurrences: indices 1..8 = strips, 9 = master.
        const suffix = idx <= 8 ? String(idx) : '_master';
        let ctrlId: string;
        if (baseName === 'select' && idx === 9) {
          ctrlId = 'flip';
        } else if (baseName === 'fader_track' || baseName === 'fader_thumb' || baseName === 'fader_graduation') {
          ctrlId = `fader${suffix}__${baseName}`;
        } else {
          ctrlId = `${baseName}${suffix}`;
        }
        el.setAttribute('data-control', ctrlId);
        el.classList.add('xt-control');
      } else {
        const single = singletonIdFor(raw);
        if (single) {
          el.setAttribute('data-control', single);
          el.classList.add('xt-control');
        }
      }
    });
    // Faders: collapse `fader{n}__<part>` data-controls so the whole
    // track+thumb+graduation strip clicks through to `fader{n}`.
    root.querySelectorAll<SVGElement>('[data-control^="fader"][data-control*="__"]').forEach((el) => {
      const dc = el.getAttribute('data-control')!;
      const m = dc.match(/^(fader[\d_master]+)__/);
      if (m) el.setAttribute('data-control', m[1]);
    });
  }

  function attachHandlers(root: SVGSVGElement): void {
    root.addEventListener('click', (e) => {
      const t = (e.target as Element | null)?.closest('[data-control]') as SVGElement | null;
      if (!t) return;
      const id = controlIdFromSvg(t.getAttribute('data-control')!);
      if (reservedControls.has(id)) return;
      selectedControl.set(id);
    });
  }

  // MCU note number → canonical SVG control id, restricted to the buttons
  // that are commonly chosen for paging navigation. Used to dim and disable
  // those controls in the editor so the user can't override the paging
  // shortcut by mapping them.
  const PAGING_NOTE_TO_CONTROL: Record<number, string> = {
    46: 'fader_prev',
    47: 'fader_next',
    48: 'channel_prev',
    49: 'channel_next',
    54: 'f1',
    55: 'f2',
    56: 'f3',
    57: 'f4',
    58: 'f5',
    59: 'f6',
    60: 'f7',
    61: 'f8'
  };

  $: reservedControls = (() => {
    const cfg = $profile.parsed as {
      paging?: { prev_note?: number; next_note?: number };
      pages?: unknown[];
    } | null;
    const set = new Set<string>();
    const prev = cfg?.paging?.prev_note ?? 46;
    const next = cfg?.paging?.next_note ?? 47;
    const a = PAGING_NOTE_TO_CONTROL[prev];
    const b = PAGING_NOTE_TO_CONTROL[next];
    if (a) set.add(a);
    if (b) set.add(b);
    // F1..F8 are wired by the runtime as direct-page jumps, capped at the
    // number of configured pages (router/xtouch_input.rs ~line 100).
    const pageCount = Math.min(8, Array.isArray(cfg?.pages) ? cfg!.pages!.length : 0);
    for (let i = 1; i <= pageCount; i++) set.add(`f${i}`);
    return set;
  })();

  $: if (svgEl) applyReservedClass(reservedControls);

  function applyReservedClass(keys: Set<string>): void {
    if (!svgEl) return;
    svgEl.querySelectorAll<SVGElement>('.is-reserved').forEach((el) => el.classList.remove('is-reserved'));
    for (const k of keys) {
      svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape(k)}"]`).forEach((el) => el.classList.add('is-reserved'));
    }
  }

  // Map an SVG data-control id to the YAML mapping key. V-pots store mappings
  // under "vpotN_rotate" (rotation is the default action); the SVG node is
  // just "vpotN". Other controls match the YAML key 1:1.
  function controlIdFromSvg(svgId: string): string {
    if (/^vpot\d+$/.test(svgId)) return `${svgId}_rotate`;
    return svgId;
  }

  // Live highlight: when last-touched control matches a node, briefly add .is-active class.
  $: if (svgEl && $live.lastTouched) highlight($live.lastTouched.control_id);

  function highlight(controlId: string): void {
    if (!svgEl) return;
    const els = svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape(controlId)}"]`);
    els.forEach((el) => {
      el.classList.add('is-active');
      window.setTimeout(() => el.classList.remove('is-active'), 600);
    });
  }

  // Mappings present on the currently selected page (only when not on -1 / global).
  $: pageMappedKeys = (() => {
    const cfg = $profile.parsed as { pages?: Array<{ controls?: Record<string, unknown> }> } | null;
    if (!cfg || $selectedPage === -1) return new Set<string>();
    return new Set(Object.keys(cfg.pages?.[$selectedPage]?.controls ?? {}));
  })();

  // Mappings present in pages_global — always highlighted, in a different color.
  $: globalMappedKeys = (() => {
    const cfg = $profile.parsed as { pages_global?: { controls?: Record<string, unknown> } } | null;
    return new Set(Object.keys(cfg?.pages_global?.controls ?? {}));
  })();

  // YAML keys (e.g. "vpot1_rotate", "vpot1_press") may not match SVG data-control
  // (e.g. "vpot1"). Strip known action suffixes to find the SVG element.
  function svgIdFor(key: string): string {
    return key.replace(/_(rotate|press|click|touch)$/i, '');
  }

  function applyMappingClass(keys: Set<string>, cls: string): void {
    if (!svgEl) return;
    svgEl.querySelectorAll<SVGElement>(`.${cls}`).forEach((el) => el.classList.remove(cls));
    for (const k of keys) {
      const els = svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape(svgIdFor(k))}"]`);
      els.forEach((el) => el.classList.add(cls));
    }
  }

  $: if (svgEl) applyMappingClass(pageMappedKeys, 'is-mapped');
  $: if (svgEl) applyMappingClass(globalMappedKeys, 'is-global-mapped');

  // Render LCD strip labels and colors from the active page's lcd config.
  $: lcdStrips = (() => {
    if ($selectedPage < 0) return null;
    const page = $profile.parsed?.pages?.[$selectedPage] as
      | { lcd?: { labels?: string[]; colors?: number[] } }
      | undefined;
    return page?.lcd ?? null;
  })();

  $: if (svgEl) renderLcds(lcdStrips);

  function renderLcds(lcd: { labels?: string[]; colors?: number[] } | null): void {
    if (!svgEl) return;
    for (let i = 1; i <= 8; i++) {
      const group = svgEl.querySelector<SVGGElement>(`g[data-control="lcd${i}"]`);
      if (!group) continue;
      const colorIdx = lcd?.colors?.[i - 1] ?? 0;
      const bg = LCD_BG[colorIdx] ?? LCD_BG[0];
      const fg = LCD_FG[colorIdx] ?? LCD_FG[0];

      const bgEl = group.querySelector<SVGPathElement>('[id="bg"]');
      if (bgEl) bgEl.setAttribute('fill', bg);

      // Remove the original separation-line group (its stroked path renders too thick
      // when we recolor it) and replace it with a fresh thin <line> at the bg's mid-Y.
      const oldSep = group.querySelector<SVGGElement>('[id="separation-line"]');
      if (oldSep) oldSep.remove();
      const existingSep = group.querySelector<SVGLineElement>('[data-sep="1"]');
      if (existingSep) existingSep.remove();
      if (bgEl) {
        const b = bgEl.getBBox();
        const midY = b.y + b.height / 2;
        const padX = b.width * 0.04;
        const line = document.createElementNS('http://www.w3.org/2000/svg', 'line');
        line.setAttribute('x1', String(b.x + padX));
        line.setAttribute('x2', String(b.x + b.width - padX));
        line.setAttribute('y1', String(midY));
        line.setAttribute('y2', String(midY));
        line.setAttribute('stroke', fg);
        line.setAttribute('stroke-width', '0.08');
        line.setAttribute('data-sep', '1');
        group.appendChild(line);
      }

      const label = (lcd?.labels?.[i - 1] ?? '') as string;
      const [line1 = '', line2 = ''] = label.split('\n');
      const texts = group.querySelectorAll<SVGTextElement>('text');
      const applyText = (el: SVGTextElement, txt: string) => {
        el.textContent = txt;
        el.setAttribute('fill', fg);
        el.setAttribute('font-size', '4.2');
        el.setAttribute('letter-spacing', '0');
        el.setAttribute('font-family', "'VT323', monospace");
        el.setAttribute('font-style', 'normal');
      };
      if (texts[0]) applyText(texts[0], line1);
      if (texts[1]) applyText(texts[1], line2);
    }
  }

  // ---- Live fader sync ---------------------------------------------------
  // Cache each fader's track + thumb geometry once after the SVG is rewritten.
  type FaderGeom = {
    trackY: number;
    trackH: number;
    thumbY0: number;
    thumbH: number;
    thumbEl: SVGGraphicsElement;
    extraTopSvg: number;    // overshoot at max value (top), in SVG user units
    extraBottomSvg: number; // overshoot at min value (bottom), in SVG user units
  };
  const faderGeom = new Map<string, FaderGeom>();

  // Visual overshoot at each end of the fader travel, in display pixels.
  const FADER_OVERSHOOT_TOP_PX = 13;     // extra travel at value=1
  const FADER_OVERSHOOT_BOTTOM_PX = 13;  // extra travel at value=0

  function captureFaderGeometry(): void {
    if (!svgEl) return;
    const labels = ['fader1','fader2','fader3','fader4','fader5','fader6','fader7','fader8','fader_master'];
    for (const id of labels) {
      const track = svgEl.querySelector<SVGGraphicsElement>(`[data-control="${id}"][id="fader-track"]`);
      const thumb = svgEl.querySelector<SVGGraphicsElement>(`[data-control="${id}"][id="fader-thumb"]`);
      if (!track || !thumb) continue;
      const tb = track.getBBox();
      const hb = thumb.getBBox();
      // Convert overshoot constants (device px) into SVG user units using the
      // track's rendered height vs. its bbox height.
      const rect = track.getBoundingClientRect();
      const pxPerUnit = rect.height > 0 && tb.height > 0 ? rect.height / tb.height : 1;
      faderGeom.set(id, {
        trackY: tb.y,
        trackH: tb.height,
        thumbY0: hb.y,
        thumbH: hb.height,
        thumbEl: thumb,
        extraTopSvg: FADER_OVERSHOOT_TOP_PX / pxPerUnit,
        extraBottomSvg: FADER_OVERSHOOT_BOTTOM_PX / pxPerUnit
      });
    }
  }

  function syncFaderPositions(values: Record<string, number>): void {
    if (!svgEl) return;
    if (faderGeom.size === 0) captureFaderGeometry();
    for (const [id, g] of faderGeom) {
      const raw = values[id];
      if (typeof raw !== 'number') continue;
      // Backend already normalizes faders to 0.0..1.0 (see classify_xtouch_midi).
      const norm = Math.max(0, Math.min(1, raw));
      // Value 0 → thumb at bottom (overshooting by extra); value 1 → top (overshooting by extra).
      const topY = g.trackY - g.extraTopSvg;
      const bottomY = g.trackY + g.trackH - g.thumbH + g.extraBottomSvg;
      const targetY = bottomY + (topY - bottomY) * norm;
      const dy = targetY - g.thumbY0;
      g.thumbEl.setAttribute('transform', `translate(0 ${dy.toFixed(3)})`);
    }
  }

  $: if (svgEl && loaded) syncFaderPositions($live.values);

  // Highlight selected control persistently
  $: if (svgEl && $selectedControl !== undefined) {
    svgEl.querySelectorAll<SVGElement>('.is-selected').forEach((el) => el.classList.remove('is-selected'));
    if ($selectedControl) {
      const els = svgEl.querySelectorAll<SVGElement>(`[data-control="${CSS.escape($selectedControl)}"]`);
      els.forEach((el) => el.classList.add('is-selected'));
    }
  }

  onMount(loadSvg);
  onDestroy(() => {});
</script>

<div class="p-3">
  {#if error}
    <div class="text-sm text-rose-600 dark:text-rose-400">Failed to load surface SVG: {error}</div>
  {:else if !loaded}
    <div class="text-sm text-slate-500 dark:text-slate-400">Loading X-Touch surface…</div>
  {/if}
  <div class="p-4">
    <div bind:this={host} class="xt-host xt-host--shadow"></div>
  </div>
</div>

<style>
  .xt-host--shadow :global(svg) {
    filter: drop-shadow(0 10px 22px rgba(0, 0, 0, 0.15)) drop-shadow(0 2px 4px rgba(0, 0, 0, 0.20));
  }
  :global(.xt-host .xt-control) {
    cursor: pointer;
    transition: filter 120ms ease, opacity 120ms ease;
  }
  :global(.xt-host .xt-control:hover) {
    filter: brightness(1.4) drop-shadow(0 0 2px rgba(56, 189, 248, 0.6));
  }
  :global(.xt-host .xt-control.is-active) {
    filter: brightness(1.8) drop-shadow(0 0 6px rgba(250, 204, 21, 0.9));
  }
  :global(.xt-host .xt-control.is-global-mapped [id^="color"]),
  :global(.xt-host .xt-control.is-global-mapped:not(:has([id^="color"])) [id="button"]) {
    fill: #fb923c;
  }
  :global(.xt-host .xt-control.is-mapped [id^="color"]),
  :global(.xt-host .xt-control.is-mapped:not(:has([id^="color"])) [id="button"]) {
    fill: #4ade80;
  }
  :global(.xt-host .xt-control.is-global-mapped[id="fader-track"]),
  :global(.xt-host .xt-control.is-global-mapped[id="fader-graduation"]) {
    fill: #fb923c;
  }
  :global(.xt-host .xt-control.is-mapped[id="fader-track"]),
  :global(.xt-host .xt-control.is-mapped[id="fader-graduation"]) {
    fill: #4ade80;
  }
  :global(.xt-host .xt-control.is-global-mapped) {
    filter: brightness(1.5) drop-shadow(0 0 1px rgba(251, 146, 60, 1)) drop-shadow(0 0 2px rgba(251, 146, 60, 0.7));
  }
  :global(.xt-host .xt-control.is-mapped) {
    filter: brightness(1.55) drop-shadow(0 0 1px rgba(74, 222, 128, 1)) drop-shadow(0 0 2px rgba(74, 222, 128, 0.7));
  }
  :global(.xt-host .xt-control.is-mapped[id="fader-graduation"]),
  :global(.xt-host .xt-control.is-global-mapped[id="fader-graduation"]) {
    filter: none;
  }
  :global(.xt-host .xt-control.is-selected) {
    outline: 2px solid #38bdf8;
    outline-offset: 2px;
  }
  :global(.xt-host .xt-control.is-selected[id="fader-graduation"]) {
    outline: none;
  }
  :global(.xt-host .xt-control.is-reserved) {
    cursor: not-allowed;
    opacity: 0.45;
  }
  :global(.xt-host .xt-control.is-reserved:hover) {
    filter: none;
  }
</style>
