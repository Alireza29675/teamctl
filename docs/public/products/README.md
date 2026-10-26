# Comparison product marks

Verified 2026-10-18 against first-party product and announcement pages. Downloaded files are public assets only. These marks identify the named products; each belongs to its respective owner.

## Grok Bot

- Source: https://x.ai/bot
- Announcement: https://x.ai/news/introducing-grok-bot
- Official announcement image: https://x.ai/images/news/introducing-grok-bot-og-2.png
- The announcement image visibly shows a circle with two angled capsule eyes. This is distinct from the xAI or general Grok mark.
- The official product page renders the same static path geometry in `svg.grok-bot-mark` under `.footer-character` and several filled versions in its UI.
- No standalone public SVG URL was found. Exact DOM geometry and computed-style evidence are saved in `grok-bot-static.json` and `grok-bot-filled.json`.
- `grok-bot-static.svg` is a self-contained static presentation using those exact three official paths, filled Graphite #191B1A for the head and near-white #FCFCFC for the eyes. This presentation normalizes source colors; it does not redraw the logo.
- Do not use `grok-bot-official.svg`: it was an intermediate sampled animation frame.
- Confidence: high on official shape, high on source relation; standalone exported asset absent.

## Dots

- Source: https://chatgpt.com/features/dots/
- Exact asset: https://images.ctfassets.net/8su2tbn87fck/PulStHeDuz5begF06sEwk/c2080ae53243122046ba61074a5f0bd2/dots-o-updated.svg
- Local: `dots.svg`, 501 bytes, 81×81 SVG, a ring filled #BAC1D3.
- This is rendered by the official Dots feature page as the abstract O character/mark. Other character illustrations on that page represent individual dots, rather than one universal character.
- Confidence: high. Use the exact bytes; do not invent a dot arrangement or substitute the general OpenAI blossom.

## Muse

- Announcement: https://about.fb.com/news/2026/09/introducing-muse-personal-ai-agent/
- The announcement links directly to https://muse.ai/ in its rollout paragraph and to https://introducing.muse.ai/ under “How We Designed Muse”. This establishes that the assets belong to the same Meta Muse product.
- Exact favicon product mark: https://muse.ai/images/favicon/app-squiggle.svg
- Header variant: https://introducing.muse.ai/landing/MuseLogo.svg
- App-icon variant: https://muse.ai/images/landing/brand/muse-app-icon.svg
- Local: `muse.svg`, 2660 bytes, 100×100 SVG, blue gradient hand-drawn M/squiggle, transparent background.
- Confidence: high. Prefer the simple product squiggle for tiny inline use over the app tile or a personal Muse avatar.

## Gemini Spark

- Announcement: https://blog.google/innovation-and-ai/products/gemini-app/next-evolution-gemini-app/
- Product family source: https://gemini.google/
- Exact official family SVG: https://www.gstatic.com/lamda/images/gemini_sparkle_aurora_33f86dc0c0257da337c63.svg
- Official family favicon PNG: https://www.gstatic.com/lamda/images/gemini_sparkle_4g_512_lt_f94943af3be039176192d.png
- Local: `gemini-spark.svg`, 27794 bytes, 192×192 SVG, current four-color Gemini sparkle.
- The specific Gemini Spark announcement presents it inside the Gemini product family. A separate Spark-only logo was not found. Document this as the Gemini family mark, not a newly identified Spark-specific logo.
- Confidence: high on family logo; medium that no separate Spark-only mark is publicly provided.

## SVG checks

All recommended SVG files parse as XML and have no script elements, inline event handlers, JavaScript URLs, or remote resource references. Gemini contains a self-contained embedded JPEG data URI plus gradients; no external fetch is required.

## Local asset integrity
- grok-bot.svg: SHA-256 `0be77f1f9c722f5c84be131b9da9908053030d655edf15c760eb8ca64b5a8dc2`
- dots.svg: SHA-256 `1e351734a2db757f16f367140714cab4c999c5382ef428f914127efe65841ffc`
- muse.svg: SHA-256 `6373d89d274258a38432f3c3729cd716498b8ded849b174a7998683dcf4aaba5`
- gemini-spark.svg: SHA-256 `f56df33f86dc0c0257da337c63bf7ad68c0a1b27b796ec707e147984351bccb3`
