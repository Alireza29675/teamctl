use super::*;

/// Run `claude /teamctl:init` to hand off to the LLM-led setup skill.
/// Inherits stdio so the operator sees the conversation in their
/// current terminal. If `claude` is not on PATH, surface a clear
/// error pointing at the install path.
pub(super) fn exec_guided() -> Result<()> {
    let status = Command::new("claude")
        .arg("/teamctl:init")
        .status()
        .with_context(|| {
            "failed to launch `claude` — is Claude Code installed and on PATH? See \
             https://code.claude.com/docs"
        })?;
    if !status.success() {
        bail!(
            "`claude /teamctl:init` exited with status {status} — see the Claude Code output \
             above for details."
        );
    }
    Ok(())
}

/// One-line label + blurb for a menu slot, for the picker display.
pub(super) fn choice_line(c: &Choice) -> (&'static str, &'static str) {
    match c {
        Choice::Guided => ("Guided", "LLM walks you through setup (opens Claude Code)"),
        Choice::Template(t) => (t.label, t.blurb),
    }
}

/// Picker UX. Shows Ideate & Build / Essentials / Guided / Blank in that
/// order with Ideate & Build as the default (Enter selects it). Returns a
/// `Choice` so the caller can branch to exec-claude vs. file-scaffold
/// paths.
///
/// On Guided selection, the confirm-intent prompt is handled by the
/// caller (so the picker is pure-input → pure-output). That keeps
/// this function testable without piping a `claude` mock.
pub(super) fn choose_template_interactive() -> Result<Choice> {
    eprintln!("Pick a template:");
    for (i, c) in picker_menu().iter().enumerate() {
        let (label, blurb) = choice_line(c);
        eprintln!("  {}) {:<16} — {}", i + 1, label, blurb);
    }
    eprint!("Choice [1]: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(parse_picker_input(line.trim()))
}

/// Pure-function picker dispatch over `picker_menu()` display order
/// (1 = Ideate & Build, 2 = Essentials, 3 = Guided, 4 = Blank). Empty
/// input → slot 1 (default-on-Enter = the flagship). Anything unparseable
/// or out-of-range also falls back to slot 1 so accidental keystrokes
/// land on the most-supported path rather than the bare-tree one.
pub(super) fn parse_picker_input(trimmed: &str) -> Choice {
    let menu = picker_menu();
    if trimmed.is_empty() {
        return menu[0];
    }
    match trimmed.parse::<usize>() {
        Ok(n) if n >= 1 => menu.get(n - 1).copied().unwrap_or(menu[0]),
        _ => menu[0],
    }
}

pub(super) fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [Y/n] ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(answer_is_yes(&line))
}

/// Parse a y/N answer with **Yes** as the empty-input default: a bare
/// Enter proceeds. Pulled out of `confirm` so the default can be unit
/// tested without driving real stdin.
pub(super) fn answer_is_yes(line: &str) -> bool {
    let s = line.trim().to_lowercase();
    s.is_empty() || s == "y" || s == "yes"
}
