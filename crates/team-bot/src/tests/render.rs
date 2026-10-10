#[test]
fn render_html_paired_bold_emphasis() {
    assert_eq!(render_html("**bold** text"), "<b>bold</b> text");
    assert_eq!(render_html("__also bold__"), "<b>also bold</b>");
}

#[test]
fn render_html_paired_italic_and_inline_code() {
    assert_eq!(render_html("*italic* text"), "<i>italic</i> text");
    assert_eq!(
        render_html("plain `code` here"),
        "plain <code>code</code> here"
    );
}

#[test]
fn render_html_translates_list_bullets() {
    let input = "- one\n- two\n  * nested\n+ three";
    let expected = "• one\n• two\n  • nested\n• three";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_preserves_emoji_and_converts_inline() {
    let input = "🔐 deploy\nrouting prompt to one channel — the **right** one";
    let expected = "🔐 deploy\nrouting prompt to one channel — the <b>right</b> one";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_leaves_single_underscore_alone() {
    // Underscore is too common in dev text to convert. `thread_id`,
    // `snake_case_var`, `path/with_underscore` must all survive.
    assert_eq!(render_html("thread_id"), "thread_id");
    assert_eq!(render_html("snake_case_var here"), "snake_case_var here");
    assert_eq!(render_html("_underscored_"), "_underscored_");
}

#[test]
fn render_html_unmatched_delimiters_pass_through() {
    // Single `*` or `` ` `` with no closing partner on the same line
    // must reach the operator verbatim. The regression that motivated
    // T-134 was `array[i] = b * 2` losing its `*`.
    assert_eq!(render_html("array[i] = b * 2"), "array[i] = b * 2");
    assert_eq!(render_html("unmatched `tick"), "unmatched `tick");
    assert_eq!(
        render_html("unmatched **bold-open"),
        "unmatched **bold-open"
    );
}

#[test]
fn render_html_pairing_is_per_line() {
    // An open `*` on one line cannot pair with a `*` on the next.
    let input = "*open\nclose*";
    assert_eq!(render_html(input), "*open\nclose*");
}

#[test]
fn render_html_escapes_lt_gt_amp_in_raw_text() {
    // Quoting a `<channel>` tag must not break Telegram's HTML
    // parser AND must not drop characters.
    assert_eq!(
        render_html("<channel source=\"team\"> & friends"),
        "&lt;channel source=\"team\"&gt; &amp; friends",
    );
}

#[test]
fn render_html_escapes_inside_inline_code() {
    // Telegram's HTML parser requires escaping inside `<code>` too.
    assert_eq!(
        render_html("see `<thing>` for more"),
        "see <code>&lt;thing&gt;</code> for more",
    );
}

#[test]
fn render_html_fenced_block_no_language() {
    let input = "before\n```\nlet x = 1;\n```\nafter";
    let expected = "before\n<pre>let x = 1;</pre>\nafter";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_fenced_block_with_language_tag() {
    let input = "```rust\nfn main() {}\n```";
    let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_fenced_block_escapes_html_inside() {
    let input = "```\n<channel> & co\n```";
    let expected = "<pre>&lt;channel&gt; &amp; co</pre>";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_unmatched_fence_falls_through_as_normal_line() {
    // A lone ``` with no closer must not swallow the rest of the
    // message — emit it as a regular line (escaping handles the
    // backticks fine, since they're delimiters not html-special).
    let input = "```\nstray";
    // Backticks-only opener falls through to inline pass; the lone
    // ``` becomes a `<code>` opener with no close, which itself
    // falls through → literal. Result is the input verbatim.
    assert_eq!(render_html(input), "```\nstray");
}

#[test]
fn html_escape_str_escapes_the_three_html_specials_only() {
    // Pin the exact escape table — T-140 leans on this for the HITL
    // approval card (`agent`/`action`), the `forward_row`
    // attribution suffix (`row.sender`), and the fence-marker
    // language tag. Quote chars are intentionally NOT escaped
    // because they only matter inside attributes, and the only
    // attribute we emit (`class="language-…"`) substitutes a tag
    // that's already been through this escape.
    assert_eq!(
        html_escape_str("<channel> & friends"),
        "&lt;channel&gt; &amp; friends",
    );
    assert_eq!(
        html_escape_str("safe-text_with.no.specials"),
        "safe-text_with.no.specials",
    );
    // Quotes pass through verbatim — DiD relies on this NOT being
    // escaped so the renderer doesn't double-encode operator-typed
    // quotes inside otherwise-plain agent text.
    assert_eq!(html_escape_str("she said \"hi\""), "she said \"hi\"");
}

#[test]
fn fence_marker_takes_leading_alphanumeric_dash_underscore_run_as_lang() {
    // T-149: the language tag is the leading run of `[A-Za-z0-9_-]`
    // — any other byte (whitespace, quote, slash, non-ASCII)
    // ends the tag. Cases inherited from T-140 (whitespace
    // termination) still hold under the tighter rule.
    assert_eq!(fence_marker("```").as_deref(), Some(""));
    assert_eq!(fence_marker("```rust").as_deref(), Some("rust"));
    assert_eq!(fence_marker("```rust // example").as_deref(), Some("rust"));
    assert_eq!(
        fence_marker("    ```python   extra junk").as_deref(),
        Some("python"),
    );
    assert_eq!(fence_marker("not a fence").as_deref(), None);
}

#[test]
fn fence_marker_admits_dash_and_underscore_in_lang() {
    // Real-world language tags use both — `shell-script`,
    // `objective-c`, `objective_c` all need to round-trip so we
    // don't regress real syntax-highlighter classes.
    assert_eq!(
        fence_marker("```shell-script").as_deref(),
        Some("shell-script"),
    );
    assert_eq!(
        fence_marker("```objective-c").as_deref(),
        Some("objective-c"),
    );
    assert_eq!(
        fence_marker("```objective_c").as_deref(),
        Some("objective_c"),
    );
    // Leading dash/underscore is intentionally allowed: the
    // `take_while` predicate has no leading-char restriction,
    // and both chars are in the allowed set so there's zero
    // injection risk. Pin the shape so a future reader doesn't
    // wonder whether `-rust` should have been rejected (per
    // ada's #154 peer review).
    assert_eq!(fence_marker("```-rust").as_deref(), Some("-rust"));
    assert_eq!(fence_marker("```_rust").as_deref(), Some("_rust"));
}

#[test]
fn fence_marker_truncates_lang_at_quote_for_attribute_injection_safety() {
    // T-149 regression: `html_escape_into` escapes `<>&` but NOT
    // `"`. A fence like ```` ```"x ```` previously yielded
    // `lang = "\"x"` which then landed inside `class="language-…"`
    // and broke the parser. The tighter parse-boundary rule
    // ends the tag at the first non-`[A-Za-z0-9_-]` byte, so the
    // quote (and anything after it) is dropped.
    assert_eq!(fence_marker("```\"x").as_deref(), Some(""));
    assert_eq!(fence_marker("```rust\"injected\"").as_deref(), Some("rust"));
    assert_eq!(fence_marker("```\"").as_deref(), Some(""));
}

#[test]
fn fence_marker_truncates_lang_at_other_punctuation() {
    // Slashes, dots, and other punctuation also end the tag —
    // none of these belong in a syntax-highlighter class anyway,
    // and dropping them is the safer default than letting them
    // through into the rendered HTML.
    assert_eq!(fence_marker("```ru/st").as_deref(), Some("ru"));
    assert_eq!(fence_marker("```py.thon").as_deref(), Some("py"));
    assert_eq!(fence_marker("```rust!").as_deref(), Some("rust"));
    assert_eq!(fence_marker("```c++").as_deref(), Some("c"));
}

#[test]
fn fence_marker_truncates_lang_at_non_ascii() {
    // Non-ASCII chars (emoji, unicode letters) are valid Rust
    // identifier characters under `is_alphanumeric()` but
    // shouldn't appear in a syntax-highlighter class. Restrict
    // to ASCII alphanumerics so an agent emitting ```` ```rüst ````
    // produces an empty lang tag (renders as plain `<pre>`)
    // instead of a class attribute with non-ASCII bytes.
    assert_eq!(fence_marker("```rüst").as_deref(), Some("r"));
    assert_eq!(fence_marker("```🦀rust").as_deref(), Some(""));
    assert_eq!(fence_marker("```rust🦀").as_deref(), Some("rust"));
}

#[test]
fn render_html_fenced_block_drops_injected_quote_in_lang_tag() {
    // T-149 round-trip: an agent emitting a fence with a quote
    // in the lang tag gets a clean fallback (empty tag → plain
    // `<pre>`) — the prior parser would have produced
    // `<pre><code class="language-"x">…` and broken Telegram's
    // HTML parser.
    let input = "```\"x\nbody\n```";
    let expected = "<pre>body</pre>";
    assert_eq!(render_html(input), expected);

    // Quote-after-valid-lang case: `rust` survives, the quote
    // and everything after it is dropped before the class
    // attribute is built.
    let input = "```rust\"injected\nfn main() {}\n```";
    let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
    assert_eq!(render_html(input), expected);
}

#[test]
fn hitl_card_text_format_pins_agent_then_action_then_summary() {
    // Pin the HITL approval card's format-string composition so
    // a future edit can't silently swap `agent`/`action` order or
    // drop one of the escapes — the standalone
    // `html_escape_str_escapes_the_three_html_specials_only` test
    // covers the function, not the composition (per ada's #145
    // peer review).
    let id: i64 = 42;
    let agent = "pm";
    let action = "approve";
    let summary = "ship the **release**";
    let actual = format!(
        "🔐 #{id}  {}\naction: {}\n{}",
        html_escape_str(agent),
        html_escape_str(action),
        render_html(summary),
    );
    assert_eq!(
        actual,
        "🔐 #42  pm\naction: approve\nship the <b>release</b>",
    );

    // And the in-schema-but-with-html-specials variant — confirms
    // escape fires before the format-string lands on Telegram.
    let actual_escaped = format!(
        "🔐 #{id}  {}\naction: {}\n{}",
        html_escape_str("ops:<bot>"),
        html_escape_str("kill & restart"),
        render_html(summary),
    );
    assert_eq!(
        actual_escaped,
        "🔐 #42  ops:&lt;bot&gt;\naction: kill &amp; restart\nship the <b>release</b>",
    );
}

#[test]
fn render_html_fenced_block_strips_trailing_lang_garbage() {
    // T-140 round-trip: a fence with `lang + comment` reaches the
    // operator with a clean `class="language-lang"` attribute.
    let input = "```rust // example\nfn main() {}\n```";
    let expected = "<pre><code class=\"language-rust\">fn main() {}</code></pre>";
    assert_eq!(render_html(input), expected);
}

#[test]
fn render_html_inline_code_is_not_re_parsed() {
    // Backtick content is NOT recursively converted — `**bold**`
    // inside a code span renders as literal text, not as <b>.
    assert_eq!(render_html("`**not bold**`"), "<code>**not bold**</code>",);
}
