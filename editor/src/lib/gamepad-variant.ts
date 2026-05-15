export type GamepadVariant = 'switch2pro' | 'xbox';

export function detectVariant(productMatch?: string | null): GamepadVariant {
  const s = (productMatch ?? '').toLowerCase();
  if (s.includes('pro controller') || s.includes('switch') || s.includes('nintendo')) {
    return 'switch2pro';
  }
  return 'xbox';
}
