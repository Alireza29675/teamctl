//! Input dispatch for application stages.

use super::{App, Stage};
use crate::approvals::{ApprovalDecider, Decision};
use crate::compose::{EditorAction, MessageSender};
use crate::keysender::{encode_key, KeySender, ScrollDirection};
use crate::mailbox::MailboxSource;
use crate::triptych::{MainLayout, Pane};
use crossterm::event::{Event, KeyCode, KeyEventKind};

pub fn handle_event<D: ApprovalDecider, S: MessageSender, M: MailboxSource, K: KeySender>(
    app: &mut App,
    ev: Event,
    decider: &D,
    sender: &S,
    mailbox_source: &M,
    key_sender: &K,
) {
    if app.stage == Stage::Triptych && app.layout == MainLayout::Chat {
        app.handle_chat_event(ev);
        return;
    }
    use crossterm::event::KeyModifiers;
    if let Event::Key(key) = &ev {
        if key.kind == KeyEventKind::Press {
            let follows_chord = matches!(key.code, KeyCode::Char('q') | KeyCode::Char('o'))
                && key.modifiers.is_empty()
                && app.stage == Stage::Triptych
                && app.mailbox_input_mode.is_none();
            if !follows_chord {
                app.pending_chord = None;
            }
        }
    }
    match ev {
        Event::Key(k) if k.kind == KeyEventKind::Press => match app.stage {
            Stage::Splash => app.dismiss_splash(),
            Stage::Triptych => match k.code {
                // T-131 PR-2: mailbox input-mode interception. When
                // the filter / search input is open, all keys route
                // to the active input buffer; everything else
                // (cursor keys, tab cycle, chord prefixes, even `q`
                // quit) is swallowed so a stray key can't trigger
                // unrelated behavior mid-edit. These MUST come first
                // — placed before the unguarded `Char('q')` quit
                // arm so typing `q` into the filter doesn't quit.
                KeyCode::Enter if app.mailbox_input_mode.is_some() => app.mailbox_input_confirm(),
                KeyCode::Esc if app.mailbox_input_mode.is_some() => app.mailbox_input_cancel(),
                KeyCode::Backspace if app.mailbox_input_mode.is_some() => {
                    app.mailbox_input_pop_char()
                }
                // Char arm is gated on "no modifier or Shift only" so
                // Ctrl / Alt / Meta + Char combos (e.g. the `Ctrl+W`
                // chord prefix, `Ctrl+C`) fall through to the swallow
                // arm below instead of landing literally in the filter
                // buffer — matches standard text-input UX (modifier
                // combos aren't typed as their literal char). qa #335
                // nit 2.
                KeyCode::Char(c)
                    if app.mailbox_input_mode.is_some()
                        && (k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT) =>
                {
                    app.mailbox_input_push_char(c)
                }
                _ if app.mailbox_input_mode.is_some() => {}

                // PR-UI-7 chord-prefix follow-ups MUST be tested
                // before unguarded `Char('q')` / `Char('o')` arms,
                // otherwise the no-modifier `q` quit would shadow
                // the `Ctrl+W q` close-split.
                KeyCode::Char('q') if app.pending_chord == Some(KeyCode::Char('w')) => {
                    app.pending_chord = None;
                    app.close_focused_split();
                }
                KeyCode::Char('o') if app.pending_chord == Some(KeyCode::Char('w')) => {
                    app.pending_chord = None;
                    app.close_other_splits();
                }
                KeyCode::Char('q') if k.modifiers.is_empty() => app.enter_quit_confirm(),
                KeyCode::Char('c') if k.modifiers.is_empty() => app.enter_chat(),
                // PR-UI-4: `a` opens the approvals modal when there's
                // at least one pending row. No-op otherwise so the
                // chord doesn't surprise anyone hammering keys.
                KeyCode::Char('a') => app.enter_approvals_modal(),
                // PR-UI-5: `@` opens DM compose to focused agent.
                // PR-UI-6: `!` now opens the broadcast picker so
                // operators choose which channel to broadcast to,
                // not just the project's `all` wire.
                KeyCode::Char('@') => app.enter_compose_dm_for_focused(),
                KeyCode::Char('!') => app.enter_compose_broadcast_with_picker(),
                // PR-UI-7 chord-prefix: when there's at least one
                // detail split, `Ctrl+W` arms the chord-prefix
                // (the next key dispatches `q` close-split, `o`
                // close-others). Tested BEFORE the wall-layout
                // toggle below so the chord-prefix wins when
                // relevant. Both casings accepted because CapsLock
                // / Shift+Ctrl produce `Char('W')`; armed value is
                // normalised to lowercase so the follow-up arms
                // above can match a single literal.
                KeyCode::Char('w') | KeyCode::Char('W')
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.detail_splits.is_empty() =>
                {
                    app.pending_chord = Some(KeyCode::Char('w'))
                }
                // PR-UI-6: layout toggles. `Ctrl+W` for Wall when
                // there are no splits to chord on; `Ctrl+M` for
                // MailboxFirst (always). Both casings accepted —
                // see the chord-arm comment above.
                KeyCode::Char('w') | KeyCode::Char('W')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.toggle_wall_layout()
                }
                KeyCode::Char('m') | KeyCode::Char('M')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.toggle_mailbox_first_layout()
                }
                // PR-UI-7 splitscreen lift: `Ctrl+|` subdivides
                // vertically, `Ctrl+-` horizontally — vim/tmux
                // operators' muscle memory matches the visual.
                KeyCode::Char('|') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.add_detail_split_vertical()
                }
                KeyCode::Char('-') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.add_detail_split_horizontal()
                }
                // Vim window-motion `Ctrl+H/J/K/L` cycles between
                // splits when there's more than one. Both casings
                // accepted — see the Ctrl+W chord-arm comment above
                // for the CapsLock + Shift+Ctrl rationale.
                KeyCode::Char('h')
                | KeyCode::Char('H')
                | KeyCode::Char('k')
                | KeyCode::Char('K')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.cycle_split_prev()
                }
                KeyCode::Char('l')
                | KeyCode::Char('L')
                | KeyCode::Char('j')
                | KeyCode::Char('J')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.cycle_split_next()
                }
                // PR-UI-6 alias preserved for back-compat: `Ctrl+Q`
                // closes the focused split. PR-UI-7 also wires the
                // proper `Ctrl+W q` chord; both work. Both casings
                // accepted for the same reason as Ctrl+W/M.
                KeyCode::Char('q') | KeyCode::Char('Q')
                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.close_focused_split()
                }
                // T-108: `Ctrl+E` activates stream-keys mode when the
                // detail pane is focused — every subsequent keystroke
                // forwards to the agent's tmux pane. Gated on detail
                // focus so operators in the roster / mailbox don't
                // get pulled into stream-mode by a stray chord. Both
                // casings accepted for the CapsLock/Shift+Ctrl case
                // (same rationale as Ctrl+W/M arms above).
                KeyCode::Char('e') | KeyCode::Char('E')
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && app.focused_pane == Pane::Detail =>
                {
                    app.enter_stream_keys()
                }
                // (chord-prefix follow-ups handled at top of arm
                // before unguarded letter-arms — see top of
                // `Stage::Triptych` match.)
                // PR-UI-7 help + tutorial chords. `?` opens help
                // overlay; `t` reopens tutorial. Both no-op if a
                // modifier is in flight (so `Shift+?` and `Ctrl+T`
                // don't double-bind).
                KeyCode::Char('?')
                    if k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT =>
                {
                    app.enter_help_overlay()
                }
                KeyCode::Char('t') if k.modifiers.is_empty() => app.enter_tutorial(),
                // PR-UI-4: Shift+Tab cycles panes backward. Some
                // terminals send `BackTab`, others send `Tab` with
                // SHIFT — handle both.
                KeyCode::BackTab => app.cycle_focus_back(),
                KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => app.cycle_focus_back(),
                // T-074 bug 6: Tab always cycles pane focus, never
                // mailbox tabs. Previously Tab routed into mailbox
                // tab-cycling when the mailbox pane was focused —
                // this stranded operators inside the mailbox with no
                // discoverable way out (Alireza's exact report). The
                // vim/tmux convention is "Tab moves between panes";
                // honour it across every pane uniformly.
                KeyCode::Tab => app.cycle_focus(),
                // T-124: mailbox sub-navigation uses Left/Right
                // arrows (more discoverable than the prior `[`/`]`
                // chord). Gated on the mailbox pane being focused
                // so the keys stay unsurprising elsewhere — Up/Down
                // remain free to scroll layout-specific lists, and
                // Left/Right have no other binding today.
                KeyCode::Right if app.focused_pane == Pane::Mailbox => app.cycle_mailbox_tab(),
                KeyCode::Left if app.focused_pane == Pane::Mailbox => app.cycle_mailbox_tab_back(),
                // PR-UI-6: in Wall layout, `j`/`k` (and arrows)
                // scroll the tile grid — same vim shape, different
                // surface. In Triptych roster focus they still
                // navigate the roster.
                KeyCode::Up | KeyCode::Char('k') if matches!(app.layout, MainLayout::Wall) => {
                    app.wall_scroll_up()
                }
                KeyCode::Down | KeyCode::Char('j') if matches!(app.layout, MainLayout::Wall) => {
                    app.wall_scroll_down()
                }
                // PR-UI-6: in MailboxFirst, `j`/`k` walk the
                // channel list.
                KeyCode::Up | KeyCode::Char('k')
                    if matches!(app.layout, MainLayout::MailboxFirst) =>
                {
                    app.select_prev_channel()
                }
                KeyCode::Down | KeyCode::Char('j')
                    if matches!(app.layout, MainLayout::MailboxFirst) =>
                {
                    app.select_next_channel()
                }
                // T-131 PR-1: mailbox row navigation when the
                // mailbox pane is focused. j/k mirror Vim; arrows
                // mirror every-day navigation. PageUp/PageDown jump a
                // screen; Home/End jump to ends. Per-tab cursor state
                // lives on `MailboxBuffers` and survives tab switches.
                // NB on layout precedence: the MailboxFirst-layout
                // arms above (j/k → channel walk) match first by
                // arm order and intentionally shadow these in that
                // layout — MailboxFirst's UX is channel-feed-centric,
                // not row-cursor-centric. These arms cover the
                // Triptych layout where mailbox is one focused pane
                // among three.
                KeyCode::Up | KeyCode::Char('k') if app.focused_pane == Pane::Mailbox => {
                    app.mailbox_cursor_up()
                }
                KeyCode::Down | KeyCode::Char('j') if app.focused_pane == Pane::Mailbox => {
                    app.mailbox_cursor_down()
                }
                KeyCode::PageUp if app.focused_pane == Pane::Mailbox => app.mailbox_page_up(),
                KeyCode::PageDown if app.focused_pane == Pane::Mailbox => app.mailbox_page_down(),
                KeyCode::Home if app.focused_pane == Pane::Mailbox => app.mailbox_cursor_home(),
                KeyCode::End if app.focused_pane == Pane::Mailbox => app.mailbox_cursor_end(),
                // T-131 PR-2: `f` opens the sender-substring filter
                // input on the active tab; `/` opens the body-search
                // input. Both gated on Pane::Mailbox so the keys stay
                // unsurprising in other panes. Once open, the
                // input-mode arms at the top of this match own the
                // keystrokes.
                KeyCode::Char('f') if app.focused_pane == Pane::Mailbox => {
                    app.open_mailbox_filter_input()
                }
                KeyCode::Char('/') if app.focused_pane == Pane::Mailbox => {
                    app.open_mailbox_search_input()
                }
                // T-131 PR-3: Enter on a selected mailbox row opens
                // the detail modal — captures the row content at
                // snapshot-at-open (variant (a) locked) and flips
                // Stage to MailboxDetailModal. No-op when
                // visible_indices is empty (`open_…` handles the
                // gate). Placed AFTER the input-mode `Enter`
                // confirm arm at the top of this match — that arm
                // fires when the operator presses Enter from
                // inside an open filter/search, not from cursor
                // selection, so the two are disjoint by mode.
                KeyCode::Enter if app.focused_pane == Pane::Mailbox => {
                    app.open_mailbox_detail_modal()
                }
                // Roster navigation — only when roster is the
                // focused pane. j/k mirror Vim; arrows mirror
                // every-day navigation.
                KeyCode::Up | KeyCode::Char('k') if app.focused_pane == Pane::Roster => {
                    app.select_prev()
                }
                KeyCode::Down | KeyCode::Char('j') if app.focused_pane == Pane::Roster => {
                    app.select_next()
                }
                _ => {}
            },
            Stage::QuitConfirm => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.confirm_quit(),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_quit(),
                _ => {}
            },
            Stage::ApprovalsModal => match k.code {
                // Asymmetric chord shape (T-074 bug 4 fix): approve is
                // the common path so it accepts both `y` and `Y` —
                // matches QuitConfirm's loose convention and the
                // muscle-memory most TUI prompts build. Deny is the
                // destructive side, so it requires deliberate Shift
                // (`N` only); a stray lowercase `n` does nothing.
                // Trades cosmetic chord-symmetry for discoverability
                // on the load-bearing approve flow.
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.apply_decision(decider, Decision::Approve, "")
                }
                KeyCode::Char('N') => app.apply_decision(decider, Decision::Deny, ""),
                KeyCode::Char('j') | KeyCode::Down => app.cycle_approval_next(),
                KeyCode::Char('k') | KeyCode::Up => app.cycle_approval_prev(),
                KeyCode::Esc | KeyCode::Char('q') => app.close_approvals_modal(),
                _ => {}
            },
            Stage::ComposeModal => {
                // PR-UI-6: when the broadcast picker is open the
                // editor doesn't see keys yet — operator first
                // chooses a channel.
                if app.compose_picker_open {
                    match k.code {
                        KeyCode::Down | KeyCode::Char('j') => app.picker_next(),
                        KeyCode::Up | KeyCode::Char('k') => app.picker_prev(),
                        KeyCode::Enter => app.picker_confirm(),
                        // PR-UI-6 fixup (Q6, dev2 review): Esc
                        // dismisses the picker overlay only and
                        // returns to the editor with whatever the
                        // operator already typed; the editor's own
                        // Esc-Esc cancel-the-modal flow handles
                        // bailing out of the whole compose. Mirrors
                        // the overlay-vs-modal symmetry vim users
                        // expect.
                        KeyCode::Esc => {
                            app.compose_picker_open = false;
                            app.compose_picker_index = 0;
                        }
                        _ => {}
                    }
                } else if app.compose_attach_input_open {
                    // T-32: path-input overlay. Keys edit the buffer
                    // directly; Enter confirms (appends marker line
                    // to the editor body); Esc cancels back to the
                    // editor. Same overlay-vs-modal symmetry as the
                    // picker — Esc dismisses *the overlay*, not the
                    // whole compose.
                    match k.code {
                        KeyCode::Char(c) => app.compose_attach_buffer.push(c),
                        KeyCode::Backspace => {
                            app.compose_attach_buffer.pop();
                        }
                        KeyCode::Enter => app.confirm_compose_attach_input(),
                        KeyCode::Esc => app.close_compose_attach_input(),
                        _ => {}
                    }
                } else if k.code == KeyCode::Tab {
                    // T-32: Tab opens the path-input overlay. The
                    // editor never sees Tab today (apply_insert
                    // ignores it), so intercepting here doesn't
                    // change the editor's surface.
                    app.open_compose_attach_input();
                } else {
                    // Route every keypress through the editor; the
                    // editor returns Send / Cancel / Continue.
                    match app.compose_editor.apply_key(k) {
                        EditorAction::Continue => {}
                        EditorAction::Send => app.apply_send(sender, mailbox_source),
                        EditorAction::Cancel => app.close_compose_modal(),
                    }
                }
            }
            Stage::HelpOverlay => match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => app.close_help_overlay(),
                _ => {}
            },
            // T-131 PR-3: mailbox detail modal. Esc OR q close;
            // j/k or Up/Down scroll the wrapped body when it
            // overflows the modal height. Other keys are swallowed
            // so a stray chord doesn't accidentally act on the
            // Triptych rendered underneath.
            Stage::MailboxDetailModal => match k.code {
                KeyCode::Esc | KeyCode::Char('q') => app.close_mailbox_detail_modal(),
                KeyCode::Char('j') | KeyCode::Down => app.mailbox_detail_scroll_down(),
                KeyCode::Char('k') | KeyCode::Up => app.mailbox_detail_scroll_up(),
                _ => {}
            },
            Stage::Tutorial => match k.code {
                KeyCode::Esc => app.close_tutorial(),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::Char('p') => app.tutorial_back(),
                _ => app.tutorial_advance(),
            },
            // T-108 / T-374 stream-keys mode. `Ctrl+E` is the only
            // chord we intercept — it's the symmetric exit toggle
            // (the same chord enters the mode from a focused detail
            // pane). Every other key — including `Esc`, `Ctrl+C`,
            // arrow keys, `Enter` — forwards to the agent's tmux
            // pane. The pass-through is intentional: the operator is
            // "effectively attached," so `Esc` reaches Claude Code
            // (it's load-bearing there) and a shell-user's `Ctrl+C`
            // sends SIGINT to the agent rather than bailing out of
            // the mode they just entered.
            Stage::StreamKeys => {
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                let ctrl_shift = k
                    .modifiers
                    .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                if ctrl && matches!(k.code, KeyCode::Char('e') | KeyCode::Char('E')) {
                    app.exit_stream_keys();
                } else if ctrl_shift && matches!(k.code, KeyCode::Up | KeyCode::Down) {
                    // Ctrl+Shift+↑/↓ switches which agent receives
                    // keystrokes without leaving stream-keys mode, so the
                    // banner, the detail pane, and the stream target move
                    // together. (Ctrl+↑/↓ alone is reserved by macOS
                    // Mission Control, so the switch chord adds Shift.)
                    //
                    // Only act when the stream target IS the roster
                    // selection — i.e. no detail split is focused. With a
                    // split focused, `stream_target_session` targets the
                    // split's agent rather than `selected_agent`, so moving
                    // the roster selection would desync the banner/detail
                    // from the session keystrokes actually land in. There
                    // we consume the chord as a no-op instead of misrouting
                    // or leaking a stray `C-S-arrow` to the pane.
                    if app.detail_splits.is_empty() || app.selected_split == 0 {
                        if matches!(k.code, KeyCode::Up) {
                            app.select_prev();
                        } else {
                            app.select_next();
                        }
                    }
                } else if let Some(session) = app.stream_target_session() {
                    if let Some(encoded) = encode_key(k) {
                        // Best-effort: a tmux failure (session
                        // vanished, target pane gone) is silent in
                        // v1; the next refresh tick reflects whatever
                        // the agent's pane actually shows.
                        let _ = key_sender.send(&session, &encoded);
                    }
                } else {
                    // Target session disappeared mid-stream (agent
                    // restarted, team reloaded). Drop back to
                    // Triptych so the operator isn't typing into the
                    // void with no feedback.
                    app.exit_stream_keys();
                }
            }
        },
        Event::Resize(_, _) => {
            // ratatui redraws on the next loop iteration; nothing to do.
        }
        // T-158: mouse-wheel routes by focused pane. Detail forwards
        // each tick to the agent's tmux pane as a copy-mode scroll —
        // wheel-up enters copy-mode and walks history, wheel-down
        // walks back toward live. Roster steps the agent selection
        // (same step as `j`/`k`). T-131 PR-1 wires Mailbox to step
        // the per-tab row cursor (same step as `j`/`k`).
        // Stages other than Triptych ignore mouse input — modal
        // overlays (compose, approvals, picker, help) own the screen
        // and shouldn't get a surprise scroll routed past them.
        Event::Mouse(m) if matches!(app.stage, Stage::Triptych) => {
            use crossterm::event::MouseEventKind;
            let direction = match m.kind {
                MouseEventKind::ScrollUp => Some(ScrollDirection::Up),
                MouseEventKind::ScrollDown => Some(ScrollDirection::Down),
                _ => None,
            };
            if let Some(dir) = direction {
                match app.focused_pane {
                    Pane::Detail => {
                        if let Some(session) = app.stream_target_session() {
                            // Best-effort, same convention as
                            // stream-keys: tmux failure (session
                            // vanished) is silent; the next refresh
                            // reflects reality.
                            let _ = key_sender.scroll(&session, dir);
                        }
                    }
                    Pane::Roster => match dir {
                        ScrollDirection::Up => app.select_prev(),
                        ScrollDirection::Down => app.select_next(),
                    },
                    Pane::Mailbox => match dir {
                        ScrollDirection::Up => app.mailbox_cursor_up(),
                        ScrollDirection::Down => app.mailbox_cursor_down(),
                    },
                }
            }
        }
        _ => {}
    }
}
