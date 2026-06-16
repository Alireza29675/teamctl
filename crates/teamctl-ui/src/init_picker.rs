//! Interactive `teamctl init` picker — a two-pane ratatui screen launched
//! by `teamctl init` (via `teamctl-ui --init-picker`). Branch-first:
//! **Browse a template** vs **Co-design with AI**; choosing Browse opens a
//! list ↔ live-detail view of the on-disk example teams, with a team-shape
//! preview (the reporting tree + per-agent capability counts) drawn from
//! `team_core::preview`.
//!
//! Self-contained on purpose: it shares teamctl-ui's theme + terminal
//! lifecycle but NOT the dashboard `app::run` loop, which is wired to tmux
//! panes / mailbox.db / a file-watcher. This is a small standalone app.
//!
//! The binary entry (`--init-picker` in `main.rs`) renders to **stderr** and
//! prints the chosen key to **stdout**, so `teamctl init` can capture the
//! selection while the UI still shows on the operator's terminal (the fzf
//! pattern). `run_standalone` owns that stderr terminal lifecycle.

use std::io;
use std::panic;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::{Frame, Terminal};

use team_core::compose::{Global, Project};
use team_core::preview::{team_shape, ShapeKind, ShapeRow};

use crate::theme::{detect_capabilities, Capabilities};

/// Artificial fetch delay so the operator sees the lazy/loading UX the real
/// remote store (fast-follow #495) will have — without any network call.
const FAKE_FETCH: Duration = Duration::from_millis(800);
const SPINNER: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];

/// What the picker resolves to. The binary entry maps this to stdout + an
/// exit code that `teamctl init` consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// An example/template key the operator chose (Browse → Enter).
    Selected(String),
    /// The "Co-design with AI" branch — `teamctl init` runs the guided flow.
    CoDesign,
    /// Esc / `q` without a choice.
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Branch,
    Browse,
}

/// Aggregate per-team capability counts, for the detail headline.
#[derive(Default, Clone, Copy)]
pub struct Counts {
    pub agents: usize,
    pub subagents: usize,
    pub skills: usize,
    pub hooks: usize,
    pub mcps: usize,
}

/// One browsable catalog entry — an example team, already parsed into its
/// reporting shape so the detail pane is a pure render.
pub struct Entry {
    pub key: String,
    pub name: String,
    pub blurb: String,
    pub rows: Vec<ShapeRow>,
    pub counts: Counts,
}

/// The picker's full state. Deterministic and terminal-free, so
/// `render_to_buffer` can snapshot any screen without a real terminal.
pub struct PickerState {
    caps: Capabilities,
    screen: Screen,
    branch_idx: usize, // 0 = Browse, 1 = Co-design
    entries: Vec<Entry>,
    list_idx: usize,
    loading: bool, // faked-fetch spinner gate (Browse first paint)
    spinner: usize,
}

impl PickerState {
    /// Build a picker over the given catalog entries, starting on the
    /// branch screen.
    pub fn new(caps: Capabilities, entries: Vec<Entry>) -> Self {
        Self {
            caps,
            screen: Screen::Branch,
            branch_idx: 0,
            entries,
            list_idx: 0,
            loading: false,
            spinner: 0,
        }
    }

    /// Load the catalog from a directory of example teams (each an
    /// `<name>/.team/team-compose.yaml` tree) and build a fresh picker.
    pub fn load(caps: Capabilities, examples_dir: &Path) -> Self {
        Self::new(caps, load_entries(examples_dir))
    }

    /// Jump straight to the Browse screen (used by snapshot tests).
    pub fn browsing(mut self) -> Self {
        self.screen = Screen::Browse;
        self.loading = false;
        self
    }

    /// Handle one key; returns `Some(outcome)` when the picker should exit.
    fn on_key(&mut self, code: KeyCode) -> Option<Outcome> {
        match self.screen {
            Screen::Branch => match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.branch_idx = self.branch_idx.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.branch_idx = (self.branch_idx + 1).min(1);
                    None
                }
                KeyCode::Enter => {
                    if self.branch_idx == 0 {
                        self.screen = Screen::Browse;
                        self.loading = true;
                        self.spinner = 0;
                        None
                    } else {
                        Some(Outcome::CoDesign)
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => Some(Outcome::Cancelled),
                _ => None,
            },
            Screen::Browse => {
                if self.loading {
                    // Only let the operator back out while the (faked) fetch
                    // is in flight; nav/select waits for the list to land.
                    return match code {
                        KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                            self.screen = Screen::Branch;
                            self.loading = false;
                            None
                        }
                        _ => None,
                    };
                }
                match code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.list_idx = self.list_idx.saturating_sub(1);
                        None
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if self.list_idx + 1 < self.entries.len() {
                            self.list_idx += 1;
                        }
                        None
                    }
                    KeyCode::Enter => self
                        .entries
                        .get(self.list_idx)
                        .map(|e| Outcome::Selected(e.key.clone())),
                    KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                        self.screen = Screen::Branch;
                        None
                    }
                    KeyCode::Char('q') => Some(Outcome::Cancelled),
                    _ => None,
                }
            }
        }
    }
}

/// Render the current screen into `buf` — the single rendering entry point,
/// shared by the live loop (`draw`) and the snapshot helper.
fn render(state: &PickerState, area: Rect, buf: &mut Buffer) {
    match state.screen {
        Screen::Branch => render_branch(state, area, buf),
        Screen::Browse => render_browse(state, area, buf),
    }
}

/// Snapshot helper: render `state` into a fresh `width × height` buffer.
pub fn render_to_buffer(state: &PickerState, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render(state, area, &mut buf);
    buf
}

fn draw(frame: &mut Frame, state: &PickerState) {
    render(state, frame.area(), frame.buffer_mut());
}

fn render_branch(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let accent = Style::default()
        .fg(state.caps.accent())
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(state.caps.muted());

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // top spacer
            Constraint::Length(1), // title
            Constraint::Length(1), // gap
            Constraint::Length(1), // option 1
            Constraint::Length(1), // option 2
            Constraint::Length(1), // gap
            Constraint::Length(1), // hint
            Constraint::Min(0),    // bottom spacer
        ])
        .split(area);

    Paragraph::new("Create a new team")
        .style(accent)
        .alignment(Alignment::Center)
        .render(rows[1], buf);

    let option = |idx: usize, label: &str, desc: &str| -> Line<'static> {
        let selected = state.branch_idx == idx;
        let marker = if selected { "▸ " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(state.caps.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label}"), style),
            Span::styled(
                format!("   {desc}"),
                Style::default().fg(state.caps.muted()),
            ),
        ])
    };

    Paragraph::new(option(0, "Browse a template", "pick from ready-made teams"))
        .alignment(Alignment::Center)
        .render(rows[3], buf);
    Paragraph::new(option(
        1,
        "Co-design with AI",
        "let Claude Code shape it with you",
    ))
    .alignment(Alignment::Center)
    .render(rows[4], buf);

    Paragraph::new("↑/↓ select · Enter choose · Esc cancel")
        .style(muted)
        .alignment(Alignment::Center)
        .render(rows[6], buf);
}

fn render_browse(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(0)])
        .split(vchunks[0]);

    render_list(state, panes[0], buf);
    render_detail(state, panes[1], buf);

    Paragraph::new("↑/↓ select · Enter choose · Esc back")
        .style(Style::default().fg(state.caps.muted()))
        .alignment(Alignment::Center)
        .render(vchunks[1], buf);
}

fn render_list(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .title(" Templates ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.caps.muted()));
    let inner = block.inner(area);
    block.render(area, buf);

    if state.loading {
        Paragraph::new(format!("{} Fetching templates…", spinner_glyph(state)))
            .style(Style::default().fg(state.caps.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    if state.entries.is_empty() {
        Paragraph::new("(no templates found)")
            .style(Style::default().fg(state.caps.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }

    let lines: Vec<Line<'_>> = state
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let selected = i == state.list_idx;
            let marker = if selected { "▸ " } else { "  " };
            let style = if selected {
                Style::default()
                    .fg(state.caps.accent())
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Line::styled(format!("{marker}{}", e.name), style)
        })
        .collect();
    Paragraph::new(lines).render(inner, buf);
}

fn render_detail(state: &PickerState, area: Rect, buf: &mut Buffer) {
    let entry = state.entries.get(state.list_idx);
    let title = entry.map(|e| format!(" {} ", e.name)).unwrap_or_default();
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(state.caps.accent()));
    let inner = block.inner(area);
    block.render(area, buf);

    if state.loading {
        Paragraph::new(format!("{} loading…", spinner_glyph(state)))
            .style(Style::default().fg(state.caps.muted()))
            .alignment(Alignment::Center)
            .render(inner, buf);
        return;
    }
    let Some(entry) = entry else { return };

    let mut lines: Vec<Line<'static>> = Vec::new();
    if !entry.blurb.is_empty() {
        lines.push(Line::styled(
            entry.blurb.clone(),
            Style::default().fg(state.caps.muted()),
        ));
        lines.push(Line::raw(""));
    }
    let c = entry.counts;
    lines.push(Line::styled(
        format!(
            "{} · {} · {} · {} · {}",
            plural(c.agents, "agent"),
            plural(c.subagents, "sub-agent"),
            plural(c.skills, "skill"),
            plural(c.hooks, "hook"),
            plural(c.mcps, "mcp"),
        ),
        Style::default().fg(state.caps.muted()),
    ));
    lines.push(Line::raw(""));
    lines.extend(shape_to_lines(&entry.rows, state.caps));

    // No wrap: the tree lines are structured, so on a narrow terminal we
    // truncate at the pane edge rather than wrap a descriptor onto a second
    // line without its tree prefix (which reads as broken).
    Paragraph::new(lines).render(inner, buf);
}

/// Turn the front-end-agnostic `ShapeRow`s into styled tree lines, mapping
/// `is_last` back to the `└──`/`├──` connectors and drawing continuation
/// columns for ancestor levels. Mirrors init.rs's box-drawing tree.
fn shape_to_lines(rows: &[ShapeRow], caps: Capabilities) -> Vec<Line<'static>> {
    let mut last_at: Vec<bool> = Vec::new();
    let mut out: Vec<Line<'static>> = Vec::new();
    for r in rows {
        if matches!(r.kind, ShapeKind::Root) {
            out.push(Line::styled(
                r.label.clone(),
                Style::default()
                    .fg(caps.accent())
                    .add_modifier(Modifier::BOLD),
            ));
            last_at = vec![true];
            continue;
        }
        let depth = r.depth as usize;
        // Continuation columns for ancestor depths 1..depth.
        let mut prefix = String::new();
        for d in 1..depth {
            let ancestor_last = last_at.get(d).copied().unwrap_or(true);
            prefix.push_str(if ancestor_last { "    " } else { "│   " });
        }
        let connector = if r.is_last {
            "└── "
        } else {
            "├── "
        };
        if last_at.len() <= depth {
            last_at.resize(depth + 1, true);
        }
        last_at[depth] = r.is_last;

        let mut spans = vec![
            Span::styled(
                format!("{prefix}{connector}"),
                Style::default().fg(caps.muted()),
            ),
            Span::styled(r.label.clone(), Style::default().fg(caps.accent())),
        ];
        if !r.descriptor.is_empty() {
            spans.push(Span::styled(
                format!("  {}", r.descriptor),
                Style::default().fg(caps.muted()),
            ));
        }
        out.push(Line::from(spans));
    }
    out
}

fn spinner_glyph(state: &PickerState) -> &'static str {
    SPINNER[state.spinner % SPINNER.len()]
}

/// Drive the picker against `terminal` until the operator chooses or
/// cancels. Polls on a short timeout so the faked-fetch spinner animates
/// and the loading deadline resolves.
fn run<B: Backend>(terminal: &mut Terminal<B>, mut state: PickerState) -> Result<Outcome> {
    let mut fetch_deadline: Option<Instant> = None;
    loop {
        if state.loading && fetch_deadline.is_none() {
            fetch_deadline = Some(Instant::now() + FAKE_FETCH);
        }
        if let Some(dl) = fetch_deadline {
            if Instant::now() >= dl {
                state.loading = false;
                fetch_deadline = None;
            }
        }

        terminal.draw(|f| draw(f, &state))?;

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if let Some(outcome) = state.on_key(key.code) {
                    return Ok(outcome);
                }
                // Leaving Browse cancels any in-flight fetch timer.
                if !state.loading {
                    fetch_deadline = None;
                }
            }
        } else if state.loading {
            state.spinner = state.spinner.wrapping_add(1);
        }
    }
}

/// Binary entry: own the **stderr** terminal lifecycle (so stdout stays
/// clean for the selection token), run the picker, and restore the terminal
/// on every exit path including panics.
pub fn run_standalone(examples_dir: &Path) -> Result<Outcome> {
    let caps = detect_capabilities();
    let state = PickerState::load(caps, examples_dir);

    install_panic_hook();
    enter_terminal()?;
    // Everything past raw-mode-on runs inside this closure so the
    // unconditional `leave_terminal()` below restores the terminal on
    // EVERY exit path — a `run` error, a `Terminal::new` failure, or a
    // clean return. Panics are caught by the hook, which calls the same
    // infallible teardown.
    let result = (move || {
        let backend = CrosstermBackend::new(io::stderr());
        let mut terminal = Terminal::new(backend)?;
        let outcome = run(&mut terminal, state);
        let _ = terminal.show_cursor();
        outcome
    })();
    leave_terminal();
    result
}

fn enter_terminal() -> Result<()> {
    enable_raw_mode()?;
    // If the alternate-screen step fails, undo raw mode before bailing so
    // the caller never returns with the shell stranded in raw mode.
    if let Err(e) = execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    Ok(())
}

/// Best-effort, unconditional teardown: every step runs regardless of an
/// earlier failure, so `disable_raw_mode()` always fires (the step that
/// actually un-wedges the operator's shell). Safe on any exit path,
/// including the panic hook.
fn leave_terminal() {
    let _ = execute!(io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        leave_terminal();
        original(info);
    }));
}

// ── catalog loading ────────────────────────────────────────────────

/// Scan `examples_dir` for `<name>/.team/team-compose.yaml` teams, parse
/// each into its reporting shape, and return the catalog (dir-name sorted).
/// Unparseable or agent-less examples are skipped — the picker never fails
/// on a bad example.
fn load_entries(examples_dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(examples_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter().filter_map(|d| load_entry(d)).collect()
}

fn load_entry(dir: &Path) -> Option<Entry> {
    let team_dir = dir.join(".team");
    let compose_str = std::fs::read_to_string(team_dir.join("team-compose.yaml")).ok()?;
    let global: Global = serde_yaml::from_str(&compose_str).ok()?;

    let mut projects: Vec<Project> = Vec::new();
    for p in &global.projects {
        if let Ok(s) = std::fs::read_to_string(team_dir.join(&p.file)) {
            if let Ok(project) = serde_yaml::from_str::<Project>(&s) {
                projects.push(project);
            }
        }
    }
    if projects.is_empty() {
        return None;
    }

    let refs: Vec<&Project> = projects.iter().collect();
    let rows = team_shape(&refs);
    let counts = counts_of(&projects);
    let key = dir.file_name()?.to_string_lossy().into_owned();
    let name = humanize(&key);
    let blurb = first_comment_blurb(&compose_str).unwrap_or_default();

    Some(Entry {
        key,
        name,
        blurb,
        rows,
        counts,
    })
}

fn counts_of(projects: &[Project]) -> Counts {
    let mut c = Counts::default();
    for p in projects {
        for agent in p.managers.values().chain(p.workers.values()) {
            c.agents += 1;
            c.subagents += agent.subagents.len();
            c.skills += agent.skills.len();
            c.hooks += agent.hooks.len();
            c.mcps += agent.mcps.len();
        }
    }
    c
}

/// First comment line of a compose file, as a one-line blurb. Prefers the
/// text after an em-dash (`# product-team — does X` → `does X`).
fn first_comment_blurb(compose: &str) -> Option<String> {
    for line in compose.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim();
            if rest.is_empty() {
                continue;
            }
            let blurb = rest.split('—').nth(1).map(str::trim).unwrap_or(rest);
            return Some(blurb.to_string());
        }
        if !trimmed.is_empty() {
            break;
        }
    }
    None
}

/// `product-team` → `Product Team`.
fn humanize(key: &str) -> String {
    key.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pluralize a count for the detail headline: `1 hook`, `2 hooks`, `0 mcps`.
fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize_splits_and_titlecases() {
        assert_eq!(humanize("product-team"), "Product Team");
        assert_eq!(humanize("oss_maintainer"), "Oss Maintainer");
        assert_eq!(humanize("blank"), "Blank");
    }

    #[test]
    fn blurb_prefers_text_after_em_dash() {
        let compose = "# product-team — discovery while a team builds.\nversion: \"2.0.0\"\n";
        assert_eq!(
            first_comment_blurb(compose).as_deref(),
            Some("discovery while a team builds.")
        );
    }

    #[test]
    fn blurb_falls_back_to_whole_comment() {
        assert_eq!(
            first_comment_blurb("# a tidy little team\n").as_deref(),
            Some("a tidy little team")
        );
        assert_eq!(first_comment_blurb("version: 2\n"), None);
    }

    fn buf_to_string(buf: &Buffer) -> String {
        let a = buf.area();
        let mut out = String::new();
        for y in 0..a.height {
            for x in 0..a.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn mono() -> Capabilities {
        Capabilities {
            color: crate::theme::ColorMode::Monochrome,
        }
    }

    /// A deterministic two-manager / one-worker team for the browse snapshot.
    fn fixture_entry() -> Entry {
        Entry {
            key: "product-team".into(),
            name: "Product Team".into(),
            blurb: "product discovery while an engineering team builds".into(),
            rows: vec![
                ShapeRow {
                    depth: 0,
                    kind: ShapeKind::Root,
                    label: "You".into(),
                    descriptor: String::new(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "Product Manager".into(),
                    descriptor: "Claude Code · Opus 4.8 · 2×a 0×s 0×h 0×m".into(),
                    is_last: false,
                },
                ShapeRow {
                    depth: 2,
                    kind: ShapeKind::Worker,
                    label: "Engineer (Claude)".into(),
                    descriptor: "Claude Code · Sonnet 4.6 · 6×a 0×s 1×h 0×m".into(),
                    is_last: true,
                },
                ShapeRow {
                    depth: 1,
                    kind: ShapeKind::Manager,
                    label: "Engineering Manager".into(),
                    descriptor: "Claude Code · Opus 4.8 · 2×a 0×s 0×h 0×m".into(),
                    is_last: true,
                },
            ],
            counts: Counts {
                agents: 4,
                subagents: 10,
                skills: 0,
                hooks: 1,
                mcps: 0,
            },
        }
    }

    #[test]
    fn branch_screen_snapshot() {
        let state = PickerState::new(mono(), vec![]);
        insta::assert_snapshot!(buf_to_string(&render_to_buffer(&state, 100, 16)));
    }

    #[test]
    fn browse_screen_snapshot() {
        let state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        insta::assert_snapshot!(buf_to_string(&render_to_buffer(&state, 100, 20)));
    }

    #[test]
    fn loading_screen_shows_spinner() {
        // Browse before the faked fetch resolves: a spinner, no list yet.
        let mut state = PickerState::new(mono(), vec![fixture_entry()]);
        state.on_key(crossterm::event::KeyCode::Enter); // Branch → Browse (loading)
        let out = buf_to_string(&render_to_buffer(&state, 100, 16));
        assert!(out.contains("Fetching templates"), "spinner state:\n{out}");
    }

    #[test]
    fn enter_on_browse_selects_current_entry() {
        let mut state = PickerState::new(mono(), vec![fixture_entry()]).browsing();
        assert_eq!(
            state.on_key(crossterm::event::KeyCode::Enter),
            Some(Outcome::Selected("product-team".into()))
        );
    }

    #[test]
    fn co_design_branch_returns_codesign() {
        let mut state = PickerState::new(mono(), vec![]);
        state.on_key(crossterm::event::KeyCode::Down); // → Co-design
        assert_eq!(
            state.on_key(crossterm::event::KeyCode::Enter),
            Some(Outcome::CoDesign)
        );
    }

    #[test]
    fn esc_on_branch_cancels() {
        let mut state = PickerState::new(mono(), vec![]);
        assert_eq!(
            state.on_key(crossterm::event::KeyCode::Esc),
            Some(Outcome::Cancelled)
        );
    }
}
