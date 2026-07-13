#!/bin/sh
# teamctl agent wrapper.
#
# Invoked by the tmux session `teamctl up` creates. Responsible for:
#   - sourcing the per-agent env file (via the tmux command's `env`)
#   - looping on the runtime so crashes auto-restart
#   - routing every runtime invocation through `teamctl rl-watch` so
#     the runtime gets a real pty (interactive REPL), rate-limit
#     signatures get parsed, hooks fire, and we sleep until the limit
#     window has cleared before respawning.
#
# This file is teamctl-managed: `teamctl up` rewrites it on every run.
# Customize behaviour through env vars (BOOTSTRAP_PROMPT, MODEL, ...)
# rather than editing the script.
#
# First positional arg is `<project>:<agent>`.

set -u

AGENT="${1:-${AGENT_ID:-}}"
if [ -z "$AGENT" ]; then
    echo "agent-wrapper: AGENT id not provided (arg or \$AGENT_ID)" >&2
    exit 2
fi

: "${RUNTIME:=claude-code}"
: "${MODEL:=}"
: "${PERMISSION_MODE:=}"
: "${CLAUDE_SETTINGS:=}"
: "${MCP_CONFIG:=}"
: "${CLAUDE_AGENTS_JSON:=}"
: "${CLAUDE_AGENT_SCOPE:=}"
# Rendered into the env file only for codex runtime (per-agent state
# root carrying the [mcp_servers.*] config). Default to empty so the
# codex arm's `[ -n "$CODEX_HOME" ]` guard is set -u-safe for other
# runtimes and older env files.
: "${CODEX_HOME:=}"
# Rendered into the env file only for opencode runtime: OPENCODE_DB
# relocates the session sqlite db (the per-agent isolation mechanism)
# and OPENCODE_CONFIG points at the per-agent config json carrying the
# MCP servers + instructions. Default to empty so the opencode arm's
# guards are set -u-safe for other runtimes and older env files.
: "${OPENCODE_DB:=}"
: "${OPENCODE_CONFIG:=}"
: "${SYSTEM_PROMPT_PATH:=}"
: "${CLAUDE_PROJECT_DIR:=.}"
: "${TEAMCTL_ROOT:=$CLAUDE_PROJECT_DIR}"
# Rendered into the env file only when the YAML `effort:` field is set.
# Default to empty here so `set -u` doesn't trip the `[ -n "$EFFORT" ]`
# check below for agents that omit it.
: "${EFFORT:=}"
# T-118 / T-174: rendered into the env file only for claude-code
# runtime. Default to empty under `set -u` so the wrapper's
# `[ -n "$CLAUDE_SESSION_ID" ]` and `[ -n "$CLAUDE_SESSION_NAME" ]`
# checks below are safe even when these vars are absent from the env
# file (env file written by an older teamctl render, missing-var
# write race, non-claude-code runtime co-existing in the same
# wrapper). Without this default, `set -u` aborts the wrapper at the
# unguarded reference, the tmux pane closes immediately, and the
# supervisor marks the agent stopped without ever printing a
# diagnostic.
: "${CLAUDE_SESSION_ID:=}"
: "${CLAUDE_SESSION_NAME:=}"
# T-190: macOS ships bash 3.2 as `/bin/sh`. Bash 3.2 has a parser
# bug in `${VAR:=DEFAULT}` parameter-expansion: it cannot reliably
# parse escape sequences inside the DEFAULT (backslash-backtick,
# backslash-quote). This wrapper's `BOOTSTRAP_PROMPT` default
# contains both — `\`<channel source=\"team\">\`` and friends — so
# every spawn on macOS aborts at this line with "unexpected EOF
# while looking for matching `}`", the tmux pane closes, and the
# supervisor marks the agent stopped. Linux dash + bash 4+ parse
# the construct correctly, which is why this regression hid through
# Linux qa.
#
# Fix: pull the default OUT of `${VAR:=...}` form into a regular
# conditional assignment. A double-quoted string literal parses
# identically on bash 3.2 / 4+ / dash, so the escapes work everywhere.
# Behavior is unchanged when BOOTSTRAP_PROMPT is already set by the
# env file (the `[ -z ]` short-circuits).
#
# The default dispatches on runtime because delivery differs: Claude
# Code Channels push `<channel>` events into the session, but every
# other runtime treats MCP as strictly request/response and drops
# unsolicited notifications — for those agents team-mcp types a short
# "📬 N new team message(s)" nudge into the pane instead, and telling
# them "you do not need to poll, events arrive" would leave them idle
# forever waiting for events that never come.
if [ -z "${BOOTSTRAP_PROMPT:-}" ]; then
    case "$RUNTIME" in
        claude-code)
            BOOTSTRAP_PROMPT="Begin your shift as ${AGENT}. Team traffic is delivered to you as \`<channel source=\"team\">\` events via Claude Code Channels -- you do not need to poll. By default the body is a short \"📬 1 new message ...\" stub (meta.lazy=\"1\"); call \`inbox_read\` with the meta.id to fetch the full body and resolve it in one step. If the stub doesn't merit handling, call \`inbox_ack\` to dismiss. When the body lands inline (no meta.lazy, e.g. operator used \`/readnow\`), act on it directly and call \`inbox_ack\` on the id when done. Between events, idle. Use \`inbox_peek\` only for non-destructive catch-up after a restart."
            ;;
        *)
            BOOTSTRAP_PROMPT="Begin your shift as ${AGENT}. Call inbox_peek now to catch up on anything already waiting. Team traffic arrives as short '📬 N new team message(s)' notes typed into this session by the team mailbox -- when one lands, call inbox_peek, then inbox_read each meta.id and inbox_ack when handled. Between notes, idle. Never reply to the 📬 note itself; the mailbox is the source of truth."
            ;;
    esac
fi

cd "$CLAUDE_PROJECT_DIR" 2>/dev/null || true

log() {
    printf '[agent-wrapper %s] %s\n' "$AGENT" "$*" >&2
}

# The runtimes surface a handful of one-shot confirmation dialogs that
# strand a headless agent because no operator is at the keyboard:
#
#   - "Loading development channels"   — fires every wrapper start
#     while team-mcp is off Anthropic's allowlist (Channels research
#     preview).
#   - "Bypass Permissions mode"        — fires on first launch under
#     `permission_mode: bypassPermissions` (the opt-in escape hatch)
#     when the acceptance marker isn't on disk (fresh $HOME, new VM).
#   - "Stop and wait for limit to reset" — fires when claude hits a
#     usage-limit cap and asks the operator whether to wait, switch
#     to extra usage, or upgrade. Default-highlighted option is
#     "wait", which is the right choice for a supervised headless
#     agent (operator can intervene manually for a different choice).
#   - "Quick safety check:" (trust-folder) — fires on first launch in a
#     directory claude hasn't trusted yet, under any permission mode that
#     doesn't bypass permissions outright (e.g. `permission_mode: auto`,
#     the headless default since 0.8.7). `teamctl up` normally pre-accepts
#     this via ~/.claude.json, but that can miss when the launch cwd and
#     the recorded key differ (symlinked paths). Default-highlighted
#     option is "Yes, I trust this folder" and Enter confirms, so one
#     Enter accepts it. This is a ONE-TIME trust gate for the agent's own
#     working dir — NOT `auto`'s per-action safety classifier. We accept
#     only this dialog; we deliberately never auto-Enter `auto`'s
#     risky-action prompts, since doing so would approve risky actions
#     unattended and defeat the very classifier `auto` exists to provide.
#   - "New MCP server(s) found in this project" — fires when claude
#     discovers project-scoped MCP servers (a `.mcp.json`) it hasn't been
#     told to enable. Two shapes: a single-server radio menu (default
#     option "Use this MCP server") and a multi-server checkbox list (all
#     boxes pre-checked); in BOTH, Enter enables the server(s) — verified
#     against claude 2.1.161. The owner opted to auto-accept this one
#     silently for headless agents (no operator notice) so panes don't
#     stall; the general fix for the whole prompt class is #421.
#   - codex's first-run trust dialog    — once per directory. The exact
#     wording has drifted across codex releases ("Yes, I trust this
#     folder", "Yes, allow Codex to work in this folder", and 0.144's
#     "Do you trust the contents of this directory?"), so the pattern
#     matches all observed variants. This is a BACKSTOP: the rendered
#     config.toml pre-seeds `[projects."<cwd>"] trust_level = "trusted"`
#     so the dialog normally never renders at all. The trust option is
#     the default, so a single Enter accepts it — running `teamctl up` is
#     itself the "I trust this directory" signal (same rationale as the
#     claude-code trust pre-acceptance in `teamctl up`). Codex's "Hooks
#     need review" prompt is deliberately NOT auto-accepted: that's a
#     security review surface and belongs to an attached operator.
#
# The watcher polls our own tmux pane for any of these headers and
# sends one Enter when matched, then sleeps 1s so the dialog clears
# from the captured frame before the next poll (otherwise the same
# match would re-fire). The single-pattern strings are runtime chrome
# that doesn't occur in normal output (the codex trust wording is
# dialog-only, so it rides the same alternation). The claude trust and
# MCP dialogs are matched on a two-string co-occurrence rather than a
# single header, so an agent that merely *prints* one of the phrases
# can't trigger a stray Enter: trust needs "Quick safety check:" AND
# "trust this folder"; MCP needs "MCP servers may execute code" AND the
# footer chrome "Enter to confirm · Esc" (the interpunct footer doesn't
# occur in prose, so an agent discussing MCP security can't collide
# with it).
#
# The watcher runs for the full lifetime of the runtime (the limit
# prompt can fire at any point, not only at boot) and is reaped by
# the outer loop after the runtime exits. No-op outside tmux
# (TMUX_PANE unset).
auto_confirm_known_dialogs() {
    pane="${TMUX_PANE:-${TMUX_SESSION:-}}"
    [ -z "$pane" ] && return 0
    command -v tmux >/dev/null 2>&1 || return 0
    while :; do
        frame=$(tmux capture-pane -t "$pane" -p 2>/dev/null)
        if printf '%s\n' "$frame" \
            | grep -qE 'Loading development channels|Bypass Permissions mode|Stop and wait for limit to reset|Yes, I trust this folder|Yes, allow Codex to work in this folder|Do you trust the contents of this directory' \
            || { printf '%s\n' "$frame" | grep -q 'Quick safety check:' \
                 && printf '%s\n' "$frame" | grep -q 'trust this folder'; } \
            || { printf '%s\n' "$frame" | grep -q 'MCP servers may execute code' \
                 && printf '%s\n' "$frame" | grep -q 'Enter to confirm · Esc'; }; then
            tmux send-keys -t "$pane" Enter
            sleep 1
            continue
        fi
        sleep 0.5
    done
}

# A resumed TUI (codex `resume --last`, opencode `-c`) reopens the
# prior conversation but sits idle: no prompt is injected on the
# resume path (see the resume probes in the runtime cases below), so
# without a wake-up the agent would wait forever for input that never
# comes. One-shot nudge: sleep past the TUI boot, then type a short
# catch-up instruction into our own pane and press Enter. Deliberately
# dumb — one sleep, one send-keys, no retry loop; worst case the text
# sits in the composer for an attached operator to see. No-op outside
# tmux (TMUX_PANE unset), same guard as the auto-confirm watcher.
nudge_resumed_session() {
    pane="${TMUX_PANE:-${TMUX_SESSION:-}}"
    [ -z "$pane" ] && return 0
    command -v tmux >/dev/null 2>&1 || return 0
    sleep 5
    tmux send-keys -t "$pane" "Restarted mid-shift: call inbox_peek to catch up on anything that arrived while you were down, then resume your role." Enter
}

# Consecutive fast resume-path failures (see the self-heal block at
# the bottom of the loop). Lives outside the loop so it survives
# across restarts. Each resume-capable runtime case sets RESUMED=1
# when its probe matched.
RESUME_FAST_FAILS=0

# Build the runtime invocation as the script's positional parameters.
# Doing this in-line (instead of in a function) keeps the args quoted —
# previous versions stuffed everything into a single $BIN_ARGS string and
# re-split on whitespace, which silently corrupted multi-word values like
# the role prompt.
while :; do
    log "starting runtime=$RUNTIME model=${MODEL:-<default>}"
    # Per-iteration: each resume-capable runtime case sets this to 1 when
    # its resume probe matches. The self-heal blocks at the bottom of the
    # loop key on it; reset here so a non-resuming runtime (or a
    # fresh-session launch) never trips them.
    RESUMED=0
    case "$RUNTIME" in
        claude-code)
            BIN=claude
            set --
            # T-118: deterministic session id + display name so the
            # conversation persists across teamctl down/up + crash
            # recovery. UUIDv5 is rendered into the env file by
            # team-core (claude-code-only); claude creates the session
            # at this UUID on first spawn and resumes it on every
            # subsequent spawn. If the session-file at this UUID is
            # ever removed (manual cleanup, claude session-dir reset),
            # claude creates a fresh one at the same UUID — self-
            # healing by construction. The BOOTSTRAP_PROMPT keeps
            # being injected on every spawn (option 1 from #118):
            # claude's session-storage slug rule isn't a documented
            # contract, so a cold-vs-warm probe here would couple the
            # wrapper to a behavior that could shift between claude
            # versions. The bootstrap is small enough that warm-start
            # cost is trivial, and the model recognizes "I've already
            # done this".
            #
            # T-174: claude code 2.1.138 split `--session-id` into
            # create-only semantics — passing it on a UUID whose jsonl
            # already exists errors with "Session ID is already in
            # use". To attach to an existing session you must use
            # `--resume <UUID>` instead. Probe the on-disk session
            # path; when it exists, splice `--resume`. On a fresh
            # spawn (or after manual cleanup) the path is absent and
            # we fall through to the original `--session-id` shape —
            # the deterministic UUID is unchanged, only the flag
            # selection branches. The `-n` display-name flag is kept
            # on both branches: claude tolerates it on `--resume`
            # (no-op when the existing session already carries the
            # name) and we don't want to bet the agent's tmux
            # identity on undocumented persistence.
            #
            # Glob (`projects/*/...`) on purpose — claude's
            # cwd-to-project-dir slug is observed-not-documented
            # (currently `/` and `.` → `-`, may shift). The UUIDv5 is
            # globally unique per agent, so at most one file ever
            # matches and we never have to mirror that algorithm.
            if [ -n "$CLAUDE_SESSION_ID" ]; then
                # The session UUID is keyed on project:agent only (no cwd),
                # so two installs that share a project.id derive identical
                # ids. This probe globs every cwd slug (projects/*/), so
                # install B can match install A's jsonl, splice `--resume`,
                # and then claude — scoped to B's own slug — can't find it
                # ("No conversation found"), exits non-zero, and the loop
                # respawns into the same failure forever. The
                # FORCE_FRESH_SESSION one-shot (set after such a failure,
                # below) breaks that: it forces a `--session-id` launch,
                # which is cwd-scoped (verified against claude 2.1.175) — it
                # opens a fresh local session when none exists here (heals
                # the collision; later restarts then resume it normally), and
                # errors harmlessly ("already in use") when our own local
                # session does exist (a genuine crash), leaving the normal
                # resume to recover on the next pass. RESUMED lets that block
                # tell a resume launch from a fresh one.
                if [ "${FORCE_FRESH_SESSION:-0}" = 1 ]; then
                    set -- "$@" --session-id "$CLAUDE_SESSION_ID"
                    RESUMED=0
                elif ls "$HOME/.claude/projects/"*/"$CLAUDE_SESSION_ID.jsonl" >/dev/null 2>&1; then
                    set -- "$@" --resume "$CLAUDE_SESSION_ID"
                    RESUMED=1
                else
                    set -- "$@" --session-id "$CLAUDE_SESSION_ID"
                    RESUMED=0
                fi
            fi
            [ -n "$CLAUDE_SESSION_NAME" ] && set -- "$@" -n "$CLAUDE_SESSION_NAME"
            # T-189 / T-361: `permission_mode: attended` is the opt-out for
            # the headless-default footgun protections. When attended, a
            # human is at the keyboard and can answer interactive prompts,
            # so we skip both:
            #   - `--permission-mode` (claude has no "attended" mode — it's
            #     a teamctl-level concept; a human drives the normal prompts),
            #   - `--settings <hook-deny>` (let interactive tools run).
            # Any other permission_mode (or unset) means headless. We default
            # to `auto`: claude's classifier lets routine work run without
            # prompts and blocks risky actions outright, so an unattended pane
            # keeps draining its inbox instead of freezing on a permission
            # dialog. (Edge: if auto blocks 3x consecutively or 20x total in a
            # session it falls back to prompting — see CHANGELOG.) An operator
            # who genuinely needs the old bypass-everything behavior for a
            # disposable sandbox can set `permission_mode: bypassPermissions`,
            # which flows through here (no teamctl-specific escape hatch). We
            # also ship the deny hook so AskUserQuestion / plan-mode pickers
            # can't strand the pane.
            if [ "${PERMISSION_MODE:-}" = "attended" ]; then
                :
            else
                set -- "$@" --permission-mode "${PERMISSION_MODE:-auto}"
                [ -n "$CLAUDE_SETTINGS" ] && set -- "$@" --settings "$CLAUDE_SETTINGS"
            fi
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            # T-048: per-agent reasoning effort. Source order is YAML
            # (rendered into this env file) > workspace `.env` (env
            # inherited from the operator shell) > unset (claude's own
            # default). Empty string is treated as unset.
            [ -n "$EFFORT" ] && set -- "$@" --effort "$EFFORT"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            # #383 Phase 3a: per-agent sub-agents. render writes the
            # `--agents` JSON only when the agent declares `subagents:`, so
            # the `[ -f ]` guard means no flag is passed when the file is
            # absent. Command substitution passes the JSON as a single arg
            # (embedded newlines preserved, `$`/backticks not re-expanded).
            [ -n "$CLAUDE_AGENTS_JSON" ] && [ -f "$CLAUDE_AGENTS_JSON" ] && \
                set -- "$@" --agents "$(cat "$CLAUDE_AGENTS_JSON")"
            # #383 Phase 3b: per-agent skills. render materializes a scope
            # dir with symlinks to declared skills under
            # <scope>/.claude/skills/ only when the agent declares `skills:`,
            # so the `[ -d ]` guard means no flag is passed when the dir is
            # absent. `--add-dir` is variadic; it sits before the `--`
            # terminator below so the bootstrap prompt isn't slurped.
            [ -n "$CLAUDE_AGENT_SCOPE" ] && [ -d "$CLAUDE_AGENT_SCOPE" ] && \
                set -- "$@" --add-dir "$CLAUDE_AGENT_SCOPE"
            # Subscribe to the team mailbox via Claude Code Channels
            # (v2.1.80+). team-mcp emits `notifications/claude/channel`
            # for every new inbox row, which lands in this session as
            # a `<channel source="team">` event -- so the agent reacts
            # on arrival without polling and idles silently between
            # events. `server:team` references the `team` entry in the
            # MCP config rendered above.
            #
            # `--dangerously-load-development-channels` (not `--channels`)
            # is required while team-mcp is off Anthropic's allowlist
            # during the Channels research preview. `--channels` would
            # be silently dropped here.
            set -- "$@" --dangerously-load-development-channels server:team
            [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
                set -- "$@" --append-system-prompt "$(cat "$SYSTEM_PROMPT_PATH")"
            # `--` terminates the variadic dev-channels list so the bare
            # BOOTSTRAP_PROMPT positional isn't slurped as another channel
            # entry.
            set -- "$@" -- "$BOOTSTRAP_PROMPT"
            AUTO_CONFIRM=1
            ;;
        codex)
            BIN=codex
            set --
            # Codex has no --mcp-config flag: MCP servers live in
            # [mcp_servers.*] tables inside $CODEX_HOME/config.toml,
            # rendered per-agent by team-core. CODEX_HOME relocates the
            # entire state root (config, sessions, history) — the clean
            # per-agent isolation mechanism — but it relocates
            # credentials too, so symlink the operator's auth.json in;
            # without it every agent would demand its own device-flow
            # login.
            if [ -n "$CODEX_HOME" ]; then
                export CODEX_HOME
                mkdir -p "$CODEX_HOME"
                if [ -f "$HOME/.codex/auth.json" ] && [ ! -e "$CODEX_HOME/auth.json" ]; then
                    ln -s "$HOME/.codex/auth.json" "$CODEX_HOME/auth.json"
                fi
            fi
            # Session persistence: codex writes JSONL rollouts under
            # $CODEX_HOME/sessions/YYYY/MM/DD/ and has no deterministic
            # session-id-at-spawn flag (unlike claude's --session-id).
            # But the per-agent CODEX_HOME isolates the session store,
            # so "the most recent session in this home" IS this agent's
            # session — `codex resume --last` becomes exact. The
            # subcommand must lead the argv (`codex resume --last
            # [flags]`), so it's spliced in before the flags below. No
            # bootstrap positional on the resume path: whether `resume`
            # accepts a PROMPT is unverified upstream, so the one-shot
            # nudge (armed after the case) re-grounds the agent instead.
            # Self-healing like the claude branch: remove the sessions
            # dir and the next spawn falls through to a fresh boot, no
            # operator action needed.
            RESUMED=0
            if [ -n "$CODEX_HOME" ] && \
                ls "$CODEX_HOME/sessions"/*/*/*/*.jsonl >/dev/null 2>&1; then
                set -- resume --last
                RESUMED=1
            fi
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            # Codex has no --effort flag; reasoning effort rides the
            # repeatable `-c KEY=VALUE` config override. Values pass
            # through verbatim — codex validates them.
            [ -n "$EFFORT" ] && set -- "$@" -c "model_reasoning_effort=$EFFORT"
            # Codex has no --instructions flag either. The
            # model_instructions_file override supersedes codex's own
            # project AGENTS.md discovery for this agent — deliberate:
            # the rendered role prompt IS the agent's instruction set.
            [ -n "$SYSTEM_PROMPT_PATH" ] && [ -f "$SYSTEM_PROMPT_PATH" ] && \
                set -- "$@" -c "model_instructions_file=$SYSTEM_PROMPT_PATH"
            # Permission mapping, mirroring the claude-code branch:
            # attended means a human is at the keyboard, so codex keeps
            # its own interactive approval default (on-request);
            # bypassPermissions flows to --yolo (the opt-in
            # bypass-everything escape hatch, no teamctl-specific
            # variant); everything else (or unset) is headless —
            # approvals can't prompt in an unattended pane, so `-a
            # never` plus the workspace-write sandbox makes the sandbox
            # boundary the guardrail: out-of-workspace actions fail
            # visibly instead of stranding the pane on a prompt.
            if [ "${PERMISSION_MODE:-}" = "attended" ]; then
                :
            elif [ "${PERMISSION_MODE:-}" = "bypassPermissions" ]; then
                set -- "$@" --yolo
            else
                set -- "$@" -a never -s workspace-write
            fi
            # The codex TUI takes the bootstrap as a positional PROMPT —
            # fresh spawns only; a resumed session gets the nudge.
            [ "$RESUMED" = 0 ] && set -- "$@" "$BOOTSTRAP_PROMPT"
            AUTO_CONFIRM=1
            ;;
        opencode)
            BIN=opencode
            set --
            # OpenCode's TUI auto-upgrades the shared binary in place on
            # boot (observed 1.17.13→1.17.18, even under full env
            # isolation) — a mid-fleet binary swap. Disable it here AND
            # via `"autoupdate": false` in the rendered config: defense
            # in depth, upstream has open bugs about the config flag
            # being ignored on some paths.
            export OPENCODE_DISABLE_AUTOUPDATE=1
            # OPENCODE_DB (per-agent session sqlite db) and
            # OPENCODE_CONFIG (per-agent config json carrying the MCP
            # servers + instructions) arrive via the env file, rendered
            # by team-core for opencode agents only. Auth stays at the
            # real ~/.local/share/opencode/auth.json — neither var
            # relocates it, so no symlink is needed (unlike codex).
            # Missing auth does NOT error: opencode silently falls back
            # to free anonymous models, so warn loudly instead.
            if [ ! -f "$HOME/.local/share/opencode/auth.json" ]; then
                log "opencode has no credentials — it will silently run on free anonymous models; run \`opencode auth login\`"
            fi
            # Session resume: the per-agent OPENCODE_DB isolates the
            # session store, and `-c` reopens the most recent session in
            # the current directory within that db — exact per agent,
            # because the db holds only this agent's sessions. `-c` on a
            # db whose cwd has no prior session silently creates a fresh
            # one — self-healing like the claude branch. No --prompt on
            # the resume path: the one-shot nudge (armed after the case)
            # re-grounds the agent instead.
            RESUMED=0
            if [ -n "$OPENCODE_DB" ] && [ -f "$OPENCODE_DB" ]; then
                set -- "$@" -c
                RESUMED=1
            fi
            # Model is provider/model form (e.g. openai/gpt-5.4-mini-fast).
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            # No effort mapping: the plain opencode TUI rejects
            # `--variant` (run-subcommand only, verified exit 1), so
            # `effort:` is unsupported on opencode v1.
            # Permission mapping, mirroring the claude-code branch:
            # attended means a human answers opencode's own interactive
            # ask-prompts; everything else (or unset) is headless and
            # maps to --auto, which auto-approves anything not
            # explicitly denied. That includes bypassPermissions —
            # opencode has no full-bypass equivalent (the --yolo feature
            # request was closed not-planned upstream), so --auto is the
            # closest available and deny rules stay enforced.
            if [ "${PERMISSION_MODE:-}" = "attended" ]; then
                :
            else
                set -- "$@" --auto
            fi
            # The opencode TUI takes the bootstrap via --prompt (a flag,
            # NOT a positional) — fresh spawns only; a resumed session
            # gets the nudge. No AUTO_CONFIRM: opencode boots straight
            # to the composer, no first-run dialogs (verified).
            [ "$RESUMED" = 0 ] && set -- "$@" --prompt "$BOOTSTRAP_PROMPT"
            ;;
        gemini)
            BIN=gemini
            set --
            [ -n "$MODEL" ] && set -- "$@" --model "$MODEL"
            [ -n "$MCP_CONFIG" ] && set -- "$@" --mcp-config "$MCP_CONFIG"
            [ -n "$SYSTEM_PROMPT_PATH" ] && set -- "$@" --system-instruction-file "$SYSTEM_PROMPT_PATH"
            set -- "$@" --yolo "$BOOTSTRAP_PROMPT"
            ;;
        *)
            log "unknown runtime: $RUNTIME"
            sleep 30
            continue
            ;;
    esac

    AUTO_CONFIRM_PID=
    if [ "${AUTO_CONFIRM:-0}" = 1 ]; then
        auto_confirm_known_dialogs &
        AUTO_CONFIRM_PID=$!
    fi
    AUTO_CONFIRM=0

    NUDGE_PID=
    if [ "${RESUMED:-0}" = 1 ]; then
        nudge_resumed_session &
        NUDGE_PID=$!
    fi

    RUN_STARTED=$(date +%s)
    if command -v teamctl >/dev/null 2>&1; then
        teamctl --root "$TEAMCTL_ROOT" rl-watch "$AGENT" -- "$BIN" "$@"
    else
        log "teamctl not on PATH — running runtime directly (no rate-limit handling)"
        "$BIN" "$@"
    fi
    ec=$?
    RUN_ENDED=$(date +%s)

    if [ -n "$AUTO_CONFIRM_PID" ]; then
        kill "$AUTO_CONFIRM_PID" 2>/dev/null
        wait "$AUTO_CONFIRM_PID" 2>/dev/null
    fi
    if [ -n "$NUDGE_PID" ]; then
        kill "$NUDGE_PID" 2>/dev/null
        wait "$NUDGE_PID" 2>/dev/null
    fi

    # Self-heal a session-id collision (see the resume-probe above). If we
    # launched with `--resume` and the runtime exited non-zero, the matched
    # jsonl may belong to a different folder (the probe globs every cwd
    # slug). Retry once, immediately, forcing a fresh cwd-scoped
    # `--session-id` rather than respawning into a resume that can never
    # succeed here. One-shot (cleared just below), so a later restart resumes
    # the now-local session normally. On a genuine crash of a real local
    # session the forced `--session-id` errors and we fall through to the
    # normal restart — one harmless extra attempt, never a loop. Gated on
    # the claude-code runtime: RESUMED is shared with the other
    # resume-capable runtimes, whose recovery is the fast-fail counter
    # below, not a claude flag swap.
    if [ "$RUNTIME" = "claude-code" ] && [ "$ec" -ne 0 ] && [ "${RESUMED:-0}" = 1 ] && [ "${FORCE_FRESH_SESSION:-0}" != 1 ]; then
        log "resume failed (ec=$ec); retrying once with a fresh cwd-scoped --session-id"
        FORCE_FRESH_SESSION=1
        continue
    fi
    FORCE_FRESH_SESSION=0

    # A corrupt session store can make the resume path die instantly on
    # every boot — the resume probe re-matches the same state each
    # time, so the restart loop would spin forever. Self-heal in the
    # same spirit as the claude branch: after 3 consecutive resume-path
    # exits that lasted under 60s, move the runtime's session store
    # aside so the next boot falls through to a fresh spawn. A
    # long-lived session or a fresh-path boot resets the counter.
    if [ "${RESUMED:-0}" = 1 ] && [ $((RUN_ENDED - RUN_STARTED)) -lt 60 ]; then
        RESUME_FAST_FAILS=$((RESUME_FAST_FAILS + 1))
        if [ "$RESUME_FAST_FAILS" -ge 3 ]; then
            case "$RUNTIME" in
                codex)
                    rm -rf "$CODEX_HOME/sessions.crash-bak"
                    mv "$CODEX_HOME/sessions" "$CODEX_HOME/sessions.crash-bak" 2>/dev/null
                    log "3 fast codex resume failures — moved $CODEX_HOME/sessions to $CODEX_HOME/sessions.crash-bak; next boot is fresh"
                    ;;
                opencode)
                    # The sqlite db moves together with its -shm/-wal
                    # sidecars (absent ones just no-op under 2>/dev/null).
                    OC_HOME=$(dirname "$OPENCODE_DB")
                    rm -rf "$OC_HOME/db.crash-bak"
                    mkdir -p "$OC_HOME/db.crash-bak"
                    mv "$OPENCODE_DB" "$OPENCODE_DB-shm" "$OPENCODE_DB-wal" "$OC_HOME/db.crash-bak/" 2>/dev/null
                    log "3 fast opencode resume failures — moved $OPENCODE_DB (+sidecars) to $OC_HOME/db.crash-bak/; next boot is fresh"
                    ;;
            esac
            RESUME_FAST_FAILS=0
        fi
    else
        RESUME_FAST_FAILS=0
    fi

    log "runtime exited ec=$ec — restarting in 5s"
    sleep 5
done
