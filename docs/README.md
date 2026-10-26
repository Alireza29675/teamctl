# TeamCTL documentation

Source for [teamctl.run](https://teamctl.run): the landing page and user documentation for TeamCTL, the open-source platform for autonomous teams of agents. Hosting uses Cloudflare Pages.

## Local development

Use Node.js **22.12 or newer** and the checked-in lockfile.

```bash
cd docs
npm ci
npm run dev        # hot-reloading dev server on :4321
npm run build      # static site to docs/dist/
npm run preview    # inspect the built site locally
```

The `predev`, `prebuild` and `prepreview` scripts sync the root changelog into the documentation. Review any resulting source changes before committing.

## Structure

- `src/pages/index.astro` — landing page.
- `src/content/docs/`: published Starlight pages for getting started, concepts, references, guides and cookbook.
- `adrs/`: source-only architecture decisions, excluded from public routes, search and sitemap.
- `brand-guide.md`: source-only brand guidance.
- `src/content.config.ts` — collection definition using `@astrojs/starlight/loaders`.
- `astro.config.mjs` — sidebar, site metadata and Starlight theme configuration.
- `src/styles/` — shared typography, palette and layout styles.
- `public/brand/` — the compact head logo, favicon exports and larger teamwork illustration.

The workflow [docs.yml](../.github/workflows/docs.yml) builds changes affecting `docs/**` and checks internal links. It also syncs [tools/install.sh](../tools/install.sh) into the served `/install` endpoint. That script is the installer source of truth; do not edit generated `public/install`.

With the configured Cloudflare credentials, a push to `main` or an explicitly dispatched workflow publishes `docs/dist` to the `teamctl-docs` Pages project. Local builds do not publish.

## Adding a page

1. Drop a `.md` or `.mdx` file under `src/content/docs/<section>/`. Include frontmatter with at least `title:`.
2. Add a sidebar entry in `astro.config.mjs`. Keep developer-only material outside `src/content/docs/` and `public/` so it is not published.
3. Run `npm run build`, inspect `npm run preview` and verify links.

## Publish a reviewed update

The maintainer authorized finishing the website checks and publishing the reviewed local version to **teamctl.run on 18 October 2026**. This authorization does not include a Git push or email send. The procedure below does not establish that publication has happened; record the actual deployment result in the dated task notes.

For this reviewed worktree, deploy directly from its repository root so Wrangler discovers both `docs/dist` and root `functions/`. The production upload uses branch label `main`; it does not change the local `codex/short-launch-page` branch.

```bash
cd /Users/alireza/dev/projects/teamctl/.worktrees/short-launch-page
teamctl_wrangler=/Users/alireza/.npm/_npx/32026684e21afda6/node_modules/.bin/wrangler
node --test docs/server/release-signup.test.mjs
install -m 0755 tools/install.sh docs/public/install
npm --prefix docs run build
"$teamctl_wrangler" pages deployment list --project-name teamctl-docs --environment production
```

Use Node 22.12+ and Wrangler **4.107.0**, with an authenticated account that can manage the existing Pages project. The binary above is the verified local installation; if it is no longer present, install that pinned version and authenticate again. Run `npm ci` from `docs/` when dependencies are absent or the lockfile changes.

Before production mutation, record the **currently serving successful production deployment ID** as the rollback target; confirm it in the Pages project's Deployments view rather than assuming the newest list entry is serving. Preserve its non-secret deployment/source metadata in the task record. Verify the production bindings and endpoint rate rule described below, inspect the final desktop/mobile/no-JavaScript behavior, internal links and assets, and confirm `docs/dist/_routes.json` invokes only `/api/release-signup`. Keep credentials and private environment files out of the upload and build output.

Once these checks pass, publish the reviewed assets and bundled Function:

```bash
"$teamctl_wrangler" pages deploy docs/dist --project-name teamctl-docs --branch main --commit-dirty=true
```

Record the returned deployment ID/URL and confirm success in Pages. Then inspect [teamctl.run](https://teamctl.run), the four walkthrough steps, comparison, assets and release form. A GET to `/api/release-signup` on the canonical host should return 405 with JSON and `Allow: POST`; another origin must be rejected. Verify the one-time temporary-contact signup/duplicate/opt-out/cleanup flow below without sending mail. Monitor the Pages deployment status and Function error/429 rates, Cloudflare WAF events and Resend request errors/contact membership. Do not log request bodies, email addresses or credentials during monitoring.

If upload fails, leave the previous production deployment serving and repair the failure before retrying. If post-deployment checks fail, use the recorded previous successful **Pages deployment ID**: in `teamctl-docs` → Deployments → that deployment's actions → **Rollback to this deployment**. The equivalent Pages API is `POST /accounts/{account_id}/pages/projects/teamctl-docs/deployments/{previous_deployment_id}/rollback`, with Pages Write permission. Preview deployments are not rollback targets. Wrangler 4.107.0 has no Pages rollback subcommand; `wrangler rollback` operates on Workers. Recheck the public site and signup behavior after rollback; project runtime bindings and WAF changes require their own review and must not be assumed reverted. [Pages rollback reference](https://developers.cloudflare.com/pages/configuration/rollbacks/).

The separate GitHub workflow remains available for a future authorized push/dispatch. It targets `teamctl-docs` and `docs/dist`, checks internal links, and uses the repository secrets `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. Do not push as a shortcut for this direct upload.

The [source-only brand guide](brand-guide.md) records logo roles, colours and wording. Procedure and CLI flags last verified: **18 October 2026**. Current successful production is `5a60e46a-7437-4897-9dbd-760c6e4e6c73` (created15:32:25UTC that day), including the mobile walkthrough scroll repair and public documentation boundary below. Its immediate rollback is `f2c18af6-f87e-4ae8-953e-9a4f641f1d9a`. Current user docs, artwork, search and sitemap pass; the signup runtime and bindings are unchanged from the verified Turnstile baseline. The earlier explicitly approved disposable-contact lifecycle passed on the pre-Turnstile baseline, including duplicate identity, preserved opt-out and final deletion/fresh 404 verification. No email sent. Record a new successful baseline before future changes; the original pre-rollout fallback is `6cac26f9-7d64-493f-83dc-bef2939c0b5e`.

## Local short-page review

The `codex/short-launch-page` branch contains a concise homepage variant. The full brand/docs draft remains in the separate `codex/brand-docs-rollout` worktree. Both are uncommitted local review work.

From the short-page worktree:

```bash
npm --prefix docs run build
npm --prefix docs run preview -- --host 127.0.0.1 --port 4323
```

For fast prototype iteration, run `npm --prefix docs run dev -- --host 127.0.0.1 --port 4323` instead of the static preview. This serves current source and reloads changes automatically; stop an existing server on the port before switching. It remains loopback only.

Open [the short preview](http://127.0.0.1:4323/). From the full-draft worktree, use the same preview command with port `4322` for [the full preview](http://127.0.0.1:4322/). Preview serves the last static build; rebuild after source edits. Stop each preview with Ctrl-C in its owning terminal. These commands serve loopback only and do not publish. If component markup updates but scoped styles retain old values during hot reload, stop and restart the owned dev server, then reload the preview; this cleared stale walkthrough styles during local review. Last verified: **18 October 2026**.

The homepage walkthrough is `src/components/landing/OrganizationStory.astro`. It illustrates four steps ending with Inspect: a roster, background sessions, messaging and the terminal UI; it does not run the displayed commands. To replace the static dashboard image with a recording, update the `controlRoom` import and associated image rendering there, keeping meaningful alternative text. Runtime ids in the example follow the schema (`gemini` is displayed as Gemini CLI).

On narrow screens (up to760px) and primary coarse-pointer devices, the walkthrough uses normal page flow and four tap controls at every viewport height. The full YAML determines its height and only long lines scroll horizontally. Tall fine-pointer desktop viewports retain the pinned scroll story; progress and button destinations measure the rendered pin height. Reserve two description lines so stage changes do not move the scene. When revising this layout, verify vertical and horizontal input starting over YAML, all four controls, landscape/tablet layouts, desktop forward/reverse scrolling and height-only changes across699/701px. A phone browser's expanding controls must not collapse or re-expand the story. Chromium viewport/input checks do not replace physical Safari/Android verification.

## Release-notification signup

The homepage form posts JSON to `/api/release-signup` with consent version `teamctl-next-release-v1`. Its server-only handler saves membership in the Resend segment **TeamCTL next release**. It retrieves contacts first, preserves global opt-outs, confirms existing membership and handles duplicate-create races through verified retrieval. Requests share one eight-second upstream deadline; rate limits return 429 and `Retry-After`. It does not send mail or add other campaigns. The key must have permission to manage contacts, rather than sending-only access. The form appears only after its JavaScript handler is ready; without JavaScript, the page explains how to enable signup.

For local development, copy `docs/.env.example` to `docs/.env`, set `RESEND_API_KEY` and `TURNSTILE_SECRET`, then run the hot-reloading dev command above. `RESEND_SEGMENT_ID` selects the release list. The public widget sitekey defaults to the project widget; set `PUBLIC_TURNSTILE_SITE_KEY` only to override it. The widget must permit the local hostname. The local environment file is ignored by Git; keep it private and never use a `PUBLIC_` name for the key. No credentials are included in browser code. Missing credentials return an unavailable state rather than a successful signup.

The development middleware and Cloudflare Pages Function use `server/release-signup.mjs`. The Pages route is at the repository root in `functions/api/release-signup.js`, matching the direct upload and workflow's root working directory. `public/_routes.json` is copied into the static build to restrict Function invocation to the signup route. Static `astro preview` does not run the API; use `astro dev` for the local signup, or a Pages Functions emulator for a built site.

Production prerequisites:

- Require encrypted `TURNSTILE_SECRET` and D1 binding `SIGNUP_REPLAY_DB` with the schema below, in addition to the Resend settings. See [Turnstile setup](#turnstile-signup-protection--18-october-revision) and [replay guard](#application-replay-guard).

- For the Resend configuration, set only `RESEND_API_KEY` and `RESEND_SEGMENT_ID` on the existing `teamctl-docs` project's **production** runtime. Segment ID: `e9a7afc4-5258-447c-ab1b-797c63e29073`. The key is secret; the segment ID is non-secret configuration, but the selected-key bulk command below stores both as encrypted bindings. Preserve every other binding and preview setting; compare binding names/types before and after without printing values.
- The Pages wrapper permits `https://teamctl.run` by default. `RELEASE_SIGNUP_ORIGIN` can explicitly replace that origin when required; keep the production canonical origin and do not permit `pages.dev` to bypass the zone rule. Local Vite development calls the shared handler directly and remains usable on loopback.
- Configure and verify a zone WAF rate-limiting rule for POST `/api/release-signup` on `teamctl.run`, with per-IP thresholds supported by the current plan. On18October the active rule **TeamCTL release signup** (`04fb09a5c07a44eab4565817eadac726`) was configured for canonical-host POSTs:5 requests per10seconds perIP, then a10second block (Freeplan). Verify the rule remains active and test throttling after deployment. Preserve unrelated WAF rules.
- The enabled Resend automation lookup returned none on 18 October 2026. Recheck before a real signup if account automation settings change. The release-only signup grants no permission for additional campaigns; author and explicitly approve the one-time notification only when the release is ready.

From the repository root above, pass only the selected runtime settings through private stdin. Keep shell tracing/debug logging off; never use `cat`, `echo`, `tee`, arguments or build variables to reveal credential values. Do not execute a broad environment upload.

```bash
set +x
set -o pipefail
node --input-type=module <<'NODE' | "$teamctl_wrangler" pages secret bulk --project-name teamctl-docs --env production
import { readFileSync } from 'node:fs';
import { parseEnv } from 'node:util';
const local = parseEnv(readFileSync('docs/.env', 'utf8'));
const settings = {
  RESEND_API_KEY: process.env.RESEND_API_KEY ?? local.RESEND_API_KEY,
  RESEND_SEGMENT_ID: process.env.RESEND_SEGMENT_ID ?? local.RESEND_SEGMENT_ID,
};
if (Object.values(settings).some((value) => typeof value !== 'string' || !value.trim())) {
  throw new Error('Missing signup runtime settings.');
}
process.stdout.write(JSON.stringify(settings));
NODE
```

This updates runtime settings; deploy the reviewed build afterward so the new deployment uses them. A missing runtime key makes the handler return safe unavailable JSON (503), never successful signup. To disable capture, remove only that binding and deploy the reviewed build without it, then verify 503; keep the static site and other bindings intact.

After deployment, create one uniquely named temporary test contact through the form, verify the dedicated segment, submit again and confirm the same contact, mark only that test contact unsubscribed and confirm another submission leaves it opted out, then delete that exact test contact and confirm it is gone. Contact create/update/delete operations must receive their tool approvals; send no welcome, confirmation or release email. Record only non-secret status and cleanup evidence. Check malformed, cross-origin, oversized and rate-limited requests without creating additional contacts.

Focused handler tests: `node --test docs/server/release-signup.test.mjs` from the repository root: **47 passed on 18 October 2026**, including Turnstile, atomic replay claims and stalled-storage deadline regressions. That day's live acceptance separately verified signup and duplicate success through the public UI with one contact ID and only the release segment; after global opt-out, the UI rejected signup, direct POST returned 409 and Resend still reported unsubscribed. The approved exact test contact was permanently deleted and a fresh lookup returned 404. The earlier diagnostic contact was also deleted and verified absent before the UI test. No emails were sent. These controlled checks do not establish email delivery or permission for another test contact/campaign.

## Diagnosing signup failures

When a valid signup fails while method/invalid-request checks pass, capture the current successful Pages deployment ID before a repair upload. Tail only requests carrying a dedicated smoke-test header, using `wrangler pages deployment tail <deployment-id> --project-name teamctl-docs --format pretty --header X-Teamctl-Smoke:release-20261018`. The wrapper logs only failure status and upstream status codes; never add request bodies, email addresses, URLs containing contacts, response bodies or credentials to diagnostics. Store raw tail output only in a private temporary file; retain sanitized findings and remove the temporary file afterward. Requests with `Accept: application/json` can expose Cloudflare's own error category separately from the handler's generic JSON failure.

The 18 October incident was reproduced in Miniflare/workerd with production compatibility date **2026-04-28**: `redirect: error` throws a TypeError before network I/O, although Node accepts it. The handler now uses **`redirect: manual`** and rejects non-success responses, so a redirect cannot forward the key. The 301/302/307/308 regressions and all 29 focused tests pass. Independent code/security review and the approved live lifecycle passed after repair. Diagnostic deployment `33a15e6b-9d2d-4fc6-bd36-3fa96519b4a8` is historical; use the successful repaired deployment `d915fda8-0b2a-4034-9933-36c7157e90fd` as the current working baseline. [Runtime implementation](https://github.com/cloudflare/workerd/blob/main/src/workerd/api/http.c%2B%2B).

For future fetch-option changes, run the shared handler inside an isolated Miniflare worker from the pinned Wrangler installation, with modules enabled and the production compatibility date. Use dummy credentials and a fake upstream that constructs the native workerd `Request` from the actual URL/options before returning contact/segment fixtures; this exercises option validation without network calls or subscribers. Verify the valid flow succeeds, all upstream calls share one deadline, and redirect/error fixtures remain failures. A Node mock that merely returns responses cannot catch workerd's Request-construction incompatibilities. Then deploy through the reviewed procedure and run only an explicitly approved disposable-contact lifecycle with exact cleanup.

## Turnstile signup protection — 18 October revision

The maintainer approved adding Turnstile to the release signup. The integration uses one managed widget named `teamctl.run (Spin)` in the existing Cloudflare account, for `teamctl.run`, `localhost` and `127.0.0.1`. Cloudflare Spin is the dashboard creation path; verification runs in the existing Pages Function. The application replay guard below uses a dedicated D1 database. Preserve the unrelated `wefork.dev` widget.

The public sitekey belongs in `PUBLIC_TURNSTILE_SITE_KEY` for the Astro build; the private value belongs only in `TURNSTILE_SECRET` in the ignored `docs/.env` for local development and the existing `teamctl-docs` production runtime. Preserve both Resend bindings and every preview setting. If copied from the dashboard, pass the secret directly from the clipboard into the selected private destination without printing it, and clear the clipboard afterward. Keep the local file mode0600 and verify it is ignored. Never place the secret in build variables or `PUBLIC_` settings.

Before any production runtime update, verify the finished server handler gates every Resend access on canonical Siteverify success, exact hostname from the canonical request URL, and action `release-signup`. Production must reject localhost results. Tokens must be bounded and single use; retry resets the widget. Use manual redirects, bounded requests/timeouts and generic errors. Missing configuration must fail closed. Retain the zone's existing per-IP WAF rule; Turnstile and throttling protect different layers. This does not prove mailbox ownership, and no email/double-opt-in behavior is added.

The pre-revision rollback target is `d915fda8-0b2a-4034-9933-36c7157e90fd`. Validate a real widget token against the protected backend and ensure replay is rejected before certifying enforcement. Isolated provider fixtures can verify success/retry UI without creating contacts. Do not describe a locally configured widget as deployed protection.

### Application replay guard

Real local widget validation returned success for an identical token twice, despite Turnstile's documented single-use contract. Do not certify provider replay rejection from mocks. The approved security revision therefore enforces one use in the application, after valid Siteverify and before Resend. `SIGNUP_REPLAY_DB` is a dedicated D1 binding; its canonical schema is `server/signup-replay.sql`. Store only SHA-256 token hashes and expiry timestamps, never addresses or raw tokens. An expiry index supports cleanup; an atomic unique insert rejects concurrent replays. Validate the challenge timestamp as well as hostname/action so an old token cannot become usable after cleanup. Missing/unavailable storage fails closed. Local Vite uses a bounded in-process store on loopback only; it is not production replay protection.

Provisioning target: existing Cloudflare account, new database `teamctl-release-guards`, EU jurisdiction. Before creating it, confirm it does not already exist. Use pinned Wrangler4.107.0 from the repository root, with shell tracing and Wrangler disk logs disabled:

```bash
"$teamctl_wrangler" d1 list
"$teamctl_wrangler" d1 create teamctl-release-guards --jurisdiction eu
"$teamctl_wrangler" d1 execute teamctl-release-guards --remote --file docs/server/signup-replay.sql
```

Record the returned database ID, initialize only the new table, and add only production binding `SIGNUP_REPLAY_DB` to the existing Pages project. Preserve preview and every other production binding. Pass only the private `TURNSTILE_SECRET` through stdin into `pages secret put TURNSTILE_SECRET --project-name teamctl-docs --env production`; do not bulk-upload the environment. The public widget identifier has a deliberate checked-in default in `index.astro` so CI does not silently build a disabled form; `PUBLIC_TURNSTILE_SITE_KEY` can override it.

Before publishing, run the full focused handler suite, a native workerd/D1 concurrency/replay check and genuine browser verification against an isolated fake contact service. After publishing, confirm missing/invalid tokens are rejected before subscriber access, and check a real verification with no email send. Monitor WAF, Siteverify rejection rates and D1 row/write quotas. Rollback to the recorded previous successful Pages deployment if checks fail; keep the new database/bindings for investigation and do not delete them automatically. An old deployment ignores these extra bindings, so removing the database is not required for rollback.

Runtime setup verified 18 October 2026: database `teamctl-release-guards` (`3bdc863b-194f-4411-8a9c-1b2762850e7a`, EU) has the schema and production binding `SIGNUP_REPLAY_DB`; encrypted `TURNSTILE_SECRET` was installed through private stdin. Existing Resend bindings, preview configuration and compatibility date 2026-04-28 were preserved. No contact or email data was written during setup. The request body limit is 4096 bytes, including a Turnstile token capped at 2048 characters. Siteverify, replay claim and Resend share an eight-second deadline; storage timeout returns 503.

For validation without contacts, use a loopback-only QA server with real Siteverify and fake Resend. A real widget submission passed, persistent success replaced the field, an HTML 429 kept the form and fresh-token retry worked. Identical-token replay returned 403 before the fake contact service. Native Miniflare/D1 independently verified one 200/one 403 for concurrent submissions and expiry cleanup. These checks create no subscribers and send no mail.

Turnstile baseline: `3ecc8dcb-ac31-40d4-a3b6-9c33eb56419a` (18 October 2026, 10:08:45 UTC). Public index and all five comparison marks match the reviewed build; live widget verification enables the form silently. GET405/AllowPOST, missing-token403, invalid-token403 and cross-origin403 checks passed without creating contacts. That revision used `d915fda8-0b2a-4034-9933-36c7157e90fd` as its rollback target. The current docs-only revision and rollback are recorded above. Full GitHub README and subsequent link cleanup were explicitly authorized and pushed, latest README-only commit `46f06eb`; other website source remains uncommitted. No email was sent.

## Public documentation boundary

The maintainer requested removal of Brand assets and ADRs from public docs on 18 October 2026. Their source lives in `brand-guide.md` and `adrs/`, outside Starlight collection and static assets. Remove navigation/footer/README links to those published pages. Before deployment, confirm the six routes, their sitemap entries and search documents are absent from the built output; static logo/artwork remain because the homepage and GitHub README use them. `public/_redirects` retires the old guide and ADR URLs with temporary 302 redirects to `/getting-started/`. Brand rules are exact paths so `/brand/` artwork remains available. After deployment, check the bare retired URLs for 302 and the expected Location, then follow to current user docs; required images must return 200 unchanged. Keep the previous successful Pages deployment as rollback. No backend, bindings or subscriber changes are required. [Pages redirects](https://developers.cloudflare.com/pages/configuration/redirects/).

If a retired path still returns cached 200 while the new Pages deployment URL and a fresh-query canonical request return the expected redirect, clear only the retired page URLs through the zone cache. In Cloudflare for teamctl.run, open Caching > Configuration > Custom Purge, select URL and submit the explicit old canonical, slashless and index.html variants, plus sitemap/search entry files if needed. Do not purge all unrelated assets. Recheck the unmodified public URLs for 302 to `/getting-started/`; a query-string bypass is diagnostic evidence only. Record purge success and the fresh public checks. [Cloudflare single-file purge procedure](https://developers.cloudflare.com/cache/how-to/purge-cache/purge-by-single-file/).

If exact URL purge does not clear old Pages copies, use Custom Purge > Prefix with `teamctl.run/brand` and `teamctl.run/adrs` (no scheme). This clears their route variants and cached brand files without deleting source or assets; normal requests refill current images. Prefix purges avoid custom cache-key limitations. If stale Pages responses still persist, follow the documented Pages fallback to Purge Everything for teamctl.run, after confirming the current deployment/rollback and checking for extra Cache Everything rules. Keep every configuration/binding/security setting unchanged. [Pages cache guidance](https://developers.cloudflare.com/pages/configuration/serving-pages/).

Verified publication on 18 October 2026: `f2c18af6-f87e-4ae8-953e-9a4f641f1d9a`. All five ADR URLs and exact brand-guide variants return302 to current user docs without a query-string bypass; user docs have no Brand assets or ADR navigation. Required brand images, sitemap and search entry match the build. Build contains33HTML pages and29search documents;1,987internal link targets pass. All47focused signup tests pass, runtime/artwork hashes and production bindings are unchanged. Before adding the explicit retirement rules, URL/prefix/zone purges did not clear stale200 copies; do not repeat purges as a substitute for route retirement. No Cache Rules or Worker routes were present.

Mobile scroll repair verified18October2026 at `5a60e46a-7437-4897-9dbd-760c6e4e6c73`: live homepage matches reviewed build; phone-emulated vertical input over YAML moves the page with pre.scrollTop0. Phone height changes across699/701px preserve stage/section height. Full YAML and native sideways input, all four touch-layout controls, stable tablet scene, desktop forward/reverse stages, reduced-motion and no-JavaScript fallback pass. Build33pages and1,988internal targets with zeroerrors;47signup tests pass. No runtime/routes/retirement-rule hashes or production bindings changed, no subscribers or mail. Physical Safari/Android compositor behavior remains unverified.
