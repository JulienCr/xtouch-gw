import { writable } from 'svelte/store';

export const selectedControl = writable<string | null>(null);

/**
 * Index of the currently selected page in `cfg.pages`.
 *
 * Special value: `-1` is the sentinel for the pinned "All pages" pseudo-page,
 * which edits `cfg.pages_global.controls` instead of `cfg.pages[i].controls`.
 *
 * Default is `0` (first real page).
 */
export const selectedPage = writable<number>(0);
