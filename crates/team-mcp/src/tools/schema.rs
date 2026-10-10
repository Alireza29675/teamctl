use serde_json::{json, Value};

/// JSON-Schema-ish tool list for `tools/list`.
pub fn schema() -> Value {
    json!([
        {
            "name": "whoami",
            "description": "Return the caller's fully-qualified agent id.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "dm",
            "description": "Send a direct message to another agent (same project). Returns the new message id.",
            "inputSchema": {
                "type": "object",
                "required": ["to", "text"],
                "properties": {
                    "to":        { "type": "string", "description": "Target agent id. Either `<project>:<agent>` or a bare `<agent>` in the caller's project." },
                    "text":      { "type": "string" },
                    "thread_id": {
                        "type": "string",
                        "description": "Optional, but the norm on replies. Group this DM into its conversation thread: when the message you're answering carried a `thread_id` in its channel meta, pass that same value here so the conversation stays coherent across the timeline; omit it (start a fresh thread) only when you begin a genuinely new topic. On cross-project *bridged* DMs the bridge sets the thread id for you, so any `thread_id` you pass is ignored."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_peek",
            "description": "Return up to `limit` unacked messages addressed to the caller. Non-destructive — peek does not mark anything resolved. Use this to browse the queue; use `inbox_read` to commit to handling specific ids (which fetches + auto-acks).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_read",
            "description": "Fetch full bodies for the listed message ids and mark them resolved in the same call (T-104: read-is-resolve). The default channel notification is a stub; call `inbox_read` with the stub's `meta.id` to drill in. Ids the caller can't see, or already-acked ids, are silently skipped.",
            "inputSchema": {
                "type": "object",
                "required": ["ids"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_ack",
            "description": "Mark the listed message ids as acknowledged without reading the body — use to dismiss messages you've decided not to handle. To handle (read + resolve), use `inbox_read` instead.",
            "inputSchema": {
                "type": "object",
                "required": ["ids"],
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "inbox_watch",
            "description": "Block up to `timeout_ms` milliseconds waiting for a new message. Returns immediately if any are pending.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeout_ms": { "type": "integer", "minimum": 0, "maximum": 60000, "default": 15000 }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "broadcast",
            "description": "Post a message to a channel in the caller's project. Caller must be a channel member and have the channel listed in can_broadcast.",
            "inputSchema": {
                "type": "object",
                "required": ["channel", "text"],
                "properties": {
                    "channel": { "type": "string" },
                    "text":    { "type": "string" }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "list_team",
            "description": "List every agent in the caller's project (project-scoped; never returns other projects).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "org_chart",
            "description": "Return the project's org chart: managers (top tier) and workers with their `reports_to` links. Use to introspect who is above you.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "reply_to_user",
            "description": "Send a message to the human operator. Available only to managers (`is_manager: true`); the configured interface adapter (Telegram, Discord, …) forwards it. \n\nThis is the ONLY channel back to the human — anything you write outside this tool is invisible to them. Use it to answer their DMs, surface progress on long-running work, escalate blockers, or proactively share something they should know. Do NOT use `dm` for human traffic (it is project-scoped inter-agent). \n\nFor work that takes more than a minute, send a brief acknowledgement first (e.g. \"on it — checking the build\") and then a separate reply when done; do not leave the operator wondering whether you started. \n\nAttach an `image` (jpg/png/webp/gif, ≤50MB) or `file` (any type, ≤50MB) by passing `{source: \"path\"|\"url\", value: \"<path or URL>\", caption?: \"<short caption>\"}`. Each of `text`, `image`, `file` lands as its own chat message; combine them in one call to send a screenshot with a follow-up sentence in a single tool invocation. At least one of `text`, `image`, `file` is required.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text":      {
                        "type": "string",
                        "description": "Mostly plain text. A small markdown subset is rendered: `**bold**`, `__bold__`, `*italic*`, `` `code` ``, and triple-backtick fenced code blocks (with optional language tag). Single-underscore italic (`_text_`) is intentionally not rendered — underscore is too common in code-style text. Bullet lines (`- item` / `* item` / `+ item`) become `• item`. Plain `<`, `>`, `&` are safe — escaped automatically. Headings, tables, and `>` blockquotes render only on a *fresh* message (omit `reply_to_message_id`); on a threaded reply they fall back to plain text, while everything above still renders on both. Use emojis sparingly to aid scanability (✅ done, ⚠️ caution, 🔧 working, ❓ question). Aim for short, chat-sized messages; split long output into multiple calls rather than sending a wall of text. Hard limit: 4096 characters per message (Telegram); longer text is rejected with an actionable error, so split proactively."
                    },
                    "image": {
                        "type": "object",
                        "description": "Image attachment. Sources: `path` (absolute path on the manager's machine) or `url` (publicly fetchable). Allowed types: jpg/jpeg/png/webp/gif. Path-source files must be ≤50MB.",
                        "required": ["source", "value"],
                        "properties": {
                            "source":  { "type": "string", "enum": ["path", "url"] },
                            "value":   { "type": "string", "description": "Absolute filesystem path or public URL." },
                            "caption": { "type": "string", "description": "Optional caption rendered under the photo. Plain text, ≤1024 chars per Telegram; longer is rejected with an actionable error." }
                        },
                        "additionalProperties": false
                    },
                    "file": {
                        "type": "object",
                        "description": "File attachment. Same sources as `image`, no mime restriction beyond Telegram's own. Path-source files must be ≤50MB.",
                        "required": ["source", "value"],
                        "properties": {
                            "source":  { "type": "string", "enum": ["path", "url"] },
                            "value":   { "type": "string" },
                            "caption": { "type": "string", "description": "Optional caption. Plain text, ≤1024 chars per Telegram; longer is rejected with an actionable error." }
                        },
                        "additionalProperties": false
                    },
                    "thread_id": {
                        "type": "string",
                        "description": "Optional. Group this reply into its mailbox conversation thread: when the inbound message's channel meta carried a `thread_id`, pass that same value here so the conversation stays coherent across the timeline; omit it (start a fresh thread) only when you begin a genuinely new topic. Operator messages from Telegram currently arrive without a `thread_id`, so there's usually nothing to carry on this path — use `reply_to_message_id` for in-chat reply threading to the operator."
                    },
                    "reply_to_message_id": {
                        "type": "integer",
                        "description": "Optional. Mailbox id of the inbound user message you're replying to — pass the `meta.id` value from the channel envelope (or `inbox_peek.id`). The bot resolves it to the right Telegram message id at insert time and threads the reply below the operator's message in the chat client. Omit when sending a fresh message. Applies to all of `text` / `image` / `file` in this call — they all reply to the same parent."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "request_approval",
            "description": "Request a human decision. Binary by default (Approve/Deny); pass `options` for a multi-choice decision rendered as tappable buttons with an implicit Cancel. Blocks until decided/denied/expired/undeliverable (long-poll). Use before any tool call that publishes, deploys, pays, or sends externally, or whenever you'd otherwise ask the operator to pick between named alternatives. Response carries `status` plus, for multi-choice, the chosen `value`; free text the operator types around the decision reaches you separately via your normal inbox. Terminal `undeliverable` means the prompt was never marked delivered to a human surface — distinct from `expired` (delivered but no decision in time).",
            "inputSchema": {
                "type": "object",
                "required": ["action", "summary"],
                "properties": {
                    "action":     { "type": "string", "description": "Coarse category, e.g. publish, deploy, payment." },
                    "scope_tag":  { "type": "string", "description": "Optional narrower tag for auto-approval matching." },
                    "summary":    { "type": "string" },
                    "payload":    { "type": "object" },
                    "options": {
                        "type": "array",
                        "description": "Optional. Labeled choices for a multi-option decision. Omit for the binary Approve/Deny card. Keep `label` terse (it's a Telegram button); put any richer context in `value`, which is what comes back to you. An implicit Cancel is always added — don't include one.",
                        "items": {
                            "type": "object",
                            "required": ["label", "value"],
                            "properties": {
                                "label": { "type": "string", "description": "Short button text the operator taps." },
                                "value": { "type": "string", "description": "Returned to you verbatim as `value` when this option is chosen." }
                            },
                            "additionalProperties": false
                        }
                    },
                    "ttl_seconds":{ "type": "integer", "minimum": 30, "maximum": 3600, "default": 900 },
                    "wait":       { "type": "boolean", "default": true, "description": "When false, return immediately after inserting the row (status=pending, delivered_at=null). Useful for diagnostics and non-blocking flows." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "show_typing",
            "description": "Show a Telegram \"typing…\" indicator in the operator's chat. Available only to managers (`is_manager: true`). Use right before kicking off work that takes more than a moment to produce visible output, so the operator gets a social cue that you're working rather than staring at silence. The indicator clears the moment any text from `reply_to_user` reaches the chat, or after a 10-second ceiling — whichever comes first. Calling `show_typing` again within an active window extends the ceiling (resets the 10s clock). No-op-safe: calling repeatedly is fine.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "read_attachment",
            "description": "Read a file the operator attached to a message. The bot/CLI surfaces attachments as a body line `📎 attachment: <absolute-path>`; pass that path here to receive a staging tempfile path the agent can `read_file()` directly. The broker enforces three guards: the path must canonicalize beneath one of `attachments.allowed_roots` (default `[$HOME]`), the file must be ≤ `max_size_bytes` (default 5 MB), and a configured `attachments.scanner` must return clean. Rejected reads return `{rejected: true, reason: \"…\"}` and never expose bytes. Operator gets a notification on telegram (when configured) and in the project's TUI Wire tab when the broker rejects. Staged tempfiles live in `<compose-root>/state/attachments-staging/` and are cleaned on team-mcp startup beyond the configured `tempfile_ttl_seconds` (default 6 h).",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path the operator passed in `📎 attachment: <path>`. Symlinks and relative paths are accepted but the broker canonicalizes before policy checks."
                    }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "compact_self",
            "description": "Compact your own context window via the `/compact` slash command. Available on `claude-code` and `codex` runtimes (both ship `/compact`; other runtimes don't recognize the slash command). \n\n**This is destructive: prior conversation detail is summarized and irretrievably trimmed.** Use only when explicitly instructed by your role (e.g. \"compact after every completed task\") or when you have clearly finished a major chunk of work and want to free space for the next one. Do not call this casually — every call permanently loses turns from your working window. \n\nFire-and-forget: the call returns immediately, and the `/compact` slash command lands in your tmux pane within a few milliseconds. Compaction itself runs asynchronously inside your session. The tool only routes; it does not block on the compaction completing.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "react_to_user",
            "description": "Apply an emoji reaction to a specific Telegram message from the operator. Available only to managers (`is_manager: true`). Use to acknowledge an inbound DM lightly without sending a full reply — 👀 to signal you're on it, ✍ to signal you're typing, 👍 to ack done. Each `react_to_user` call replaces any previous bot reaction on that message; pass an unsupported emoji and the call rejects with a clear error before reaching Telegram. The set of allowed emoji is the standard Telegram bot-reaction set (premium-tier-agnostic, ~75 emoji); use what you'd reach for in normal chat reactions. Pass the `telegram_msg_id` value from the inbound mailbox row you're reacting to.",
            "inputSchema": {
                "type": "object",
                "required": ["telegram_msg_id", "emoji"],
                "properties": {
                    "telegram_msg_id": {
                        "type": "integer",
                        "description": "Telegram message id to react to. Pass the `telegram_msg_id` value from the inbound mailbox row you're acknowledging."
                    },
                    "emoji": {
                        "type": "string",
                        "description": "Reaction emoji. Must be one of the allowed bot-reaction emojis (👍 👎 ❤️ 🔥 🥰 👏 😁 🤔 🤯 😱 🤬 😢 🎉 🤩 🤮 💩 🙏 👌 🕊 🤡 🥱 🥴 😍 🐳 💯 🤣 ⚡ 🍌 🏆 💔 🤨 😐 🍓 🍾 💋 🖕 😈 😴 😭 🤓 👻 👀 🎃 🙈 😇 😨 🤝 ✍ 🤗 🫡 🎅 🎄 ☃ 💅 🤪 🗿 🆒 💘 🙉 🦄 😘 💊 🙊 😎 👾 🤷 😡 plus a few combos like ❤️‍🔥 🌚 🌭 👨‍💻). Out-of-set emoji rejected at the MCP boundary."
                    }
                },
                "additionalProperties": false
            }
        }
    ])
}
