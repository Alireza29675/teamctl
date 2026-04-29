// Generate the §1 hero still-image (PNG + WebP + an SVG source-of-truth).
// Uses the same seeded RNG and node layout as the live R3F canvas so the
// still is a frozen frame of the live art — not a degraded version. Spec
// non-negotiable: the fallback must be design-equal.
//
// Run from docs/:
//   node tools/landing/generate-hero-still.mjs
//
// Outputs (committed alongside the source):
//   src/assets/landing/hero-still.svg   (source of truth)
//   src/assets/landing/hero-still.png   (rasterised, served via <picture>)
//   src/assets/landing/hero-still.webp  (rasterised, preferred src)

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

import { makeHeroGraph, projectForStill } from '../../src/components/landing/hero-graph.js';

// ---- Render parameters --------------------------------------------------
const W = 2560;
const H = 1440;
const SPREAD = 6;        // matches DEFAULTS.spread in hero-graph.js
const EDGE_CUTOFF = 3.4; // matches DEFAULTS.edgeCutoff
const BG = '#0d0e10';

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = resolve(__dirname, '../../src/assets/landing');
mkdirSync(OUT_DIR, { recursive: true });

const graph = makeHeroGraph({});
const projected = graph.nodes.map((n) => ({
  n,
  ...projectForStill(n.pos, W, H, SPREAD),
}));

// ---- Build SVG ----------------------------------------------------------
//
// SVG uses the same warm-neutral palette as the live canvas. The accent
// (--accent) is NOT used here — it's reserved for §2 per marketing.

const defs = `
  <defs>
    <radialGradient id="g-glow" cx="50%" cy="50%" r="50%">
      <stop offset="0%"   stop-color="#fff8e8" stop-opacity="0.95"/>
      <stop offset="35%"  stop-color="#e9e7df" stop-opacity="0.50"/>
      <stop offset="70%"  stop-color="#a89f8e" stop-opacity="0.10"/>
      <stop offset="100%" stop-color="#000000" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="g-bg" cx="50%" cy="35%" r="65%">
      <stop offset="0%"   stop-color="#1a1814" stop-opacity="0.6"/>
      <stop offset="100%" stop-color="${BG}"   stop-opacity="0"/>
    </radialGradient>
  </defs>
`;

const edges = graph.edges
  .map((e) => {
    const a = projected[e.a];
    const b = projected[e.b];
    const fade = Math.max(0, 1 - e.d / EDGE_CUTOFF);
    const opacity = (0.18 * (0.4 + fade * 0.6)).toFixed(3);
    return `<line x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" stroke="#a89f8e" stroke-width="0.9" stroke-opacity="${opacity}" />`;
  })
  .join('\n      ');

// Render nodes back-to-front by depth so closer ones overlap behind ones.
projected.sort((p, q) => p.depth - q.depth);
const nodes = projected
  .map(({ n, x, y, depth }) => {
    // Glow disc: scaled by node.size * depth so closer nodes feel larger.
    const r = 36 * n.size * (0.7 + depth * 0.6);
    const innerR = r * 0.18;
    const innerOpacity = (0.55 + depth * 0.35).toFixed(3);
    return `<g transform="translate(${x.toFixed(1)} ${y.toFixed(1)})">
        <circle r="${r.toFixed(1)}" fill="url(#g-glow)" />
        <circle r="${innerR.toFixed(2)}" fill="#fff8e8" fill-opacity="${innerOpacity}" />
      </g>`;
  })
  .join('\n      ');

const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" preserveAspectRatio="xMidYMid slice">
  <rect width="${W}" height="${H}" fill="${BG}"/>
  <rect width="${W}" height="${H}" fill="url(#g-bg)"/>
  ${defs}
  <g style="mix-blend-mode: screen">
      ${edges}
      ${nodes}
  </g>
</svg>
`;

const svgPath = resolve(OUT_DIR, 'hero-still.svg');
writeFileSync(svgPath, svg);

// ---- Rasterise to PNG + WebP via sharp ---------------------------------

const pngBuf = await sharp(Buffer.from(svg)).png({ quality: 92 }).toBuffer();
writeFileSync(resolve(OUT_DIR, 'hero-still.png'), pngBuf);

const webpBuf = await sharp(Buffer.from(svg)).webp({ quality: 86 }).toBuffer();
writeFileSync(resolve(OUT_DIR, 'hero-still.webp'), webpBuf);

console.log(`hero-still: ${graph.nodes.length} nodes, ${graph.edges.length} edges`);
console.log(`  ${svgPath}`);
console.log(`  ${resolve(OUT_DIR, 'hero-still.png')}  (${pngBuf.length} bytes)`);
console.log(`  ${resolve(OUT_DIR, 'hero-still.webp')} (${webpBuf.length} bytes)`);
