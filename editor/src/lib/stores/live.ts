import { writable, get } from 'svelte/store';
import { liveSocket, type LiveEvent } from '$lib/api';
import { selectedPage } from '$lib/stores/selection';

export interface LiveState {
  lastTouched: { control_id: string; ts: number; value?: number } | null;
  axes: Record<string, number>;
  values: Record<string, number>; // control_id → most recent value
  connections: Record<string, { status: 'up' | 'down'; detail?: string }>;
  socketConnected: boolean;
  /** Last server-confirmed active page index. Used by PageTabs to avoid
   *  echoing a `page_changed` event back as a redundant POST. */
  serverPageIndex: number | null;
}

export function getServerPageIndex(): number | null {
  return get(live).serverPageIndex;
}

const initial: LiveState = {
  lastTouched: null,
  axes: {},
  values: {},
  connections: {},
  socketConnected: false,
  serverPageIndex: null
};

export const live = writable<LiveState>(initial);

let started = false;
let captureHandlers: Array<(ev: LiveEvent) => boolean> = [];

export function startLive(): void {
  if (started) return;
  started = true;
  liveSocket.open();
  liveSocket.onConnected((c) => live.update((s) => ({ ...s, socketConnected: c })));
  liveSocket.subscribe((ev) => {
    handleEvent(ev);
    // capture handlers run after; first one returning true is removed
    captureHandlers = captureHandlers.filter((h) => !h(ev));
  });
}

function handleEvent(ev: LiveEvent): void {
  // Backend tags variants with `event`; tolerate older `kind`-tagged messages too.
  const variant = (ev.event ?? ev.kind) as string | undefined;
  if (import.meta.env.DEV) console.debug('[live]', variant, ev);
  if (variant === 'hw_event' && ev.control_id) {
    const id = ev.control_id;
    const val = typeof ev.value === 'number' ? ev.value : 0;
    live.update((s) => {
      const next: LiveState = {
        ...s,
        lastTouched: { control_id: id, ts: Date.now(), value: val },
        values: { ...s.values, [id]: val }
      };
      if (id.includes('.axis.')) next.axes = { ...s.axes, [id]: val };
      return next;
    });
    // Decay last-touched after 600ms
    const myId = id;
    setTimeout(() => {
      live.update((s) => (s.lastTouched && s.lastTouched.control_id === myId ? { ...s, lastTouched: null } : s));
    }, 600);
  } else if (variant === 'page_changed' && typeof ev.index === 'number') {
    const idx = ev.index;
    live.update((s) => ({ ...s, serverPageIndex: idx }));
    // Mirror the X-Touch's active page in the editor's page tabs.
    // We always sync to a real page index — leaving the user on the "All
    // pages" pseudo-tab (-1) would defeat the requested two-way sync.
    if (get(selectedPage) !== idx) selectedPage.set(idx);
  } else if (variant === 'connection' && ev.target) {
    live.update((s) => ({
      ...s,
      connections: {
        ...s.connections,
        [ev.target!]: { status: (ev.status as 'up' | 'down') ?? 'down', detail: ev.detail }
      }
    }));
  }
}

/**
 * Register a one-shot capture handler. Returns an unsubscribe to cancel.
 * The handler should return true to consume the event and exit capture mode.
 */
export function onceCapture(handler: (ev: LiveEvent) => boolean): () => void {
  captureHandlers.push(handler);
  return () => {
    captureHandlers = captureHandlers.filter((h) => h !== handler);
  };
}
