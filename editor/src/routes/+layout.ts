// SPA mode: prerender disabled, SSR off so the build emits a single index.html
// that the Rust gateway can serve at /editor.
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'never';
