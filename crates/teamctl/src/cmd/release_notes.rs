//! Release-body fetch, parse, and render for `teamctl update`'s
//! post-success "what's new" display and the `teamctl whatsnew`
//! subcommand. The GitHub release body is the source of truth for the
//! user-facing voice piece; this module turns its markdown into a
//! framed terminal block.
//!
//! Two display modes share the parser + renderer:
//!
//! - **Single-version** — `teamctl whatsnew [version]` or a post-update
//!   call where there's only one body to show. Frame: `✨ What's new
//!   in v<X.Y.Z>`.
//! - **Aggregate / range** — `teamctl whatsnew --since X` or a
//!   post-update call that crossed multiple versions. Frame: `✨
//!   What's new in v<from> → v<to>`. Each version's body rendered
//!   below its own `v<X.Y.Z>` subheader, oldest-first.
//!
//! Floor: pre-`FLOOR_VERSION` releases (cargo-dist auto-generated
//! noise) are silently excluded from aggregates. Operator can still
//! ask for them by name with `teamctl whatsnew <ver>`, but the raw
//! fallback kicks in since their markdown shape isn't the convention.

use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::cmd::update::{compare_versions, VersionOrder, CURRENT_VERSION};

const RELEASES_TAG_API: &str = "https://api.github.com/repos/Alireza29675/teamctl/releases/tags/";
const RELEASES_LIST_API: &str = "https://api.github.com/repos/Alireza29675/teamctl/releases";

/// First version with curated release-body convention. Anything below
/// this is cargo-dist auto-generated and silently excluded from
/// aggregate displays.
const FLOOR_VERSION: &str = "0.8.0";

/// Public changelog URL referenced in the footer of every rendered
/// "what's new" block. Lives at the Astro Starlight site (ships in
/// #169 / T-169 alongside this PR).
const CHANGELOG_URL: &str = "https://teamctl.run/changelog";

/// ANSI escape opening italic + dim. Pairs with [`STYLE_RESET`].
const STYLE_DIM_ITALIC: &str = "\x1b[2;3m";
const STYLE_RESET: &str = "\x1b[0m";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub headline: String,
    pub description: String,
}

/// One release as returned by the GitHub /releases list endpoint —
/// just the two fields the renderer needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseEntry {
    pub version: String,
    pub body: String,
}

/// Fetch the GitHub release-body markdown for a tagged version. The
/// tag is the v-prefixed shape (`v0.8.0`); callers pass either form.
pub fn fetch_release_body(version: &str) -> Result<String> {
    let v = version.trim_start_matches('v');
    let url = format!("{RELEASES_TAG_API}v{v}");
    let raw = curl_get(&url)?;
    extract_body_field(&raw)
        .ok_or_else(|| anyhow!("no `body` field in GitHub releases response for v{v}"))
}

/// Subset of the GitHub releases-API response we actually read. All
/// other fields are ignored on parse. `body` is `Option` to cover both
/// the `"body": null` and the field-missing shapes the API has been
/// observed in.
#[derive(Deserialize)]
struct ReleaseResponse {
    #[serde(default)]
    body: Option<String>,
}

/// Pull the `body` value out of a GitHub releases-API JSON blob.
/// Returns `None` when the field is missing, `null`, or empty — all
/// three mean "no release notes to show." Parse failure (malformed
/// JSON) also resolves to `None`; the caller falls back to the
/// release-link line rather than raising.
fn extract_body_field(json: &str) -> Option<String> {
    let parsed: ReleaseResponse = serde_json::from_str(json).ok()?;
    let body = parsed.body?;
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Fetch the list of all GitHub releases (newest-first per the API)
/// and parse the tag + body fields out of each. Used by the aggregate
/// (`--since` and post-update multi-version) display.
pub(crate) fn fetch_releases_list() -> Result<Vec<ReleaseEntry>> {
    let raw = curl_get(RELEASES_LIST_API)?;
    let list = parse_releases_list(&raw);
    if list.is_empty() {
        bail!("no usable releases in GitHub releases response");
    }
    Ok(list)
}

/// Parse the `/releases` JSON array into `(tag, body)` pairs. Returns
/// empty Vec on malformed JSON (tolerant: a parse failure on the list
/// endpoint shouldn't break `teamctl update`'s install path).
fn parse_releases_list(json: &str) -> Vec<ReleaseEntry> {
    let v: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let tag = item
                .get("tag_name")?
                .as_str()?
                .trim_start_matches('v')
                .to_string();
            if tag.is_empty() {
                return None;
            }
            let body = item
                .get("body")
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string();
            Some(ReleaseEntry { version: tag, body })
        })
        .collect()
}

/// `true` when `version` is below the curated-display floor. Pre-floor
/// bodies are excluded from aggregate displays as cargo-dist noise.
pub(crate) fn is_below_floor(version: &str) -> bool {
    matches!(
        compare_versions(version.trim_start_matches('v'), FLOOR_VERSION),
        VersionOrder::Older
    )
}

/// Select all releases in `(from, to]` that sit at or above the floor.
/// `from` is exclusive (you've already seen it), `to` is inclusive (it's
/// the new version). Returned oldest-first so the operator reads in
/// chronological order.
pub(crate) fn select_range(all: &[ReleaseEntry], from: &str, to: &str) -> Vec<ReleaseEntry> {
    let from = from.trim_start_matches('v');
    let to = to.trim_start_matches('v');
    let mut selected: Vec<ReleaseEntry> = all
        .iter()
        .filter(|r| !is_below_floor(&r.version))
        .filter(|r| matches!(compare_versions(&r.version, from), VersionOrder::Newer))
        .filter(|r| !matches!(compare_versions(&r.version, to), VersionOrder::Newer))
        .cloned()
        .collect();
    selected.sort_by(|a, b| match compare_versions(&a.version, &b.version) {
        VersionOrder::Older => std::cmp::Ordering::Less,
        VersionOrder::Equal => std::cmp::Ordering::Equal,
        VersionOrder::Newer => std::cmp::Ordering::Greater,
    });
    selected
}

/// Parse `# Headline\nDescription` pairs out of a release-body markdown
/// string. Lines starting with exactly `# ` are headlines; lines after
/// a headline (until the next `# ` or EOF) are the description.
///
/// Non-conforming-body detection: returns an empty Vec when the body
/// contains markdown that the curated convention doesn't allow —
/// sub-headings (`## `, `### `, ...) or fenced code blocks (` ``` `).
/// That keeps the cargo-dist auto-generated body shape (which uses
/// `# team-bot 0.7.3` as section labels around install-shell snippets)
/// from being mis-rendered as styled "entries"; the caller falls
/// through to raw display.
pub(crate) fn parse_release_body(body: &str) -> Vec<Entry> {
    if has_non_convention_markdown(body) {
        return Vec::new();
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in body.lines() {
        if let Some(headline) = line.strip_prefix("# ") {
            if let Some((h, desc)) = current.take() {
                entries.push(Entry {
                    headline: h,
                    description: desc.join("\n").trim().to_string(),
                });
            }
            current = Some((headline.trim().to_string(), Vec::new()));
        } else if let Some((_, desc)) = current.as_mut() {
            desc.push(line.to_string());
        }
    }
    if let Some((h, desc)) = current {
        entries.push(Entry {
            headline: h,
            description: desc.join("\n").trim().to_string(),
        });
    }
    entries
}

/// Detect markdown the curated release-body convention deliberately
/// excludes. Sub-headings (`## ` and deeper) and fenced code blocks
/// (` ``` `) both signal "this is a richer markdown body, not the
/// curated voice piece" — most commonly the cargo-dist auto-generated
/// body. When either is present, the parser short-circuits to "no
/// entries" and the renderer falls through to raw display.
fn has_non_convention_markdown(body: &str) -> bool {
    body.lines()
        .any(|l| l.starts_with("## ") || l.starts_with("```"))
}

/// Per-crate install sections cargo-dist appends to every release body —
/// one heading per crate (`# team-bot 0.8.1`, `# teamctl 0.8.1`, ...)
/// with shell-install snippets and sha256 tables underneath. Truncate
/// the body at the first such heading so the curated prose stands
/// alone. Older releases / hand-edited bodies without these headings
/// are returned unchanged.
fn truncate_at_cargo_dist(body: &str) -> &str {
    let mut idx = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if is_cargo_dist_install_heading(trimmed) {
            return &body[..idx];
        }
        idx += line.len();
    }
    body
}

/// Match `^# <known-crate> <MAJOR>.<MINOR>.<PATCH>$` exactly. The
/// known-crate set is the four workspace crates cargo-dist publishes;
/// add to this list when a new crate joins the release.
fn is_cargo_dist_install_heading(line: &str) -> bool {
    // Order matters here only for the "teamctl-ui" / "teamctl" pair
    // because `strip_prefix("teamctl")` succeeds on "teamctl-ui ..." —
    // but the following byte is `-`, not the space we require, so the
    // semver check fails cleanly. The check is robust regardless of
    // order; the alphabetical list is just for readability.
    const KNOWN_CRATES: &[&str] = &["team-bot", "team-mcp", "teamctl", "teamctl-ui"];
    let rest = match line.strip_prefix("# ") {
        Some(r) => r,
        None => return false,
    };
    for c in KNOWN_CRATES {
        if let Some(after_crate) = rest.strip_prefix(c) {
            if let Some(ver) = after_crate.strip_prefix(' ') {
                if is_three_part_semver(ver) {
                    return true;
                }
            }
        }
    }
    false
}

/// `true` if `s` is exactly `MAJOR.MINOR.PATCH` with all-digit
/// components. Trailing pre-release / build-metadata suffixes are
/// rejected — cargo-dist's install headings never carry them.
fn is_three_part_semver(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Render the body of a single release — entries with styled
/// descriptions, or raw fallback. No top frame, no footer. Use
/// [`render`] for the single-version display and [`render_range`] for
/// aggregate. The body is pre-truncated at the first cargo-dist
/// install heading so install tables / sha256 hex don't bury the
/// curated voice piece (see #197).
fn render_body(body: &str) -> String {
    let body = truncate_at_cargo_dist(body);
    let entries = parse_release_body(body);
    if entries.is_empty() {
        let trimmed = body.trim_end();
        if trimmed.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str(trimmed);
        out.push('\n');
        return out;
    }
    let mut out = String::new();
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&e.headline);
        out.push('\n');
        for line in e.description.lines() {
            out.push_str("  ");
            out.push_str(STYLE_DIM_ITALIC);
            out.push_str(line);
            out.push_str(STYLE_RESET);
            out.push('\n');
        }
    }
    out
}

/// Single-version frame line — `✨ What's new in v<X.Y.Z>` followed by
/// a blank line.
fn frame_single(version: &str) -> String {
    let v = version.trim_start_matches('v');
    format!("✨ What's new in v{v}\n\n")
}

/// Aggregate frame line — `✨ What's new in v<from> → v<to>` followed
/// by a blank line. Explicitly shows the range so the operator knows
/// what they're reading.
fn frame_range(from: &str, to: &str) -> String {
    let from = from.trim_start_matches('v');
    let to = to.trim_start_matches('v');
    format!("✨ What's new in v{from} → v{to}\n\n")
}

/// Footer line printed at the bottom of every "what's new" output —
/// links to the full curated changelog.
fn footer_line() -> String {
    format!("📖 Full changelog: {CHANGELOG_URL}")
}

/// Compose the framed single-version "what's new" terminal block.
/// Non-conforming body (no `# ` headings, or contains `##`/code-fence)
/// → raw body after the frame, no styling. Footer link at the bottom.
/// Empty body (or body truncated to empty by cargo-dist stripping)
/// elides the body+blank-line so the output is `frame + footer` with
/// exactly one blank line between, not two.
pub fn render(version: &str, body: &str) -> String {
    let mut out = frame_single(version);
    let body_rendered = render_body(body);
    if !body_rendered.is_empty() {
        out.push_str(&body_rendered);
        out.push('\n');
    }
    out.push_str(&footer_line());
    out.push('\n');
    out
}

/// Compose the aggregate "what's new" block — range frame, then each
/// release as a `v<X.Y.Z>` subheader followed by its body, footer at
/// the bottom. Entries should be ordered oldest-first. Empty per-entry
/// bodies (post-truncation) elide their blank-line so we don't ship
/// double-blank gaps between subheaders.
pub(crate) fn render_range(from: &str, to: &str, entries: &[ReleaseEntry]) -> String {
    let mut out = frame_range(from, to);
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("v{}\n", e.version.trim_start_matches('v')));
        let body_rendered = render_body(&e.body);
        if !body_rendered.is_empty() {
            out.push_str(&body_rendered);
        }
    }
    out.push('\n');
    out.push_str(&footer_line());
    out.push('\n');
    out
}

/// Single-line fallback when the release-body fetch fails (network,
/// `gh` missing, 404, malformed JSON). Always exit 0; never raise.
pub fn fallback_link(version: &str) -> String {
    let v = version.trim_start_matches('v');
    format!(
        "(release notes unavailable — check https://github.com/Alireza29675/teamctl/releases/tag/v{v})"
    )
}

/// Best-effort single-version print. Used by the `whatsnew` subcommand
/// when no `--since` is given. Fetch failure → fallback link, exit 0.
/// Callers that want a leading blank line (post-install visual gap)
/// should emit it themselves; this function prints the block as-is.
pub fn print_for(version: &str) {
    match fetch_release_body(version) {
        Ok(body) => print!("{}", render(version, &body)),
        Err(_) => println!("{}", fallback_link(version)),
    }
}

/// Effective FROM for the displayed frame. Per owner: the range is
/// `max(current, FLOOR_VERSION) to updated`. Selection logic already
/// clamps via `is_below_floor`; this is just the visual side so a
/// pre-floor `current` doesn't leak into the header.
fn effective_from(current: &str) -> String {
    if is_below_floor(current) {
        FLOOR_VERSION.to_string()
    } else {
        current.trim_start_matches('v').to_string()
    }
}

/// Best-effort aggregate print of every release body in `(from, to]`
/// that sits at or above the floor. Used by `teamctl update`'s
/// post-success display and `whatsnew --since X`. Fetch failure on
/// the list endpoint → falls back to single-version print of `to`.
/// Empty range (no intermediates above floor, or only `to` matters)
/// → single-version print of `to` so the user always sees the target.
pub fn print_since(from: &str, to: &str) {
    let display_from = effective_from(from);
    let to_v = to.trim_start_matches('v');
    // Degenerate range — effective from equals to (e.g. `whatsnew
    // --since 0.7.3` from a v0.8.0 binary clamps to 0.8.0 → 0.8.0).
    // Drop to the single-version frame so the operator doesn't see
    // "v0.8.0 → v0.8.0" in the header.
    if display_from == to_v {
        print_target_inline(to);
        return;
    }
    let entries = match fetch_releases_list() {
        Ok(all) => select_range(&all, from, to),
        Err(_) => {
            // The list endpoint failed; fall through to fetching just
            // the target version's body. Preserves the "always show
            // the target" contract.
            print_target_inline(to);
            return;
        }
    };
    if entries.is_empty() {
        // No intermediates landed in (from, to] above floor — fall
        // back to single-version display of the target.
        print_target_inline(to);
        return;
    }
    if entries.len() == 1 {
        // A 1-element range repeats the same version across three
        // layers (range frame, per-version subheader, body's opening
        // line). Collapse to the single-version frame instead. See
        // #198. Genuine multi-version ranges still use render_range.
        print_target_inline(&entries[0].version);
        return;
    }
    print!("{}", render_range(&display_from, to, &entries));
}

/// Helper used by `print_since` when the list-endpoint path can't
/// produce a range — fetches and prints just the target's body, or
/// the quiet fallback line if even that fails.
fn print_target_inline(version: &str) {
    match fetch_release_body(version) {
        Ok(body) => print!("{}", render(version, &body)),
        Err(_) => println!("{}", fallback_link(version)),
    }
}

fn curl_get(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args([
            "-sS",
            "-H",
            &format!("User-Agent: teamctl-cli/{CURRENT_VERSION}"),
            "--max-time",
            "15",
            url,
        ])
        .output()
        .context("run curl (is curl installed?)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("curl failed: {}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_body_handles_single_line_json() {
        let blob = r#"{"id":1,"body":"hello world","name":"v0.8.0"}"#;
        assert_eq!(extract_body_field(blob).as_deref(), Some("hello world"));
    }

    #[test]
    fn extract_body_unescapes_newlines_and_quotes() {
        let blob = r#"{"body":"line1\nline2 \"quoted\" \\path"}"#;
        assert_eq!(
            extract_body_field(blob).as_deref(),
            Some("line1\nline2 \"quoted\" \\path")
        );
    }

    #[test]
    fn extract_body_returns_none_for_null_body() {
        let blob = r#"{"body":null,"tag_name":"v0.8.0"}"#;
        assert!(extract_body_field(blob).is_none());
    }

    #[test]
    fn extract_body_returns_none_for_empty_body() {
        let blob = r#"{"body":""}"#;
        assert!(extract_body_field(blob).is_none());
    }

    #[test]
    fn extract_body_returns_none_for_missing_field() {
        let blob = r#"{"tag_name":"v0.8.0"}"#;
        assert!(extract_body_field(blob).is_none());
    }

    #[test]
    fn extract_body_handles_unicode_escape() {
        let blob = r#"{"body":"✨ sparkle"}"#;
        assert_eq!(extract_body_field(blob).as_deref(), Some("✨ sparkle"));
    }

    #[test]
    fn extract_body_survives_round_trip_with_realistic_release_body() {
        // T-180: pin the serde_json swap against a release-body shape
        // that mixes the escapes the old hand-rolled parser had to
        // special-case: nested quotes around CLI vocabulary, escaped
        // backslashes inside backtick-spans, unicode-escaped accent,
        // and an emoji-prefixed opening line. If serde drifts or the
        // struct stops covering all three, this fails loud.
        let blob = r#"{"tag_name":"v0.8.0","body":"✨ \"What's new\" cleanup\n\nSwap `extract_body_field` to serde_json — fixes \"escaped quote\" handling and unicode (éclat) survives round-trip. Path: C:\\Users\\test.","name":"0.8.0"}"#;
        let body = extract_body_field(blob).expect("body present");
        assert!(body.contains("\"What's new\""));
        assert!(body.contains("`extract_body_field`"));
        assert!(body.contains("éclat"));
        assert!(body.contains(r"C:\Users\test"));
        assert!(body.starts_with("✨"));
    }

    #[test]
    fn extract_body_returns_none_for_malformed_json() {
        // Old hand-rolled parser silently returned None on shapes it
        // couldn't walk; serde behaves the same via `.ok()?`. Pin it
        // explicitly so a future change can't accidentally raise.
        let blob = r#"{"body": "unterminated"#;
        assert!(extract_body_field(blob).is_none());
    }

    #[test]
    fn parse_release_body_handles_single_entry() {
        let body = "# First headline\nA description line.\nAnother line.\n";
        let parsed = parse_release_body(body);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].headline, "First headline");
        assert_eq!(parsed[0].description, "A description line.\nAnother line.");
    }

    #[test]
    fn parse_release_body_handles_multiple_entries() {
        let body = "# Headline A\nDesc A.\n\n# Headline B\nDesc B line 1.\nDesc B line 2.\n";
        let parsed = parse_release_body(body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].headline, "Headline A");
        assert_eq!(parsed[0].description, "Desc A.");
        assert_eq!(parsed[1].headline, "Headline B");
        assert_eq!(parsed[1].description, "Desc B line 1.\nDesc B line 2.");
    }

    #[test]
    fn parse_release_body_returns_empty_when_no_headings() {
        let body = "just a paragraph of release notes\nwith two lines";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_treats_h2_as_non_convention() {
        let body = "# Headline\nIntro line.\n## Subsection\nMore description.\n";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_treats_code_fence_as_non_convention() {
        let body = "# Headline\nDescription with snippet:\n```sh\nteamctl up\n```\n";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_treats_cargo_dist_body_as_non_convention() {
        let body = "# team-bot 0.7.3\n## Install team-bot 0.7.3\n### shell\n```sh\ncurl ...\n```\n";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_handles_headline_without_description() {
        let body = "# Only a headline\n\n# Another\nWith body.\n";
        let parsed = parse_release_body(body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].headline, "Only a headline");
        assert!(parsed[0].description.is_empty());
        assert_eq!(parsed[1].headline, "Another");
    }

    #[test]
    fn render_frames_version_and_styles_descriptions() {
        let body = "# Headline\nLine one.\nLine two.\n";
        let rendered = render("0.8.0", body);
        assert!(rendered.starts_with("✨ What's new in v0.8.0\n\n"));
        assert!(rendered.contains("Headline\n"));
        assert!(rendered.contains("  \x1b[2;3mLine one.\x1b[0m"));
        assert!(rendered.contains("  \x1b[2;3mLine two.\x1b[0m"));
    }

    #[test]
    fn render_strips_v_prefix_in_header() {
        let body = "# H\nD.\n";
        let rendered = render("v0.8.0", body);
        assert!(rendered.starts_with("✨ What's new in v0.8.0\n\n"));
        assert!(!rendered.contains("vv0.8.0"));
    }

    #[test]
    fn render_falls_back_to_raw_for_non_conforming_body() {
        let body = "Older release with no structured convention.\nJust prose.\n";
        let rendered = render("0.6.0", body);
        assert!(rendered.starts_with("✨ What's new in v0.6.0\n\n"));
        assert!(rendered.contains("Older release with no structured convention."));
        assert!(rendered.contains("Just prose."));
        // No ANSI escapes on the raw fallback path.
        let body_only = rendered.replace("📖 Full changelog: https://teamctl.run/changelog\n", "");
        assert!(!body_only.contains("\x1b["));
    }

    #[test]
    fn render_separates_entries_with_blank_line() {
        let body = "# A\nDesc A.\n\n# B\nDesc B.\n";
        let rendered = render("0.8.0", body);
        let a_idx = rendered.find("Desc A.").unwrap();
        let b_idx = rendered.find("B\n").unwrap();
        let between = &rendered[a_idx..b_idx];
        let nl_count = between.matches('\n').count();
        assert!(
            nl_count >= 2,
            "expected blank line between entries, got: {between:?}"
        );
    }

    #[test]
    fn render_handles_empty_body() {
        let rendered = render("0.8.0", "");
        assert!(rendered.starts_with("✨ What's new in v0.8.0\n\n"));
        assert!(rendered.contains("📖 Full changelog"));
    }

    #[test]
    fn render_empty_body_has_no_double_blank_before_footer() {
        // #201: empty body must produce `frame + blank + footer`,
        // never `frame + blank + blank + footer`.
        let rendered = render("0.8.0", "");
        assert_eq!(
            rendered,
            "✨ What's new in v0.8.0\n\n📖 Full changelog: https://teamctl.run/changelog\n",
            "expected exactly one blank line between frame and footer, got: {rendered:?}"
        );
    }

    #[test]
    fn render_install_at_top_truncated_body_has_no_double_blank() {
        // #201 / #197 interaction: when truncation drops a
        // cargo-dist-only body down to empty, render() must not emit
        // a stray extra blank line.
        let body = "# teamctl 0.8.1\n## Install\n```sh\nnoise\n```\n";
        let rendered = render("0.8.1", body);
        assert_eq!(
            rendered,
            "✨ What's new in v0.8.1\n\n📖 Full changelog: https://teamctl.run/changelog\n",
            "truncated-to-empty body must not double-blank, got: {rendered:?}"
        );
    }

    #[test]
    fn render_range_empty_entry_body_has_no_double_blank_between_subheaders() {
        // #201 in aggregate path: an entry whose body truncates to
        // empty should NOT leave a stray blank line under its
        // subheader. Two entries, second has empty body — gap between
        // subheaders should be exactly one blank line.
        let entries = vec![
            ReleaseEntry {
                version: "0.8.1".into(),
                body: "# A\nDesc A.".into(),
            },
            ReleaseEntry {
                version: "0.8.2".into(),
                body: "# teamctl 0.8.2\n## Install\n".into(),
            },
        ];
        let rendered = render_range("0.8.0", "0.8.2", &entries);
        // Between the styled description of entry-1 and the v0.8.2
        // subheader, there must be exactly one blank line — i.e. the
        // pattern `\n\nv0.8.2\n`, and no `\n\n\nv0.8.2\n`.
        assert!(
            rendered.contains("\n\nv0.8.2\n"),
            "expected single blank line before v0.8.2 subheader, got: {rendered:?}"
        );
        assert!(
            !rendered.contains("\n\n\nv0.8.2\n"),
            "expected no double blank line before v0.8.2 subheader, got: {rendered:?}"
        );
    }

    #[test]
    fn render_appends_footer_line() {
        let rendered = render("0.8.0", "# H\nD.\n");
        assert!(rendered.contains("📖 Full changelog: https://teamctl.run/changelog"));
        assert!(
            rendered.ends_with("https://teamctl.run/changelog\n"),
            "footer should be the last line: {rendered:?}"
        );
    }

    #[test]
    fn fallback_link_contains_tag_url() {
        let line = fallback_link("0.8.0");
        assert!(line.contains("https://github.com/Alireza29675/teamctl/releases/tag/v0.8.0"));
        assert!(line.contains("release notes unavailable"));
    }

    #[test]
    fn fallback_link_strips_v_prefix() {
        assert_eq!(fallback_link("v0.8.0"), fallback_link("0.8.0"));
    }

    // ── Range / aggregate display ────────────────────────────────────

    #[test]
    fn is_below_floor_is_inclusive_at_floor() {
        // The floor IS the first supported version, not below it.
        assert!(!is_below_floor("0.8.0"));
        assert!(!is_below_floor("v0.8.0"));
        assert!(!is_below_floor("0.8.1"));
        assert!(!is_below_floor("0.9.0"));
        assert!(!is_below_floor("1.0.0"));
    }

    #[test]
    fn is_below_floor_excludes_pre_floor_versions() {
        assert!(is_below_floor("0.7.3"));
        assert!(is_below_floor("0.7.0"));
        assert!(is_below_floor("0.6.0"));
        assert!(is_below_floor("v0.7.3"));
    }

    #[test]
    fn parse_releases_list_extracts_tag_and_body() {
        // r##""## escape needed because the JSON content contains `"#`
        // (body field starts with `"# `), which would prematurely close
        // a single-hash raw string.
        let json = r##"[
            {"tag_name":"v0.8.2","body":"# H2\nBody2","name":"v0.8.2"},
            {"tag_name":"v0.8.1","body":"# H1\nBody1"},
            {"tag_name":"v0.8.0","body":"# H0\nBody0"}
        ]"##;
        let list = parse_releases_list(json);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].version, "0.8.2");
        assert_eq!(list[0].body, "# H2\nBody2");
        assert_eq!(list[2].version, "0.8.0");
    }

    #[test]
    fn parse_releases_list_returns_empty_on_malformed_json() {
        assert!(parse_releases_list("{not json").is_empty());
        assert!(parse_releases_list(r#"{"message":"Not Found"}"#).is_empty());
    }

    #[test]
    fn parse_releases_list_skips_items_missing_tag() {
        let json = r#"[
            {"tag_name":"v0.8.0","body":"keep"},
            {"body":"skip me — no tag"},
            {"tag_name":"","body":"skip — empty tag"}
        ]"#;
        let list = parse_releases_list(json);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "0.8.0");
    }

    #[test]
    fn parse_releases_list_tolerates_null_body() {
        let json = r#"[{"tag_name":"v0.8.0","body":null}]"#;
        let list = parse_releases_list(json);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].body, "");
    }

    fn sample_releases() -> Vec<ReleaseEntry> {
        // GitHub returns newest-first; mirror that here.
        vec![
            ReleaseEntry {
                version: "0.8.3".into(),
                body: "# T3\nD3".into(),
            },
            ReleaseEntry {
                version: "0.8.2".into(),
                body: "# T2\nD2".into(),
            },
            ReleaseEntry {
                version: "0.8.1".into(),
                body: "# T1\nD1".into(),
            },
            ReleaseEntry {
                version: "0.8.0".into(),
                body: "# T0\nD0".into(),
            },
            ReleaseEntry {
                version: "0.7.3".into(),
                body: "old".into(),
            },
        ]
    }

    #[test]
    fn select_range_excludes_from_and_includes_to() {
        let r = select_range(&sample_releases(), "0.8.0", "0.8.3");
        let vs: Vec<&str> = r.iter().map(|e| e.version.as_str()).collect();
        assert_eq!(vs, vec!["0.8.1", "0.8.2", "0.8.3"]);
    }

    #[test]
    fn select_range_cuts_below_floor() {
        // from below floor; floor + above should still surface.
        let r = select_range(&sample_releases(), "0.7.0", "0.8.1");
        let vs: Vec<&str> = r.iter().map(|e| e.version.as_str()).collect();
        // 0.7.3 excluded by floor; 0.8.0 + 0.8.1 included.
        assert_eq!(vs, vec!["0.8.0", "0.8.1"]);
    }

    #[test]
    fn select_range_returns_oldest_first() {
        let r = select_range(&sample_releases(), "0.7.0", "0.8.3");
        assert_eq!(r.first().unwrap().version, "0.8.0");
        assert_eq!(r.last().unwrap().version, "0.8.3");
    }

    #[test]
    fn select_range_is_empty_when_from_equals_to() {
        // (X, X] is empty; the caller falls back to single-version.
        let r = select_range(&sample_releases(), "0.8.2", "0.8.2");
        assert!(r.is_empty());
    }

    #[test]
    fn select_range_excludes_targets_below_floor() {
        // Both endpoints pre-floor; the result should be empty.
        let r = select_range(&sample_releases(), "0.7.0", "0.7.3");
        assert!(r.is_empty());
    }

    #[test]
    fn render_range_shows_range_frame_and_per_version_subheaders() {
        let entries = vec![
            ReleaseEntry {
                version: "0.8.1".into(),
                body: "# H1\nD1.".into(),
            },
            ReleaseEntry {
                version: "0.8.2".into(),
                body: "# H2\nD2.".into(),
            },
        ];
        let rendered = render_range("0.8.0", "0.8.2", &entries);
        assert!(rendered.starts_with("✨ What's new in v0.8.0 → v0.8.2\n\n"));
        assert!(rendered.contains("v0.8.1\nH1\n"));
        assert!(rendered.contains("v0.8.2\nH2\n"));
        assert!(rendered.ends_with("https://teamctl.run/changelog\n"));
    }

    #[test]
    fn render_range_strips_v_prefix_consistently() {
        let entries = vec![ReleaseEntry {
            version: "v0.8.1".into(),
            body: "# H\nD.".into(),
        }];
        let rendered = render_range("v0.8.0", "v0.8.1", &entries);
        assert!(rendered.starts_with("✨ What's new in v0.8.0 → v0.8.1\n\n"));
        // No double-v leaking through anywhere.
        assert!(!rendered.contains("vv"));
    }

    #[test]
    fn render_range_separates_versions_with_blank_line() {
        let entries = vec![
            ReleaseEntry {
                version: "0.8.1".into(),
                body: "# A\nDesc A.".into(),
            },
            ReleaseEntry {
                version: "0.8.3".into(),
                body: "# B\nDesc B.".into(),
            },
        ];
        // Frame uses v0.8.0 → v0.8.4 so the frame doesn't end in a
        // string that collides with our subheader search patterns.
        let rendered = render_range("0.8.0", "0.8.4", &entries);
        assert!(
            rendered.contains("\n\nv0.8.3\n"),
            "expected blank line before v0.8.3 subheader, got: {rendered:?}"
        );
    }

    #[test]
    fn footer_line_points_to_changelog() {
        assert_eq!(
            footer_line(),
            "📖 Full changelog: https://teamctl.run/changelog"
        );
    }

    #[test]
    fn effective_from_clamps_pre_floor_current_to_floor() {
        // Hypothetical 0.7.x backport: the running binary's version is
        // pre-floor, so the displayed frame's FROM clamps up to 0.8.0
        // rather than leaking "v0.7.3" into the header.
        assert_eq!(effective_from("0.7.3"), "0.8.0");
        assert_eq!(effective_from("v0.6.0"), "0.8.0");
    }

    #[test]
    fn effective_from_preserves_at_or_above_floor() {
        assert_eq!(effective_from("0.8.0"), "0.8.0");
        assert_eq!(effective_from("0.8.5"), "0.8.5");
        assert_eq!(effective_from("v0.9.0"), "0.9.0");
        assert_eq!(effective_from("1.0.0"), "1.0.0");
    }

    // ── #197: cargo-dist install-heading truncation ─────────────────

    #[test]
    fn is_cargo_dist_install_heading_recognizes_known_crates() {
        assert!(is_cargo_dist_install_heading("# team-bot 0.8.1"));
        assert!(is_cargo_dist_install_heading("# team-mcp 0.8.1"));
        assert!(is_cargo_dist_install_heading("# teamctl 0.8.1"));
        assert!(is_cargo_dist_install_heading("# teamctl-ui 0.8.1"));
        assert!(is_cargo_dist_install_heading("# teamctl 10.20.30"));
    }

    #[test]
    fn is_cargo_dist_install_heading_rejects_other_shapes() {
        assert!(!is_cargo_dist_install_heading("# Headline"));
        assert!(!is_cargo_dist_install_heading("## team-bot 0.8.1"));
        assert!(!is_cargo_dist_install_heading("team-bot 0.8.1"));
        assert!(!is_cargo_dist_install_heading("# unknown-crate 0.8.1"));
        assert!(!is_cargo_dist_install_heading("# teamctl 0.8"));
        assert!(!is_cargo_dist_install_heading("# teamctl 0.8.1-rc.1"));
        assert!(!is_cargo_dist_install_heading("# teamctl v0.8.1"));
        assert!(!is_cargo_dist_install_heading(""));
    }

    #[test]
    fn truncate_at_cargo_dist_drops_install_section_and_below() {
        let body = "Curated prose paragraph one.\n\n\
            Curated prose paragraph two.\n\n\
            # team-bot 0.8.1\n## Install\n```sh\ncurl ...\n```\n";
        let kept = truncate_at_cargo_dist(body);
        assert!(kept.starts_with("Curated prose paragraph one."));
        assert!(kept.contains("paragraph two."));
        assert!(!kept.contains("team-bot 0.8.1"));
        assert!(!kept.contains("Install"));
        assert!(!kept.contains("```"));
    }

    #[test]
    fn truncate_at_cargo_dist_unchanged_when_no_install_heading() {
        let body = "# Curated headline\nDescription with no install boilerplate.\n";
        assert_eq!(truncate_at_cargo_dist(body), body);
    }

    #[test]
    fn truncate_at_cargo_dist_empty_result_when_install_starts_at_top() {
        // Cargo-dist body with no prose above the first crate heading —
        // truncation yields an empty slice. Render's empty-body branch
        // handles this without panicking.
        let body = "# teamctl 0.8.1\n## Install\n";
        assert_eq!(truncate_at_cargo_dist(body), "");
    }

    #[test]
    fn truncate_at_cargo_dist_keeps_prose_with_inline_hash() {
        // An inline `# 1` or `## ` mid-paragraph is not a heading
        // because the heading detector requires the line to start with
        // `# <crate> <semver>`. Make sure prose containing `#1` style
        // strings survives.
        let body = "Fixed issue #197 by truncating before cargo-dist.\n";
        assert_eq!(truncate_at_cargo_dist(body), body);
    }

    #[test]
    fn render_body_strips_cargo_dist_install_section() {
        let body = "# Curated headline\nDescription.\n\n# teamctl 0.8.1\n```sh\nnoise\n```\n";
        let rendered = render_body(body);
        // The curated entry should render with style; the cargo-dist
        // tail must be gone.
        assert!(rendered.contains("Curated headline\n"));
        assert!(rendered.contains("Description."));
        assert!(!rendered.contains("teamctl 0.8.1"));
        assert!(!rendered.contains("```"));
    }

    // ── #198: 1-element range collapses to single-version frame ─────

    #[test]
    fn render_range_path_used_for_multi_version() {
        // Sanity check the path still exists for genuine ranges.
        let entries = vec![
            ReleaseEntry {
                version: "0.8.1".into(),
                body: "# A\nDesc A.".into(),
            },
            ReleaseEntry {
                version: "0.8.2".into(),
                body: "# B\nDesc B.".into(),
            },
        ];
        let rendered = render_range("0.8.0", "0.8.2", &entries);
        assert!(rendered.starts_with("✨ What's new in v0.8.0 → v0.8.2\n\n"));
        assert!(rendered.contains("\nv0.8.1\n"));
        assert!(rendered.contains("\n\nv0.8.2\n"));
    }
}
