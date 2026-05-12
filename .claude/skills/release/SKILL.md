---
name: release
description: Use when writing a public-facing GitHub release announcement for a new version. Produces the curated short body users see on the releases page. Picks a shape that fits the release size (triplet for major, bullets for polish, single paragraph for hotfix). Separates the writer's surface from engineering's technical cascade (version bump, CHANGELOG.md dating, tag push, gh release create, cargo-dist or equivalent + smoke tests).
---

# Release body

This skill is the writing surface for cutting a release. It produces the **public-facing GitHub release body**: short, curated, scannable. The thing your users see on `github.com/<org>/<repo>/releases` and (if you've wired it) what an update command prints after upgrade.

This skill stops at a ratified body. The technical cascade (version bumps, CHANGELOG dating, tag push, cargo-dist or equivalent, smoke tests) is engineering's job; the placeholders are demarcated below.

## Flow

Six beats, in order.

1. **Read the merge surface.** `git log --oneline <last-tag>..origin/main` and `gh pr list --state merged --search "merged:>=<date>"` to gather the PRs that landed. Read `CHANGELOG.md` `[Unreleased]` if engineering keeps one; that's the exhaustive surface, you'll write the curated short version.
2. **Sort into tiers.** Tier 1: the changes that genuinely change what a user can DO (verbs, not nouns). Tier 2: the quiet wins. Cut: engineering-only changes (test deflakes, build hygiene, internal refactors). Hold: known-broken features; never lead the public with a foot-gun.
3. **Pick the shape.** Count weighty Tier-1 features. Three means **triplet shape**. Zero to two means **bullets shape**. A single fix means **hotfix shape**. See *Shape variants* below. The shape determines headline and body structure.
4. **Read the last shipped body side-by-side.** Match its rhythm: word count per paragraph, sentence count, bullet density. Drift on rhythm is the most common reason a body comes back "not nice" despite hitting the right structure.
5. **Surface to ratify.** Pass the draft to whoever ratifies user-facing copy (owner, lead, you-yesterday). Include named variant questions so the ratify is one-round.
6. **Hand off the technical cascade.** Once ratified, pass the approved body to engineering to wire into `gh release create`. The placeholder checklist below is theirs to execute.

## Voice rails

The release body is the **most-screenshotted artifact** of any version cut. It lives on github.com long after you've forgotten it. Get it right.

- **Shorter is better.** Owner directive (msg 2027): *"the shorter descriptions the better."* When in doubt, cut the second sentence. One short clause beats two competent ones. The reader's eye should glide; if it stumbles on density, you wrote too much.
- **Match precedent rhythm.** Before surfacing, read the last shipped body side-by-side. Match: word count per paragraph (20-25 target for triplet ¶s), sentence count per paragraph (2 max, often 1), first-word-is-user-action (*"Send a voice note"* not *"`teamctl init` retired..."*), one-thought-per-bullet (no semicolon-stacked sub-facts). If your draft feels heavier on the tongue than the precedent, the rhythm is off.
- **Two screens, not five.** A whole body should fit roughly one phone scroll. Anything longer fails the screenshot test.
- **Plain American English.** No marketing speak (*"unlock"*, *"empower"*, *"reimagine"*). Show what changed; trust the reader.
- **One opener sentence is plenty.** *"teamctl learned three new tricks this release."* beats *"We're excited to announce..."*. A bridge sentence connects headline to body; one is enough.
- **Verb headlines beat noun headlines (when you use a triplet).** *listen, look, attach* tells the reader what they can DO. *voice, files, focus* is fine but weaker.
- **Emoji anchors.** One emoji per triplet paragraph or Tier 2 bullet. Used for scanability, not decoration.
- **Cut engineering-only changes.** Test deflakes, CI hygiene, internal refactors don't land in the public body. They're in `CHANGELOG.md`.
- **Em-dashes banned.** Use colons, semicolons, or a fresh sentence. The em-dash invites prose drift; release bodies are short on purpose.
- **Thanks outside contributors by handle (standing rule).** Every release with external Co-Authored-By trailers gets a `🙏 Thanks to @<handle> ...` line at the bottom. **Verify the handle from source**, never from a display-name guess:
  - Commit trailer: `git log --format='%(trailers:key=Co-authored-by)' <range>`. Handle is between `+` and `@` in `<id+handle@users.noreply.github.com>`.
  - PR author: `gh pr view <N> --json author --jq '.author.login'`.
  - Owner-supplied profile URL: `github.com/<handle>` → `<handle>`.
  - "Hamed Fathi" the display name does NOT imply `@HamedFathi` the handle. When unsure, ask.

## Shape variants

Pick the shape before drafting. The release's substance dictates the shape, not the other way around. Forcing a triplet onto a polish release is the most common drift.

### Triplet shape (0.8.0-style)

Use when three genuinely user-facing, weighty new features shipped. Each anchors a paragraph.

Structure:

```
<bridge sentence>

<emoji> **Verb1.** <paragraph 1, 20-25 words, 2 sentences max, user-anchored>

<emoji> **Verb2.** <paragraph 2>

<emoji> **Verb3.** <paragraph 3>

And a few quiet wins underneath:

- <emoji> **<name>**: <one-thought clause>.
- ... (3-5 bullets total)

🙏 Thanks to @<handle> for outside contributions.
```

Title: `vX.Y.Z: <verb1>, <verb2>, <verb3>`. Em-dashes banned, so colon form.

Example: v0.8.0 release body (`listen, look, attach`).

Rules for picking the triplet:

- Each verb must answer *"what can a user now do that they couldn't last release?"*
- If a change isn't user-facing, it doesn't make Tier 1.
- Verbs in the headline match the bold-lead verbs in the body.
- If you have more than three Tier-1 candidates, fold the weakest into Tier 2.
- If you have fewer than three, switch to the bullets shape.

### Bullets shape (polish / patch releases)

Use when fewer than three weighty Tier-1 features shipped. Polish releases, smaller patches, infrastructure wins that have user-visible impact but don't each carry a paragraph.

Structure:

```
<bridge sentence framing what the release is about>

- <emoji> **<name>**: <one-thought clause>.
- <emoji> **<name>**: <one-thought clause>.
- ... (3-7 bullets total)

🙏 Thanks to @<handle> for outside contributions.
```

Title: `vX.Y.Z: <short descriptive phrase>` or just `vX.Y.Z`. No forced three-word triplet.

When to choose this over the triplet: if you find yourself stretching for a third weighty verb, or if every triplet candidate is *"polish"* rather than *"new capability"*, drop the triplet and use bullets. Owner-ratified guidance: *"all of the releases should not be this large and you don't need to always pick three words"* (tg 1908).

### Hotfix shape (0.8.1-style)

Use when the release is essentially a single fix.

Structure:

```
<X.Y.Z> fixes <what>.

<emoji> **<Fix-name>.** <one-paragraph description: what was broken, what changed>.

<emoji> <Companion-fix-name if any>. <short paragraph>.

<one-line upgrade instruction if relevant>.

🙏 Thanks to <whoever helped diagnose>.
```

Title: `vX.Y.Z` or `vX.Y.Z: <fix nickname>`. No triplet.

Example: v0.8.1 release body (single bash 3.2 hotfix on macOS).

## Tier 2 bullet rules (applies to triplet and bullets shapes)

- One visual line per bullet. The colon prevents prose drift.
- One thought per bullet, ideally **one short clause**. Cut the second sentence when in doubt. No semicolon-stacked sub-facts.
- Backticks for command names and code identifiers.
- 3-5 bullets is the sweet spot for triplet shape; 3-7 for bullets shape. More than that and you're back to engineering catalog.

## What to cut

Most release notes include too much. The cuts matter as much as the keeps.

- **Test deflakes.** Engineering quality work, no user-visible change.
- **Build hygiene.** CI configs, dependency bumps, lint fixes.
- **Internal refactors.** Function renames, module splits, schema cleanups.
- **Per-MCP-tool callouts when the tool is invisible to the user.** Users don't tool-shop; they use features.
- **Stack changes that don't change what the user does.** New crates, vendored libs, build-system tweaks.
- **Known-broken features.** Either fix before cut, or note in a small "Known issues" line at the bottom. Never lead with them.

These all belong in `CHANGELOG.md`, which is engineering's exhaustive surface. The release body is the **curated** surface.

## Technical cascade: engineering's surface

After the body is ratified, the technical cascade is engineering's job. This skill stops at the ratified body. The placeholders below name what engineering still needs to do.

- [ ] **Version selection.** Semver call (major, minor, patch). Engineering judges blast radius.
- [ ] **`CHANGELOG.md` dating.** Rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD`. Open a fresh `[Unreleased]` block for the next cycle.
- [ ] **Manifest version bumps.** `Cargo.toml` (workspace + per-crate as needed), `package.json`, or equivalent.
- [ ] **Commit + tag.** `chore(release): bump to X.Y.Z` followed by `git tag vX.Y.Z`.
- [ ] **Tag push.** Triggers cargo-dist or equivalent release pipeline.
- [ ] **`gh release create vX.Y.Z`** with the ratified body. Install tables and asset uploads are auto-generated by cargo-dist (or the equivalent in your stack).
- [ ] **Smoke test.** Fresh-machine install via the release URL. Verify binaries report the new version.
- [ ] **Announcement** (if applicable). Mastodon, BlueSky, Twitter, Slack channels.

Hand the ratified body and this checklist to engineering. The skill is done when the body is approved.

## When the body is wrong

Three common failure modes, and how to fix:

- **"Feels shattered" / "doesn't flow".** Usually 5+ separate short paragraphs without grouping. Fix: collapse Tier 2 into the bulleted block; the triplet stays as three paragraphs but the rest folds.
- **"Reads like a changelog".** You included every PR. Cut to Tier 1 plus 5 bullets max. The rest goes in `CHANGELOG.md`.
- **"Not nice" / "we had a convention".** Structure right, rhythm wrong. Run the side-by-side diff against the last shipped body: word count per paragraph, sentence count, bullet density, first-word-is-user-action. If your draft feels heavier on the tongue than the precedent, the rhythm is off.

## Pruning

Treat this file like code. Review when a release body comes back with friction; prune rules that aren't catching real mistakes. The test:

> *Would removing this instruction cause a future release body to be weaker?*

If no, delete it. This skill is for the writing surface only; everything else belongs in engineering's release runbook.
