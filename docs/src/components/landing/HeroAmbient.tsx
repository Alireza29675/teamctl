import { lazy, Suspense, useEffect, useState } from 'react';
import { hasWebGL, prefersReducedMotion } from '../../lib/motion';
// `?url` forces Vite/Astro to return plain string URLs rather than the
// image-metadata object Astro 5 hands to .astro files — React's <source>
// and <img> need strings or they render `[object Object]`.
import stillPng from '../../assets/landing/hero-still.png?url';
import stillWebp from '../../assets/landing/hero-still.webp?url';
import './hero.css';

// §1 — Ambient hero composition.
//
// Renders the design-equal still-image always (so first paint is complete
// before any JS hydrates), and overlays the live R3F canvas on top once
// the island hydrates IF the client supports WebGL and the user hasn't
// asked for reduced motion. The canvas fades in over the still — visitors
// on every path see the same artwork, never a black rectangle.
//
// dev3 wires this into index.astro as <HeroAmbient client:visible />.

const HeroCanvas = lazy(() => import('./HeroCanvas'));

const TAGLINE = 'Persistent AI coworkers, supervised like services.';

export default function HeroAmbient() {
  const [showCanvas, setShowCanvas] = useState(false);
  const [canvasFaded, setCanvasFaded] = useState(false);

  useEffect(() => {
    if (prefersReducedMotion()) return;
    if (!hasWebGL()) return;
    setShowCanvas(true);
    // Honour the user changing their motion preference at runtime.
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const listener = (e: MediaQueryListEvent) => {
      if (e.matches) {
        setShowCanvas(false);
        setCanvasFaded(false);
      } else if (hasWebGL()) {
        setShowCanvas(true);
      }
    };
    mq.addEventListener('change', listener);
    return () => mq.removeEventListener('change', listener);
  }, []);

  // Fade the canvas in a beat after mount so the still doesn't pop.
  useEffect(() => {
    if (!showCanvas) return;
    const id = window.setTimeout(() => setCanvasFaded(true), 120);
    return () => window.clearTimeout(id);
  }, [showCanvas]);

  return (
    <section className="hero" aria-label="teamctl">
      <div className="hero-stage">
        <picture className="hero-still" aria-hidden={showCanvas && canvasFaded}>
          <source srcSet={stillWebp} type="image/webp" />
          <img
            src={stillPng}
            alt=""
            decoding="async"
            loading="eager"
            draggable={false}
          />
        </picture>

        {showCanvas && (
          <div
            className="hero-canvas"
            data-faded={canvasFaded ? 'true' : 'false'}
          >
            <Suspense fallback={null}>
              <HeroCanvas />
            </Suspense>
          </div>
        )}
      </div>

      <div className="hero-copy">
        <h1 className="hero-tagline">{TAGLINE}</h1>
        {/* CTA is a placeholder — marketing/PM may finalise the wording. */}
        <a className="hero-cta" href="/getting-started/">
          Read the docs
          <span aria-hidden="true"> →</span>
        </a>
      </div>
    </section>
  );
}
