//! Comment-preserving YAML edit substrate.
//!
//! Wraps [`yaml_edit`] (rowan-backed lossless syntax tree) so callers that
//! mutate `.team/*.yaml` keep the user's comments, blank-line clusters, and
//! key ordering intact across save. The previous `serde_yaml::Value`
//! round-trip stripped all of that on every write — see the dogfood
//! `.team/projects/teamctl.yaml` regressions on the PR #54 + PR #55
//! cascades for the class this closes.
//!
//! ## Surface
//!
//! - [`load`] / [`save`] — IO with `anyhow` context.
//! - Re-exports of [`yaml_edit::Document`], [`yaml_edit::Mapping`],
//!   [`yaml_edit::Sequence`], and [`YamlPath`] so callers can drive the
//!   editor directly for round-trip + leaf updates.
//! - [`set_nested_mapping`] — bounded line-anchored helper for the one
//!   pattern yaml-edit 0.2.x can't do natively: insert or replace a
//!   properly-indented sub-block at a known parent path.
//!
//! ## Why the bounded helper
//!
//! `yaml_edit::Document::set_path` creates intermediate mappings via
//! `MappingBuilder::new().build_document().as_mapping()` and inserts them
//! with `mapping.set(key, &empty_mapping)`. The empty mapping has zero
//! base-indent, and the resulting nested entries serialize at column 0
//! instead of indenting under the parent (see `path::set_path_on_mapping`,
//! registry source line 401-435 of yaml-edit 0.2.1). Filed upstream for
//! a fix; until then [`set_nested_mapping`] handles the create-nested
//! pattern via line-anchored splice into the source string before the
//! Document re-parse. Substrate consumers never see the splice.
//!
//! Per-pm scope lock (msg 1969): the helper handles ONE pattern only
//! ("insert a properly-indented sub-block at a known parent path"). If a
//! future T-077-E verb needs a different yaml-edit-gap workaround,
//! escalate; do NOT generalize this helper.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

pub use yaml_edit::path::YamlPath;
pub use yaml_edit::{Document, Mapping, Sequence};

/// Read `path` and parse it as an editable YAML document.
///
/// The returned [`Document`] retains the source's comments, blank-line
/// clusters, and key ordering; mutations applied to it preserve everything
/// outside the touched range.
pub fn load(path: &Path) -> Result<Document> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    raw.parse::<Document>()
        .with_context(|| format!("parse {}", path.display()))
}

/// Serialize `doc` and write it to `path`, replacing any previous contents.
///
/// `Document`'s `Display` impl emits the underlying syntax tree verbatim,
/// so untouched regions round-trip byte-for-byte (modulo the upstream
/// pre-document-trivia limitation noted in the module docs).
pub fn save(doc: &Document, path: &Path) -> Result<()> {
    fs::write(path, doc.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Insert or replace a nested mapping at the given parent path.
///
/// `parent_path` is a sequence of mapping keys descending from the root.
/// All but the last key must resolve to a mapping that the helper can
/// either find in the source or create alongside its existing siblings.
/// The last key (the leaf) is the mapping the caller wants to upsert.
/// `value_pairs` becomes the body of that leaf mapping.
///
/// Existing siblings of the leaf — and existing siblings of any
/// intermediate the helper has to create — are preserved with their
/// comments and ordering intact. If the leaf mapping already exists,
/// it is replaced wholesale by `value_pairs`. Other adapters under the
/// same parent (e.g. `discord:` next to `telegram:`) survive.
///
/// # Errors
/// Returns an error if `parent_path` is empty or if the first key is
/// not a top-level mapping in the document.
pub fn set_nested_mapping(
    doc: Document,
    parent_path: &[&str],
    value_pairs: &[(&str, &str)],
) -> Result<Document> {
    if parent_path.is_empty() {
        return Err(anyhow!("set_nested_mapping: parent_path must not be empty"));
    }
    let source = doc.to_string();
    let edited = splice_nested_mapping(&source, parent_path, value_pairs)?;
    edited
        .parse::<Document>()
        .with_context(|| "re-parse spliced YAML")
}

/// Line-anchored splice: walk `source` to find the deepest existing
/// ancestor of `path`, then insert (or replace) the missing tail and the
/// leaf body at the right indent.
fn splice_nested_mapping(
    source: &str,
    path: &[&str],
    value_pairs: &[(&str, &str)],
) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let trailing_newline = source.ends_with('\n');

    // Walk the path top-down, tracking the (line, indent) of each existing
    // ancestor. Stop at the first missing component.
    let mut current_indent: usize = 0;
    let mut search_start: usize = 0;
    let mut search_end: usize = lines.len();
    let mut existing_depth: usize = 0;
    let mut leaf_replace_range: Option<(usize, usize, usize)> = None; // (start_line, end_line_exclusive, leaf_indent)

    for (depth, key) in path.iter().enumerate() {
        let parent_indent = current_indent;
        let child_indent_min = if depth == 0 { 0 } else { parent_indent + 1 };
        match find_key_in_block(&lines, search_start, search_end, key, child_indent_min) {
            Some((line_idx, key_indent)) => {
                existing_depth = depth + 1;
                current_indent = key_indent;
                let block_end = block_end_after(&lines, line_idx, key_indent);
                if depth == path.len() - 1 {
                    leaf_replace_range = Some((line_idx, block_end, key_indent));
                } else {
                    search_start = line_idx + 1;
                    search_end = block_end;
                }
            }
            None => break,
        }
    }

    if existing_depth == 0 {
        return Err(anyhow!(
            "set_nested_mapping: top-level key `{}` not found",
            path[0]
        ));
    }

    // Build the replacement / insertion block.
    let insert_indent = if existing_depth == path.len() {
        // Leaf already exists; reuse its indent.
        leaf_replace_range.expect("leaf existed").2
    } else {
        // First missing component lands one level deeper than its parent.
        current_indent + 2
    };

    let missing_tail = &path[existing_depth..];
    let mut block_lines: Vec<String> = Vec::new();
    let mut indent = insert_indent;
    for key in missing_tail {
        block_lines.push(format!("{:indent$}{key}:", "", indent = indent, key = key));
        indent += 2;
    }
    // Leaf indent: if the leaf was found, missing_tail is empty and `indent`
    // == insert_indent. Otherwise indent has already advanced past the last
    // missing key. In both cases the value pairs sit at `indent`.
    let value_indent = indent;
    if existing_depth == path.len() {
        // We're replacing an existing leaf — emit the leaf key line too.
        block_lines.push(format!(
            "{:indent$}{key}:",
            "",
            indent = insert_indent,
            key = path[path.len() - 1]
        ));
    }
    for (k, v) in value_pairs {
        block_lines.push(format!(
            "{:indent$}{k}: {v}",
            "",
            indent = value_indent,
            k = k,
            v = v
        ));
    }

    // Splice: replace the leaf-block range or insert at the parent's
    // block end.
    let mut out_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    if let Some((start, end, _)) = leaf_replace_range {
        out_lines.splice(start..end, block_lines);
    } else {
        // Insert at the end of the deepest-existing-ancestor's block.
        // search_end is that block's end (exclusive); insert there.
        out_lines.splice(search_end..search_end, block_lines);
    }

    let mut joined = out_lines.join("\n");
    if trailing_newline && !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// Within `lines[start..end]`, find a mapping key whose indent is `>=
/// min_indent` AND is the direct-child indent of its parent (i.e. the
/// minimum indent appearing in this slice that is `>= min_indent`).
/// Returns `(line_idx, indent)`.
fn find_key_in_block(
    lines: &[&str],
    start: usize,
    end: usize,
    key: &str,
    min_indent: usize,
) -> Option<(usize, usize)> {
    // First pass: find the smallest indent in this slice that's >= min_indent
    // and belongs to a mapping key line (`<indent>foo:` with foo non-empty).
    // That defines "direct children" of the parent.
    let mut child_indent: Option<usize> = None;
    for line in lines.iter().take(end).skip(start) {
        if let Some((indent, _)) = parse_mapping_key_line(line) {
            if indent >= min_indent {
                child_indent = Some(child_indent.map_or(indent, |c| c.min(indent)));
            }
        }
    }
    let child_indent = child_indent?;

    // Second pass: find the named key at exactly child_indent.
    for (i, line) in lines.iter().enumerate().take(end).skip(start) {
        if let Some((indent, found_key)) = parse_mapping_key_line(line) {
            if indent == child_indent && found_key == key {
                return Some((i, indent));
            }
        }
    }
    None
}

/// Returns `(indent, key)` if `line` is a `<indent>key:` mapping entry —
/// i.e. starts with spaces, has a non-empty unquoted-non-list key, and
/// ends `:` (possibly followed by whitespace + an inline value).
///
/// Conservative: this helper does NOT recognise quoted keys, flow
/// mappings, or anchors. The verbs T-077-E targets stick to the canonical
/// block-style YAML in `examples/*/.team/`, which uses none of those.
/// If a future verb needs broader coverage, escalate per the pm-locked
/// scope rule in the module docs — do not silently extend here.
fn parse_mapping_key_line(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
        return None;
    }
    let colon_idx = trimmed.find(':')?;
    let key = &trimmed[..colon_idx];
    if key.is_empty() {
        return None;
    }
    // Reject lines like "key: value: tail" — only recognise where the key
    // contains no colon. This keeps us out of inline-value territory.
    if key.contains(':') {
        return None;
    }
    // After ':' must be end-of-line OR whitespace (then either end-of-line
    // for a parent mapping, or value).
    let after = &trimmed[colon_idx + 1..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        // e.g. `http://...` — colon is part of a value, not a key separator.
        return None;
    }
    Some((indent, key))
}

/// End (exclusive) of the block belonging to a key at line `key_line` with
/// indent `key_indent`. The block includes every following line whose
/// effective indent is `> key_indent` plus interleaved blank/comment
/// lines, stopping at the first line with indent `<= key_indent` that is
/// itself a mapping key (or end of file).
fn block_end_after(lines: &[&str], key_line: usize, key_indent: usize) -> usize {
    for (i, line) in lines.iter().enumerate().skip(key_line + 1) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent <= key_indent {
            return i;
        }
    }
    lines.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMENTED_FIXTURE: &str = "\
version: 2

# managers block: each manager is a long-running agent.
managers:
  pm:
    runtime: claude-code  # canonical runtime
    role_prompt: roles/pm.md
    # interfaces lands here once `teamctl bot setup` runs
  eng_lead:
    runtime: claude-code
    role_prompt: roles/eng_lead.md

# trailing footer
";

    #[test]
    fn round_trip_preserves_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.yaml");
        fs::write(&path, COMMENTED_FIXTURE).unwrap();

        let doc = load(&path).unwrap();
        save(&doc, &path).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(
            after, COMMENTED_FIXTURE,
            "load → save without mutation must be byte-perfect"
        );
    }

    #[test]
    fn mutation_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.yaml");
        fs::write(&path, COMMENTED_FIXTURE).unwrap();

        let doc = load(&path).unwrap();
        let doc = set_nested_mapping(
            doc,
            &["managers", "pm", "interfaces", "telegram"],
            &[("bot_token_env", "PM_TOKEN"), ("chat_ids_env", "PM_CHATS")],
        )
        .unwrap();
        save(&doc, &path).unwrap();

        let after = fs::read_to_string(&path).unwrap();

        assert!(
            after.contains("# managers block: each manager is a long-running agent."),
            "block comment dropped:\n{after}"
        );
        assert!(
            after.contains("# canonical runtime"),
            "trailing line comment dropped:\n{after}"
        );
        assert!(
            after.contains("# trailing footer"),
            "footer comment dropped:\n{after}"
        );
        assert!(
            after.contains("    interfaces:"),
            "interfaces not properly indented under pm:\n{after}"
        );
        assert!(
            after.contains("      telegram:"),
            "telegram not properly indented under interfaces:\n{after}"
        );
        assert!(
            after.contains("        bot_token_env: PM_TOKEN"),
            "leaf not properly indented:\n{after}"
        );
        assert!(after.contains("        chat_ids_env: PM_CHATS"));

        // Key ordering preserved on unchanged sections.
        let pm_idx = after.find("pm:").expect("pm key");
        let eng_idx = after.find("eng_lead:").expect("eng_lead key");
        assert!(pm_idx < eng_idx, "manager key order swapped:\n{after}");

        // Blank line separator between pm and eng_lead survives.
        assert!(
            after.contains("\n  eng_lead:"),
            "eng_lead boundary broken:\n{after}"
        );
    }

    /// Regression test for the dogfood-yaml class that hit PR #54 + PR #55.
    /// Saving through this substrate doesn't strip the comments the user
    /// put in their project YAML.
    #[test]
    fn save_does_not_strip_existing_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oss-shape.yaml");
        let fixture = "\
version: 2

project:
  id: oss
  name: OSS Maintainer
  cwd: ./workspace

# Hub-and-spoke: maintainer is the only manager; workers fan out below.
managers:
  maintainer:
    runtime: claude-code
    role_prompt: roles/maintainer.md
    # `teamctl bot setup` writes the interfaces.telegram block here.

workers:
  bug_fix:
    runtime: claude-code  # workers default to sonnet
    reports_to: maintainer
";
        fs::write(&path, fixture).unwrap();

        let doc = load(&path).unwrap();
        let doc = set_nested_mapping(
            doc,
            &["managers", "maintainer", "interfaces", "telegram"],
            &[
                ("bot_token_env", "TEAMCTL_TG_MAINTAINER_TOKEN"),
                ("chat_ids_env", "TEAMCTL_TG_MAINTAINER_CHATS"),
            ],
        )
        .unwrap();
        save(&doc, &path).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.contains(
                "# Hub-and-spoke: maintainer is the only manager; workers fan out below."
            ),
            "block comment dropped — regression class still open:\n{after}"
        );
        assert!(
            after.contains("# `teamctl bot setup` writes the interfaces.telegram block here."),
            "inline comment dropped:\n{after}"
        );
        assert!(
            after.contains("# workers default to sonnet"),
            "trailing line comment dropped:\n{after}"
        );
        assert!(after.contains("    interfaces:"));
        assert!(after.contains("      telegram:"));
        assert!(after.contains("        bot_token_env: TEAMCTL_TG_MAINTAINER_TOKEN"));
        assert!(after.contains("        chat_ids_env: TEAMCTL_TG_MAINTAINER_CHATS"));
    }

    /// Idempotency: re-running set_nested_mapping with the same path
    /// replaces the leaf in place rather than appending a duplicate.
    /// Sibling adapters under the same parent survive.
    #[test]
    fn idempotent_replace_preserves_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("siblings.yaml");
        let fixture = "\
version: 2
managers:
  pm:
    runtime: claude-code
    interfaces:
      discord:
        bot_token_env: PM_DISCORD_TOKEN
      telegram:
        bot_token_env: OLD_TOKEN
        chat_ids_env: OLD_CHATS
";
        fs::write(&path, fixture).unwrap();

        let doc = load(&path).unwrap();
        let doc = set_nested_mapping(
            doc,
            &["managers", "pm", "interfaces", "telegram"],
            &[
                ("bot_token_env", "NEW_TOKEN"),
                ("chat_ids_env", "NEW_CHATS"),
            ],
        )
        .unwrap();
        save(&doc, &path).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(
            after.matches("telegram:").count(),
            1,
            "duplicate telegram block:\n{after}"
        );
        assert_eq!(
            after.matches("discord:").count(),
            1,
            "discord sibling lost:\n{after}"
        );
        assert!(
            after.contains("PM_DISCORD_TOKEN"),
            "discord adapter contents lost:\n{after}"
        );
        assert!(after.contains("NEW_TOKEN"));
        assert!(after.contains("NEW_CHATS"));
        assert!(!after.contains("OLD_TOKEN"));
        assert!(!after.contains("OLD_CHATS"));
    }
}
