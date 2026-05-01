import { writable } from 'svelte/store';

export interface Toast {
  id: number;
  kind: 'success' | 'error' | 'info';
  message: string;
}

export const toasts = writable<Toast[]>([]);

let counter = 0;

export function pushToast(kind: Toast['kind'], message: string, ttl = 4000): void {
  const id = ++counter;
  toasts.update((t) => [...t, { id, kind, message }]);
  setTimeout(() => {
    toasts.update((t) => t.filter((x) => x.id !== id));
  }, ttl);
}
