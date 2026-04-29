// Shared layout for the §1 ambient hero scene.
// The same seeded RNG powers the live R3F canvas AND the still-image
// generator (tools/landing/generate-hero-still.mjs). Treats the still
// as a frozen frame of the live art, not a degraded version (per spec:
// design-equal fallback).
//
// Plain JS + JSDoc so the Node-side generator script can import it
// without a TS loader. The React-side .tsx imports it the same way.

/**
 * @typedef {readonly [number, number, number]} Vec3
 *
 * @typedef {object} HeroNode
 * @property {number} id
 * @property {Vec3} pos
 * @property {number} size  - 0..1 size weight
 * @property {number} phase - radians; off-rhythm breathing
 *
 * @typedef {object} HeroEdge
 * @property {number} a
 * @property {number} b
 * @property {number} d - Euclidean distance between endpoints
 *
 * @typedef {object} HeroGraph
 * @property {HeroNode[]} nodes
 * @property {HeroEdge[]} edges
 *
 * @typedef {object} HeroLayoutOptions
 * @property {number} [count]
 * @property {number} [spread]
 * @property {number} [neighbours]
 * @property {number} [edgeCutoff]
 * @property {number} [seed]
 */

// Mulberry32 — small, deterministic, non-crypto. Seed is the only input;
// same seed → same constellation in JS and in the still-image script.
function mulberry32(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** @type {Required<HeroLayoutOptions>} */
const DEFAULTS = {
  count: 48,
  spread: 6,
  neighbours: 3,
  edgeCutoff: 3.4,
  seed: 0x7e_a3_c7_10,
};

/**
 * @param {HeroLayoutOptions} [opts]
 * @returns {HeroGraph}
 */
export function makeHeroGraph(opts = {}) {
  const { count, spread, neighbours, edgeCutoff, seed } = { ...DEFAULTS, ...opts };
  const rand = mulberry32(seed);

  /** @type {HeroNode[]} */
  const nodes = [];
  for (let i = 0; i < count; i++) {
    const bx = (rand() + rand()) / 2;
    const by = (rand() + rand()) / 2;
    const bz = (rand() + rand()) / 2;
    nodes.push({
      id: i,
      pos: [
        (bx - 0.5) * 2 * spread,
        (by - 0.5) * 2 * spread * 0.55,
        (bz - 0.5) * 2 * spread * 0.7,
      ],
      size: 0.55 + rand() * 0.85,
      phase: rand() * Math.PI * 2,
    });
  }

  /** @type {Map<string, HeroEdge>} */
  const edgeSet = new Map();
  for (const a of nodes) {
    /** @type {{ id: number; d: number }[]} */
    const dists = [];
    for (const b of nodes) {
      if (b.id === a.id) continue;
      const dx = a.pos[0] - b.pos[0];
      const dy = a.pos[1] - b.pos[1];
      const dz = a.pos[2] - b.pos[2];
      const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
      dists.push({ id: b.id, d });
    }
    dists.sort((p, q) => p.d - q.d);
    for (let k = 0; k < Math.min(neighbours, dists.length); k++) {
      const { id, d } = dists[k];
      if (d > edgeCutoff) continue;
      const key = a.id < id ? `${a.id}-${id}` : `${id}-${a.id}`;
      if (!edgeSet.has(key)) {
        edgeSet.set(key, { a: Math.min(a.id, id), b: Math.max(a.id, id), d });
      }
    }
  }

  return { nodes, edges: [...edgeSet.values()] };
}

/**
 * Project a 3D world point to the still-image's 2D viewBox using the same
 * trivial orthographic-ish projection the live canvas's default camera
 * settles into.
 *
 * @param {Vec3} p
 * @param {number} viewW
 * @param {number} viewH
 * @param {number} spread
 */
export function projectForStill(p, viewW, viewH, spread) {
  const nx = p[0] / spread;
  const ny = p[1] / spread;
  const nz = p[2] / spread;
  const x = viewW / 2 + nx * (viewW * 0.42);
  const y = viewH / 2 - ny * (viewH * 0.42);
  const depth = (nz + 1) / 2;
  return { x, y, depth };
}

export const HERO_DEFAULTS = DEFAULTS;
