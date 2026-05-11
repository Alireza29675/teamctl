#!/usr/bin/env node
// Regenerates docs/src/content/docs/changelog.md from the repo-root
// CHANGELOG.md so teamctl.run/changelog is a 1:1 mirror of the in-tree
// source of truth. Runs as a pre-step on docs `dev`, `build`, and
// `preview`. The output file is gitignored; CHANGELOG.md is canonical.
//
// Optional intro: if docs/changelog-intro.md exists, its contents are
// spliced in above the rendered changelog body — that's Neda's slot
// for a per-release voice paragraph. Engineers don't edit it.

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DOCS_ROOT = resolve(HERE, '..');
const REPO_ROOT = resolve(DOCS_ROOT, '..');

const SRC = resolve(REPO_ROOT, 'CHANGELOG.md');
const INTRO = resolve(DOCS_ROOT, 'changelog-intro.md');
const OUT = resolve(DOCS_ROOT, 'src/content/docs/changelog.md');

const FRONTMATTER = `---
title: Changelog
description: Every release of teamctl, mirrored from the in-tree CHANGELOG.md at the repo root.
---

`;

const NEDA_SLOT_MARKER = `<!-- intro: Neda — drop a per-release voice paragraph into docs/changelog-intro.md and it will land here on the next build. -->`;

function stripLeadingHeading(md) {
  // CHANGELOG.md opens with `# Changelog` + a Keep-a-Changelog blurb.
  // Starlight already renders the page title from frontmatter, so we
  // skip both the H1 and the immediately-following blurb paragraph to
  // avoid double titling.
  const lines = md.split('\n');
  let i = 0;
  while (i < lines.length && lines[i].trim() === '') i++;
  if (i < lines.length && lines[i].startsWith('# ')) {
    i++;
    while (i < lines.length && lines[i].trim() === '') i++;
    // Drop the first paragraph after the H1 (the standard
    // Keep-a-Changelog format-blurb).
    while (i < lines.length && lines[i].trim() !== '') i++;
    while (i < lines.length && lines[i].trim() === '') i++;
  }
  return lines.slice(i).join('\n');
}

function injectVersionAnchors(md) {
  // Starlight auto-slugs `## [0.8.0] — 2026-05-11` to `#080--2026-05-11`.
  // Add an inline HTML anchor before each version heading so the
  // canonical short form (`#0-8-0`) also resolves — the docs ticket
  // calls it out by name and stable short hashes are friendlier to
  // share in release-note links.
  return md.replace(
    /^## \[(\d+)\.(\d+)\.(\d+)\]/gm,
    (line, major, minor, patch) => `<a id="${major}-${minor}-${patch}"></a>\n\n${line}`,
  );
}

const body = injectVersionAnchors(stripLeadingHeading(readFileSync(SRC, 'utf8')));
const intro = existsSync(INTRO) ? readFileSync(INTRO, 'utf8').trim() : NEDA_SLOT_MARKER;

const out = `${FRONTMATTER}${intro}\n\n${body.trimEnd()}\n`;
writeFileSync(OUT, out, 'utf8');

const rel = OUT.slice(REPO_ROOT.length + 1);
console.log(`sync-changelog: wrote ${rel} from CHANGELOG.md (${body.split('\n').length} body lines)`);
