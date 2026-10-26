//! Explicit agent console and on-demand workspace shell, scoped to a defined agent.
use super::*;
use axum::extract::{Path, Query, State};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    sync::Mutex,
};
use team_core::{
    compose::Compose,
    supervisor::{AgentSpec, Supervisor, TmuxSupervisor},
};

pub(super) struct Shell {
    output: Arc<Mutex<VecDeque<u8>>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}
impl Drop for Shell {
    fn drop(&mut self) {
        // portable-pty signals a saved PID before waiting. A shell already
        // reaped by try_wait may have had that PID reused by another process.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
pub(super) type Shells = Mutex<HashMap<String, Shell>>;

#[derive(Default, Deserialize)]
pub(super) struct Mode {
    #[serde(default)]
    mode: String,
}
#[derive(Deserialize)]
pub(super) struct Input {
    #[serde(default)]
    input: String,
    key: Option<String>,
    #[serde(default)]
    connect: bool,
    #[serde(default)]
    mode: String,
}
fn spec(state: &Workspace, id: &str) -> ApiResult<(Compose, AgentSpec)> {
    let compose = Compose::load_read_only(&state.root).map_err(ApiError::from)?;
    let h = compose
        .agents()
        .find(|h| h.id() == id)
        .ok_or_else(|| ApiError::missing("Unknown agent"))?;
    let spec = AgentSpec::from_handle(h, &state.root, &compose.global.supervisor.tmux_prefix);
    Ok((compose, spec))
}
pub(super) async fn read(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Query(q): Query<Mode>,
) -> ApiResult<Json<Value>> {
    blocking(move || {
        let (_,spec)=spec(&state,&id)?;
        if q.mode=="shell" {
            let mut shells=state.shells.lock().map_err(|_|ApiError::bad("Shell is unavailable"))?;
            return Ok(Json(if let Some(shell)=shells.get_mut(&id){let alive=shell.child.try_wait().map_err(anyhow::Error::from)?.is_none();let bytes=shell.output.lock().map_err(|_|ApiError::bad("Shell is unavailable"))?;json!({"output":strip_ansi(&String::from_utf8_lossy(&bytes.iter().copied().collect::<Vec<_>>()).replace('\r',"")),"connected":alive,"interactive":alive,"mode":"shell"})}else{json!({"output":"Connect to open a workspace shell.","connected":false,"mode":"shell"})}));
        }
        let target=format!("={}:0.0",spec.tmux_session.replace('.',"_"));
        let status=TmuxSupervisor.state(&spec).unwrap_or(team_core::supervisor::AgentState::Unknown);
        if status==team_core::supervisor::AgentState::Running {
            let output=std::process::Command::new("tmux").args(["capture-pane","-p","-t",&target,"-S","-250"]).output().map_err(anyhow::Error::from)?;
            return Ok(Json(json!({"output":String::from_utf8_lossy(&output.stdout),"connected":output.status.success(),"interactive":true,"mode":"agent"})));
        }
        let path=team_core::supervisor::native_log_path(&spec);
        let mut output=String::new();
        if let Ok(mut file)=std::fs::File::open(path){use std::io::{Seek,SeekFrom};let len=file.metadata().map_err(anyhow::Error::from)?.len();let _=file.seek(SeekFrom::Start(len.saturating_sub(65536)));let mut bytes=Vec::new();file.take(65536).read_to_end(&mut bytes).map_err(anyhow::Error::from)?;output=String::from_utf8_lossy(&bytes).into_owned();}
        if output.is_empty(){output="No agent console output yet. Start agents with teamctl up. Native agents accept messages in Chat.".into();}
        Ok(Json(json!({"output":strip_ansi(&output),"connected":false,"interactive":false,"mode":"agent"})))
    }).await
}
pub(super) async fn write(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(input): Json<Input>,
) -> ApiResult<Json<Value>> {
    if input.input.len() > 8192 {
        return Err(ApiError::bad("Console input exceeds 8 KiB"));
    }
    let key = match input.key.as_deref() {
        None => None,
        Some("Enter") => Some("Enter"),
        Some("Escape") => Some("Escape"),
        Some("C-c") => Some("C-c"),
        Some(_) => return Err(ApiError::bad("Unsupported console key")),
    };
    blocking(move || {
        let (compose,spec)=spec(&state,&id)?;
        if input.mode=="shell" {
            let mut shells=state.shells.lock().map_err(|_|ApiError::bad("Shell is unavailable"))?;
            if input.connect {
                let exited=shells.get_mut(&id).map(|shell|shell.child.try_wait()).transpose().map_err(anyhow::Error::from)?.flatten().is_some();
                if exited {shells.remove(&id);}
                if !shells.contains_key(&id){shells.insert(id.clone(),open_shell(&compose,&id)?);}
            }
            let shell=shells.get_mut(&id).ok_or_else(||ApiError::bad("Connect the workspace shell first"))?;
            if shell.child.try_wait().map_err(anyhow::Error::from)?.is_some(){return Err(ApiError(StatusCode::CONFLICT,"This shell has exited; connect again to open a new shell".into()));}
            shell.writer.write_all(input.input.as_bytes()).map_err(anyhow::Error::from)?;
            if let Some(key)=key {shell.writer.write_all(match key{"Enter"=>b"\r","Escape"=>b"\x1b",_=>b"\x03"}).map_err(anyhow::Error::from)?;}
            shell.writer.flush().map_err(anyhow::Error::from)?;
            return Ok(Json(json!({"ok":true,"mode":"shell"})));
        }
        let target=format!("={}:0.0",spec.tmux_session.replace('.',"_"));
        if TmuxSupervisor.state(&spec).map_err(ApiError::from)?!=team_core::supervisor::AgentState::Running{return Err(ApiError(StatusCode::CONFLICT,"This agent has no interactive terminal. Send a Chat message or connect a workspace shell.".into()));}
        if !input.input.is_empty(){let status=std::process::Command::new("tmux").args(["send-keys","-t",&target,"-l","--",&input.input]).status().map_err(anyhow::Error::from)?;if !status.success(){return Err(ApiError::bad("Agent terminal is unavailable"));}}
        if let Some(key)=key {let status=std::process::Command::new("tmux").args(["send-keys","-t",&target,key]).status().map_err(anyhow::Error::from)?;if !status.success(){return Err(ApiError::bad("Agent terminal is unavailable"));}}
        Ok(Json(json!({"ok":true})))
    }).await
}
fn open_shell(compose: &Compose, id: &str) -> ApiResult<Shell> {
    let h = compose
        .agents()
        .find(|h| h.id() == id)
        .ok_or_else(|| ApiError::missing("Unknown agent"))?;
    // Container-backed agents must not accidentally open an unrestricted host shell.
    let serialized = serde_json::to_value(h.spec).map_err(anyhow::Error::from)?;
    if serialized
        .pointer("/execution/sandbox/type")
        .and_then(Value::as_str)
        .is_some_and(|v| v != "host")
    {
        return Err(ApiError::bad(
            "Container shells are unavailable between turns; use the agent chat or Docker console",
        ));
    }
    let project = compose
        .projects
        .iter()
        .find(|p| p.project.id == h.project)
        .ok_or_else(|| ApiError::missing("Unknown project"))?;
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 32,
            cols: 110,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(ApiError::from)?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut command = CommandBuilder::new(shell);
    command.cwd(compose.root.join(&project.project.cwd));
    command.env("TERM", "dumb");
    command.env("TEAMCTL_AGENT", id);
    let env = team_core::render::env_path(&compose.root, h.project, h.agent);
    if let Ok(env) = std::fs::read_to_string(env) {
        for line in env.lines().filter(|line| !line.starts_with('#')) {
            if let Some((key, value)) = line.split_once('=') {
                command.env(key, value);
            }
        }
    }
    let child = pty.slave.spawn_command(command).map_err(ApiError::from)?;
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().map_err(ApiError::from)?;
    let writer = pty.master.take_writer().map_err(ApiError::from)?;
    let output = Arc::new(Mutex::new(VecDeque::new()));
    let sink = output.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(count) = reader.read(&mut chunk) {
            if count == 0 {
                break;
            }
            let Ok(mut bytes) = sink.lock() else {
                break;
            };
            bytes.extend(&chunk[..count]);
            while bytes.len() > 65536 {
                bytes.pop_front();
            }
        }
    });
    Ok(Shell {
        output,
        writer,
        child,
        _master: pty.master,
    })
}
fn strip_ansi(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                for next in chars.by_ref() {
                    if next == '\x07' {
                        break;
                    }
                }
            }
        } else if !ch.is_control() || matches!(ch, '\n' | '\t') {
            output.push(ch);
        }
    }
    output
}
