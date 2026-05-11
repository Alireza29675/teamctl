//! Release-body fetch, parse, and render for `teamctl update`'s
//! post-success "what's new" display and the `teamctl whatsnew`
//! subcommand. The GitHub release body is the source of truth for the
//! user-facing voice piece; this module turns its markdown into a
//! framed terminal block.
//!
//! Convention parsed: a markdown `# ` heading is the entry headline;
//! the paragraph underneath is the description. Multiple entries per
//! release. Anything that doesn't match falls through to raw body
//! display — never an error.

use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::cmd::update::CURRENT_VERSION;

const RELEASES_TAG_API: &str = "https://api.github.com/repos/Alireza29675/teamctl/releases/tags/";

/// ANSI escape opening italic + dim. Pairs with [`STYLE_RESET`].
const STYLE_DIM_ITALIC: &str = "\x1b[2;3m";
const STYLE_RESET: &str = "\x1b[0m";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub headline: String,
    pub description: String,
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

/// Compose the framed "what's new" terminal block for a version and a
/// raw release body. Non-conforming body (no `# ` headings) → raw body
/// after the frame line, no styling. Each entry: headline at normal
/// weight, description lines indented two spaces and rendered in
/// italic + dim. Blank line between entries.
pub fn render(version: &str, body: &str) -> String {
    let v = version.trim_start_matches('v');
    let entries = parse_release_body(body);
    let mut out = String::new();
    out.push_str(&format!("✨ What's new in v{v}\n\n"));
    if entries.is_empty() {
        let trimmed = body.trim_end();
        if !trimmed.is_empty() {
            out.push_str(trimmed);
            out.push('\n');
        }
        return out;
    }
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

/// Single-line fallback when the release-body fetch fails (network,
/// `gh` missing, 404, malformed JSON). Always exit 0; never raise.
pub fn fallback_link(version: &str) -> String {
    let v = version.trim_start_matches('v');
    format!(
        "(release notes unavailable — check https://github.com/Alireza29675/teamctl/releases/tag/v{v})"
    )
}

/// Best-effort "print release notes after a successful update". Used
/// in [`crate::cmd::update`]'s Older arm after the binary install + the
/// claude-plugin sync. Never raises — fetch failure prints the
/// fallback link and returns.
pub fn print_for(version: &str) {
    println!();
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
        // `✨` is ✨ — exactly the kind of character a release body
        // may contain when JSON-encoded with ASCII-safe escaping.
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
        // The curated convention is `# Headline\nDescription` with no
        // sub-headings. A `## ` line means the body is richer markdown
        // (most commonly cargo-dist's auto-generated body), so the
        // parser short-circuits to empty and the renderer raw-falls
        // back. Keeps users from seeing `## Install` styled as a
        // dim-italic description by accident.
        let body = "# Headline\nIntro line.\n## Subsection\nMore description.\n";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_treats_code_fence_as_non_convention() {
        // Fenced code blocks signal install-snippet / changelog-rich
        // bodies. Same raw-fallback treatment as `## ` sub-headings.
        let body = "# Headline\nDescription with snippet:\n```sh\nteamctl up\n```\n";
        assert!(parse_release_body(body).is_empty());
    }

    #[test]
    fn parse_release_body_treats_cargo_dist_body_as_non_convention() {
        // Smoke-test against the actual shape cargo-dist produces.
        // `# team-bot 0.7.3` looks like a headline, but the body has
        // both `## Install...` and fenced code blocks — neither match
        // the curated convention.
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
        // Each description line is two-space indented and italic+dim wrapped.
        assert!(rendered.contains("  \x1b[2;3mLine one.\x1b[0m"));
        assert!(rendered.contains("  \x1b[2;3mLine two.\x1b[0m"));
    }

    #[test]
    fn render_strips_v_prefix_in_header() {
        let body = "# H\nD.\n";
        let rendered = render("v0.8.0", body);
        assert!(rendered.starts_with("✨ What's new in v0.8.0\n\n"));
        // No double-v.
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
        assert!(!rendered.contains("\x1b["));
    }

    #[test]
    fn render_separates_entries_with_blank_line() {
        let body = "# A\nDesc A.\n\n# B\nDesc B.\n";
        let rendered = render("0.8.0", body);
        // Between the description of A and the headline of B there
        // should be exactly one blank line.
        let a_idx = rendered.find("Desc A.").unwrap();
        let b_idx = rendered.find("B\n").unwrap();
        let between = &rendered[a_idx..b_idx];
        // Count newlines between the end of A's description line and
        // the start of B's headline line.
        let nl_count = between.matches('\n').count();
        assert!(
            nl_count >= 2,
            "expected blank line between entries, got: {between:?}"
        );
    }

    #[test]
    fn render_handles_empty_body() {
        let rendered = render("0.8.0", "");
        assert_eq!(rendered, "✨ What's new in v0.8.0\n\n");
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
}
