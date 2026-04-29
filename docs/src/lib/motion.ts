// Reduced-motion plumbing for the v2 landing.
// Every animation on `/` should consult these helpers so a single user
// preference flips the page from animated to still-art-equal.

export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

// Thin wrapper over gsap.matchMedia that forwards reduce/no-reduce contexts.
// Usage:
//   import gsap from 'gsap';
//   import { withMotion } from '../lib/motion';
//   withMotion(gsap, ({ reduce, full }) => {
//     full(() => { /* full animation */ });
//     reduce(() => { /* still / minimal */ });
//   });
export function withMotion(
  gsap: typeof import('gsap').default,
  build: (ctx: {
    reduce: (fn: () => void) => void;
    full: (fn: () => void) => void;
  }) => void,
) {
  const mm = gsap.matchMedia();
  const reduce = (fn: () => void) =>
    mm.add('(prefers-reduced-motion: reduce)', fn);
  const full = (fn: () => void) =>
    mm.add('(prefers-reduced-motion: no-preference)', fn);
  build({ reduce, full });
  return mm;
}

// Lazy WebGL availability check. Mirrors the spec's "no-WebGL → static fallback"
// gate. Returns false on SSR.
export function hasWebGL(): boolean {
  if (typeof window === 'undefined') return false;
  try {
    const canvas = document.createElement('canvas');
    return !!(
      canvas.getContext('webgl2') ||
      canvas.getContext('webgl') ||
      canvas.getContext('experimental-webgl')
    );
  } catch {
    return false;
  }
}
