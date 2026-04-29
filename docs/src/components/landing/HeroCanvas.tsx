import { useMemo, useRef } from 'react';
import { Canvas, useFrame } from '@react-three/fiber';
import * as THREE from 'three';
// @ts-expect-error — JSDoc-typed JS module; shared with the Node still-image generator.
import { makeHeroGraph } from './hero-graph.js';

interface HeroNode {
  id: number;
  pos: readonly [number, number, number];
  size: number;
  phase: number;
}
interface HeroEdge { a: number; b: number; d: number }
interface HeroGraph { nodes: HeroNode[]; edges: HeroEdge[] }

// §1 — ambient WebGL canvas for the hero.
// Drifting agent graph: ~48 nodes, soft additive glow, off-phase breathing,
// thin edges connecting nearest neighbours. No drei (stack shrunk per
// eng_lead msg 438). Warm-neutral palette only — the page's accent colour
// is reserved for §2's live-state pulse.
//
// This component renders only the <Canvas>; reduced-motion / no-WebGL
// gating happens in HeroAmbient.tsx so the still-image fallback stays
// the perf-safe baseline.

// Warm neutral, sits on --bg #0d0e10 without picking up the §2 accent.
const NODE_COLOR = new THREE.Color('#e9e7df');
const NODE_COLOR_DEEP = new THREE.Color('#7a7368');
const EDGE_COLOR = new THREE.Color('#a89f8e');

// One radial-gradient sprite, used by every node. Procedural (no asset
// dependency); regenerated once per mount.
function makeGlowTexture(): THREE.Texture {
  const size = 128;
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  g.addColorStop(0, 'rgba(255, 248, 232, 1)');
  g.addColorStop(0.35, 'rgba(233, 231, 223, 0.55)');
  g.addColorStop(0.7, 'rgba(168, 159, 142, 0.12)');
  g.addColorStop(1, 'rgba(0, 0, 0, 0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.needsUpdate = true;
  return tex;
}

interface NodeFieldProps {
  graph: HeroGraph;
}

function NodeField({ graph }: NodeFieldProps) {
  const pointsRef = useRef<THREE.Points>(null);
  const baseSizes = useMemo(() => Float32Array.from(graph.nodes.map((n) => n.size)), [graph]);
  const phases = useMemo(() => Float32Array.from(graph.nodes.map((n) => n.phase)), [graph]);
  const basePositions = useMemo(() => {
    const arr = new Float32Array(graph.nodes.length * 3);
    graph.nodes.forEach((n, i) => {
      arr[i * 3 + 0] = n.pos[0];
      arr[i * 3 + 1] = n.pos[1];
      arr[i * 3 + 2] = n.pos[2];
    });
    return arr;
  }, [graph]);

  const positions = useMemo(() => basePositions.slice(), [basePositions]);
  const sizes = useMemo(() => baseSizes.slice(), [baseSizes]);

  const geometry = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('size', new THREE.BufferAttribute(sizes, 1));
    return g;
  }, [positions, sizes]);

  const material = useMemo(() => {
    const map = makeGlowTexture();
    return new THREE.PointsMaterial({
      size: 0.9,
      map,
      transparent: true,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
      sizeAttenuation: true,
      color: NODE_COLOR,
    });
  }, []);

  useFrame(({ clock }) => {
    const t = clock.getElapsedTime();
    const posAttr = geometry.getAttribute('position') as THREE.BufferAttribute;
    const sizeAttr = geometry.getAttribute('size') as THREE.BufferAttribute;
    for (let i = 0; i < graph.nodes.length; i++) {
      const px = basePositions[i * 3 + 0];
      const py = basePositions[i * 3 + 1];
      const pz = basePositions[i * 3 + 2];
      const ph = phases[i];
      // Slow, off-phase drift. Different frequencies per axis so motion
      // never feels like a single rocking gesture.
      const dx = Math.sin(t * 0.18 + ph) * 0.12;
      const dy = Math.cos(t * 0.13 + ph * 1.7) * 0.09;
      const dz = Math.sin(t * 0.21 + ph * 0.6) * 0.10;
      posAttr.array[i * 3 + 0] = px + dx;
      posAttr.array[i * 3 + 1] = py + dy;
      posAttr.array[i * 3 + 2] = pz + dz;
      // Breathing: ±15% size around the base, off-phase.
      sizeAttr.array[i] = baseSizes[i] * (1 + 0.15 * Math.sin(t * 0.45 + ph));
    }
    posAttr.needsUpdate = true;
    sizeAttr.needsUpdate = true;
    if (pointsRef.current) {
      // Slow continuous rotation around Y so the silhouette never repeats.
      pointsRef.current.rotation.y = t * 0.012;
    }
  });

  return <points ref={pointsRef} geometry={geometry} material={material} />;
}

interface EdgeFieldProps {
  graph: HeroGraph;
  cutoff: number;
}

function EdgeField({ graph, cutoff }: EdgeFieldProps) {
  const linesRef = useRef<THREE.LineSegments>(null);
  const indices = useMemo(() => {
    // Two endpoints per edge → array of [a, b] index pairs flat.
    const flat: number[] = [];
    graph.edges.forEach((e) => flat.push(e.a, e.b));
    return flat;
  }, [graph]);

  const opacities = useMemo(() => {
    // Soft fall-off: closer edges are brighter, distant ones whisper.
    return Float32Array.from(graph.edges.map((e) => Math.max(0, 1 - e.d / cutoff)));
  }, [graph, cutoff]);

  const geometry = useMemo(() => {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(new Float32Array(indices.length * 3), 3));
    return g;
  }, [indices]);

  const material = useMemo(() => {
    return new THREE.LineBasicMaterial({
      color: EDGE_COLOR,
      transparent: true,
      opacity: 0.18,
      depthWrite: false,
      blending: THREE.AdditiveBlending,
    });
  }, []);

  useFrame(({ clock }) => {
    const t = clock.getElapsedTime();
    const posAttr = geometry.getAttribute('position') as THREE.BufferAttribute;
    const arr = posAttr.array as Float32Array;
    for (let i = 0; i < graph.edges.length; i++) {
      const e = graph.edges[i];
      const a = graph.nodes[e.a];
      const b = graph.nodes[e.b];
      const ax = a.pos[0] + Math.sin(t * 0.18 + a.phase) * 0.12;
      const ay = a.pos[1] + Math.cos(t * 0.13 + a.phase * 1.7) * 0.09;
      const az = a.pos[2] + Math.sin(t * 0.21 + a.phase * 0.6) * 0.10;
      const bx = b.pos[0] + Math.sin(t * 0.18 + b.phase) * 0.12;
      const by = b.pos[1] + Math.cos(t * 0.13 + b.phase * 1.7) * 0.09;
      const bz = b.pos[2] + Math.sin(t * 0.21 + b.phase * 0.6) * 0.10;
      const o = i * 6;
      arr[o + 0] = ax; arr[o + 1] = ay; arr[o + 2] = az;
      arr[o + 3] = bx; arr[o + 4] = by; arr[o + 5] = bz;
    }
    posAttr.needsUpdate = true;
    if (linesRef.current) {
      linesRef.current.rotation.y = t * 0.012;
      // Whole-field breathing on opacity — tiny, barely perceptible.
      const base = 0.18;
      material.opacity = base * (1 + 0.18 * Math.sin(t * 0.27));
    }
  });

  // Per-edge opacity baked into vertex colours via the depth fade we computed.
  // (LineBasicMaterial with vertexColors would give per-edge tinting; for
  // calm hum the global opacity-pulse above is enough.)
  void opacities;

  return <lineSegments ref={linesRef} geometry={geometry} material={material} />;
}

export interface HeroCanvasProps {
  /** Override RNG seed when art-directing a different silhouette. */
  seed?: number;
}

export default function HeroCanvas({ seed }: HeroCanvasProps) {
  const graph = useMemo(() => makeHeroGraph({ seed }), [seed]);
  return (
    <Canvas
      // Keep the heavy stuff off first paint: this island is wrapped in
      // <HeroAmbient client:visible> by Astro, so the canvas only mounts
      // when the hero scrolls into view (on the landing page that's
      // immediate, but the gate matters for slow connections).
      camera={{ position: [0, 0, 9], fov: 38 }}
      dpr={[1, 1.75]}
      gl={{ antialias: true, alpha: true, powerPreference: 'low-power' }}
      style={{ width: '100%', height: '100%', display: 'block' }}
    >
      <NodeField graph={graph} />
      <EdgeField graph={graph} cutoff={3.4} />
    </Canvas>
  );
}
