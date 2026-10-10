/// Render a small markdown subset to Telegram HTML so agent messages reach
/// the operator with formatting intact AND legitimate `_`/`*`/`` ` ``
/// characters preserved (T-134). Conservative whitelist: `**bold**`,
/// `__bold__`, `*italic*`, `` `code` ``, fenced code blocks (with optional
/// language tag — restricted to `[A-Za-z0-9_-]` per T-149, so e.g.
/// `` ```c++ `` renders with `class="language-c"`), and the existing
/// `- item` / `* item` / `+ item` → `• item` bullet glyph.
/// Single-underscore italic (`_text_`) is intentionally NOT converted
/// — underscore is too common in dev text
/// (`snake_case`, `thread_id`, URLs, paths). Inline conversion is
/// paired-only on the same line; an unmatched `*` or `` ` `` passes
/// through literally. `<`, `>`, `&` are escaped in every raw segment
/// (including inside `<code>` / `<pre>` per Telegram's HTML parser).
///
/// Callers pair the output with `.parse_mode(ParseMode::Html)` on the
/// teloxide send. Telegram supports a fixed HTML whitelist (`<b>`,
/// `<i>`, `<u>`, `<s>`, `<code>`, `<pre>`, `<a>`, `<tg-spoiler>`); the
/// emitted tags here stay inside that whitelist.
pub(super) fn render_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    let lines: Vec<&str> = s.lines().collect();
    let mut i = 0;
    let mut first = true;
    while i < lines.len() {
        if !first {
            out.push('\n');
        }
        first = false;
        let line = lines[i];
        // Fenced code block? Look for a matching close on a later line.
        if let Some(lang) = fence_marker(line) {
            let close_idx = ((i + 1)..lines.len()).find(|&j| fence_marker(lines[j]).is_some());
            if let Some(close) = close_idx {
                if lang.is_empty() {
                    out.push_str("<pre>");
                } else {
                    out.push_str("<pre><code class=\"language-");
                    html_escape_into(&mut out, &lang);
                    out.push_str("\">");
                }
                for (k, body_line) in lines[(i + 1)..close].iter().enumerate() {
                    if k > 0 {
                        out.push('\n');
                    }
                    html_escape_into(&mut out, body_line);
                }
                if lang.is_empty() {
                    out.push_str("</pre>");
                } else {
                    out.push_str("</code></pre>");
                }
                i = close + 1;
                continue;
            }
            // Unmatched fence: fall through and treat as a normal line.
        }
        render_normal_line(line, &mut out);
        i += 1;
    }
    out
}

/// Return `Some(lang)` (possibly empty) if `line` is a fence marker
/// (`` ``` `` optionally followed by a language tag). Leading
/// whitespace is permitted. The language tag is parsed at the
/// boundary as the leading run of `[A-Za-z0-9_-]` characters — any
/// other byte (whitespace, quote, slash, non-ASCII, …) ends the
/// tag and the rest of the line is dropped. T-149 closes the
/// quote-in-attribute injection vector this way: a fence like
/// `` ```"x `` previously yielded `lang = "\"x"`, which then landed
/// inside `class="language-…"` and broke the parser. Schema-tighten
/// at the parse boundary instead of growing the escaper to handle
/// quotes — the ASCII-class rule also matches how every real syntax
/// highlighter (HighlightJS, Pygments, chroma) classifies a
/// language tag, so we lose no real-world capability.
pub(super) fn fence_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let after = trimmed.strip_prefix("```")?;
    Some(
        after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect(),
    )
}

pub(super) fn render_normal_line(line: &str, out: &mut String) {
    let trimmed = line.trim_start();
    let leading = &line[..line.len() - trimmed.len()];
    let body = if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
    {
        format!("• {rest}")
    } else {
        trimmed.to_string()
    };
    out.push_str(leading);
    render_inline_html(&body, out);
}

/// Inline-pass markdown → HTML for one line. Recognised: `**…**`,
/// `__…__` → `<b>`; `*…*` → `<i>`; `` `…` `` → `<code>`. Pairing is
/// per-line: an open delimiter without a matching close on the same
/// line falls through to escaped-literal output. Content inside a
/// recognised pair is HTML-escaped but NOT recursively re-parsed for
/// further markdown.
pub(super) fn render_inline_html(s: &str, out: &mut String) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Pair lookup requires non-empty content — `<b></b>`,
        // `<i></i>`, `<code></code>` are never desired output, and the
        // empty-content match is what causes a stray `**` or backtick
        // run to swallow itself instead of passing through literally.
        if bytes.get(i..i + 2) == Some(b"**") {
            if let Some(end) = s[i + 2..].find("**").filter(|&e| e > 0) {
                let close = i + 2 + end;
                out.push_str("<b>");
                html_escape_into(out, &s[i + 2..close]);
                out.push_str("</b>");
                i = close + 2;
                continue;
            }
        }
        if bytes.get(i..i + 2) == Some(b"__") {
            if let Some(end) = s[i + 2..].find("__").filter(|&e| e > 0) {
                let close = i + 2 + end;
                out.push_str("<b>");
                html_escape_into(out, &s[i + 2..close]);
                out.push_str("</b>");
                i = close + 2;
                continue;
            }
        }
        if bytes[i] == b'`' {
            if let Some(end) = s[i + 1..].find('`').filter(|&e| e > 0) {
                let close = i + 1 + end;
                out.push_str("<code>");
                html_escape_into(out, &s[i + 1..close]);
                out.push_str("</code>");
                i = close + 1;
                continue;
            }
        }
        if bytes[i] == b'*' {
            if let Some(end) = s[i + 1..].find('*').filter(|&e| e > 0) {
                let close = i + 1 + end;
                out.push_str("<i>");
                html_escape_into(out, &s[i + 1..close]);
                out.push_str("</i>");
                i = close + 1;
                continue;
            }
        }
        let next = s[i..]
            .chars()
            .next()
            .expect("byte index inside string bounds yields a char");
        match next {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(next),
        }
        i += next.len_utf8();
    }
}

/// Escape `<`, `>`, `&` per Telegram's HTML parse mode. Quote escaping
/// is unnecessary outside attributes; the only attribute we emit is
/// `class="language-…"` and the language tag is HTML-escaped before
/// substitution.
pub(super) fn html_escape_into(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
}

pub(super) fn html_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    html_escape_into(&mut out, s);
    out
}

/// #479: backslash-escape the markdown specials that could mangle an
/// interpolated agent id in the rich-message attribution line.
pub(super) fn markdown_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '_' | '*' | '`' | '[' | ']' | '(' | ')' | '~' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ── T-101 voice STT ────────────────────────────────────────────────────
