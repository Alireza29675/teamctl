---
description: Craft a curated public-facing GitHub release body for a new version. Voice-first writing surface; engineering handles the technical cascade separately.
allowed-tools: Bash, Read, Write, Edit
---

`/teamctl:release` is the writing surface for cutting a release. It produces the **public-facing GitHub release body**: short, curated, scannable. The thing your users see on `github.com/<org>/<repo>/releases` and (if you've wired it) what `teamctl update` prints after upgrade.

This skill stops at a ratified body. The technical cascade (version bumps, CHANGELOG dating, tag push, cargo-dist or equivalent, smoke tests) is engineering's job; the placeholders are demarcated below.

## Flow

Six beats, in order.

1. **Read the merge surface.** `git log --oneline <last-tag>..origin/main` and `gh pr list --state merged --search "merged:>=<date>"` to gather the PRs that landed. Read `CHANGELOG.md` `[Unreleased]` if engineering keeps one; that's the exhaustive surface, you'll write the curated short version.
2. **Sort into tiers.** Tier 1: the 2-3 changes that genuinely change what a user can DO (verbs, not nouns). Tier 2: 3-5 quiet wins. Cut: engineering-only changes (test deflakes, build hygiene, internal refactors). Hold: known-broken features; never lead the public with a foot-gun.
3. **Headline.** Triplet of verbs that names what's new (*"listen, look, attach"*, *"capture, route, ship"*). Owner-style colon form: `vX.Y.Z: <triplet>`. Em-dashes banned.
4. **Body.** Three short paragraphs (one per triplet verb), then a bulleted "quiet wins" block (Tier 2), then thanks (outside contributors) and a closer link out to the full changelog.
5. **Surface to ratify.** Pass the draft to whoever ratifies user-facing copy (owner, lead, you-yesterday). Include named variant questions so the ratify is one-round.
6. **Hand off the technical cascade.** Once ratified, pass the approved body to engineering to wire into `gh release create`. The placeholder checklist below is theirs to execute.

## Voice rails

The release body is the **most-screenshotted artifact** of any version cut. It lives on github.com long after you've forgotten it. Get it right.

- **Verb headlines beat noun headlines.** *listen, look, attach* tells the reader what they can DO. *voice, files, focus* is fine but weaker.
- **Two screens, not five.** Headline plus triplet plus 5-bullet block plus closer equals one scroll on a phone. Anything longer fails the screenshot test.
- **Emoji anchors.** One emoji per triplet verb, one per Tier 2 bullet. Used for scanability, not decoration.
- **Plain American English.** No marketing speak (*"unlock"*, *"empower"*, *"reimagine"*). Show what changed; trust the reader.
- **Cut engineering-only changes.** Test deflakes, CI hygiene, internal refactors don't land in the public body. They're in `CHANGELOG.md`.
- **Em-dashes banned.** Use colons, semicolons, or a fresh sentence. The em-dash invites prose drift; release bodies are short on purpose.
- **One opener sentence is plenty.** *"teamctl learned three new tricks this release."* beats *"We're excited to announce..."*.
- **Thanks outside contributors by handle.** Co-authored-by lines in commits earn a `🙏 Thanks to @handle` line at the bottom.
- **Closer points out, not in.** *"For the full picture: site.run/changelog"* or *"Full diff: vX.Y.Z...vA.B.C"*. Don't enumerate every PR in the body itself.

## The two-tier model

### Tier 1: the triplet

The three changes that **change what a user can do**. Each gets a short bold paragraph: one verb-anchored sentence, then 1-2 sentences of substance.

Example structure (from teamctl v0.8.0):

```
🎙️ **Listen.** Send a voice note to your manager in Telegram. Groq transcribes it, your team reads it like any other message.

👀 **Look.** The TUI grew up. Mouse-wheel scrolls the focused pane, `Ctrl+E` forwards keys straight into the agent's tmux pane, claude renders at full pane size instead of 80×24, and the Triptych layout no longer fights you.

📎 **Attach.** Drop a file path in the TUI compose pane and your agent reads the file when the message lands. Bring your own malware scanner if you want a gate; defaults work out of the box.
```

Rules for picking the triplet:

- Each item must answer *"what can a user now do that they couldn't last release?"*
- If a change isn't user-facing, it doesn't make Tier 1.
- Verbs in the headline triplet match the bold-lead verbs in the body.
- If you have more than three Tier-1 candidates, fold the weakest into Tier 2.

### Tier 2: the quiet wins

3-5 changes that matter but aren't headline-grabbers. Bullets, emoji anchors, colon-separated `**name**: short description` form.

Example structure:

```
And a few quiet wins underneath:

- 📬 **Lazy inbox**: agents see notification stubs instead of a firehose, so they stay on task.
- 💬 **Conversational `init`**: template picker out, domain-discovery conversation in.
- 🧱 **Stackable role prompts**: `role_prompt` now accepts a list of markdown files.
- 🔄 **Smarter `teamctl update`**: refreshes the Claude Code plugin alongside the binaries.
- 📖 **A docs rewrite worth a fresh look**: new README, new examples, fresh concept pages.
```

Rules for Tier 2:

- One visual line per bullet. The colon prevents prose drift.
- Backticks for command names and code identifiers.
- 3-5 bullets is the sweet spot. More than 5 and you're back to engineering catalog.

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
- [ ] **Docs-site changelog page sync** (if applicable). The closer link should resolve.
- [ ] **Announcement** (if applicable). Mastodon, BlueSky, Twitter, Slack channels.

Hand the ratified body and this checklist to engineering. The skill is done when the body is approved.

## When the body is wrong

Two common failure modes, and how to fix:

- **"Feels shattered" / "doesn't flow".** Usually means you wrote 5+ separate short paragraphs without grouping. Fix: collapse Tier 2 into the bulleted block; the triplet stays as three paragraphs but the rest folds.
- **"Reads like a changelog".** You included every PR. Cut to Tier 1 + 5 bullets max. The rest goes in `CHANGELOG.md`.

## Pruning

Treat this file like code. Review when a release body comes back with friction; prune rules that aren't catching real mistakes. The test:

> *Would removing this instruction cause a future release body to be weaker?*

If no, delete it. This skill is for the writing surface only; everything else belongs in engineering's release runbook.
