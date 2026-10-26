use super::{config::Spec, process::Process};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use team_core::chat::protocol::{AdapterFrame, HostFrame, Mode, OutboundMessage, VERSION};
use team_core::chat::{self, InboundMessage, Repository};
use team_core::compose::Compose;

pub fn run(compose: &Compose, spec: &Spec, mode: Mode) -> Result<()> {
    run_with_context(compose, spec, mode, false, None)
}

pub fn run_with_context(
    compose: &Compose,
    spec: &Spec,
    mode: Mode,
    identify: bool,
    context_file: Option<&std::path::Path>,
) -> Result<()> {
    let carrier = context_file
        .map(|path| super::host::read(path, compose, spec))
        .transpose()?;
    let state = match &carrier {
        Some(context) => context.state_dir.clone(),
        None => super::config::state_dir(&compose.root, &spec.name)?,
    };
    super::config::ensure_state_scope(compose, &state)?;
    super::config::private_dir(&state)?;
    let identity_lock = team_core::registry::lifecycle_lock()?;
    super::super::up::guard_no_name_collision(compose, None)?;
    if let Some(applied) = super::lifecycle::load(&compose.root)?.get(&spec.name) {
        anyhow::ensure!(
            applied.agent == spec.config.agent,
            "adapter target changed while its prior applied identity remains; stop this instance or reconcile both targets before running it"
        );
    }
    let owner = super::host::Lease::acquire(compose, &spec.name, &spec.config.agent, &state)?;
    super::super::up::reserve_in_registry(compose, None)?;
    drop(identity_lock);
    let signals = Signals::new()?;
    let (command, args) = if spec.kind == "whatsapp" {
        super::assets::command(&compose.root)?
    } else {
        (
            spec.config.command.clone().context("adapter command")?,
            spec.config.args.clone(),
        )
    };
    let command = if command.contains(std::path::MAIN_SEPARATOR)
        && !std::path::Path::new(&command).is_absolute()
    {
        compose.root.join(command).to_string_lossy().into_owned()
    } else {
        command
    };
    let mut env = BTreeMap::new();
    for (key, source) in &spec.config.env {
        env.insert(
            key.clone(),
            match &carrier {
                Some(context) => context
                    .env
                    .get(source)
                    .cloned()
                    .context("private adapter credential mapping is missing")?,
                None => std::env::var(source).map_err(|_| {
                    anyhow::anyhow!("adapter credential environment {source} is unset or not UTF-8")
                })?,
            },
        );
    }
    let database = compose.root.join(&compose.global.broker.path);
    anyhow::ensure!(
        identify || mode == Mode::Pair || database.is_file(),
        "adapter broker is not initialized; run `teamctl up` first"
    );
    let conn = chat::open(database)?;
    if mode == Mode::Run && !identify {
        let registered: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id=?1)",
            [&spec.config.agent],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            registered,
            "adapter target is not registered in this broker; run `teamctl up` first"
        );
    }
    let repo = Repository::new(&conn);
    let mut process = Process::spawn(&command, &args, &compose.root, &env)?;
    process.send(&HostFrame::Initialize {
        version: VERSION,
        instance_id: spec.name.clone(),
        state_dir: state.to_string_lossy().into_owned(),
        mode,
        config: spec.config.options.clone(),
    })?;
    let worker = format!(
        "{}-{}",
        std::process::id(),
        super::super::chat::request_id()
    );
    let stop = signals.stop.clone();
    let mut pending: Option<(chat::Delivery, String, Instant)> = None;
    let result = (|| {
        let mut unavailable_since = Some(Instant::now());
        let mut announced = false;
        let mut ready = false;
        let mut capabilities = Vec::<String>::new();
        while !stop.load(Ordering::Relaxed) {
            if owner.stop_requested()? {
                break;
            }
            if unavailable_since.is_some_and(|since| {
                since.elapsed() > Duration::from_secs(spec.config.startup_timeout_secs)
            }) {
                anyhow::bail!("adapter connection timed out; check authentication and run `adapter pair` if needed");
            }
            match process.events.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => match frame? {
                    AdapterFrame::Ready {
                        version,
                        capabilities: declared,
                        ..
                    } => {
                        anyhow::ensure!(version == VERSION, "adapter protocol version mismatch");
                        announced = true;
                        ready = true;
                        unavailable_since = None;
                        capabilities = declared;
                        if !identify {
                            repo.register_capabilities(&spec.name, &capabilities)?;
                        }
                        if mode == Mode::Pair {
                            println!("paired · {}", spec.name);
                            break;
                        }
                    }
                    AdapterFrame::Message {
                        external_id,
                        peer_id,
                        sender_id,
                        text,
                        kind,
                        payload,
                        title,
                        reply_to_external_id,
                    } => {
                        anyhow::ensure!(
                            announced,
                            "adapter emitted a message before its protocol handshake"
                        );
                        if identify {
                            show_identity(&peer_id, &sender_id)?;
                            process.send(&HostFrame::Accepted {
                                peer_id,
                                external_id,
                            })?;
                            continue;
                        }
                        if mode == Mode::Pair || !spec.authorized(&peer_id, &sender_id) {
                            process.send(&HostFrame::Rejected {
                                peer_id,
                                external_id,
                                code: "forbidden_sender".into(),
                            })?;
                            continue;
                        }
                        let result = repo.ingest(InboundMessage {
                            adapter_id: spec.name.clone(),
                            peer_id: peer_id.clone(),
                            sender_id,
                            agent_id: spec.config.agent.clone(),
                            external_id: external_id.clone(),
                            text,
                            kind,
                            payload,
                            title,
                            reply_to_external_id,
                        });
                        match result {
                            Ok(_)=>process.send(&HostFrame::Accepted {peer_id,external_id})?,
                            Err(chat::Error::Database(_))|Err(chat::Error::Io(_))=>anyhow::bail!("adapter inbound persistence failed; source remains queued for retry"),
                            Err(_)=>process.send(&HostFrame::Rejected {peer_id,external_id,code:"invalid_message".into()})?,
                        }
                    }
                    AdapterFrame::Sent {
                        request_id,
                        external_id,
                    } => {
                        if pending.as_ref().is_some_and(|(_, id, _)| id == &request_id) {
                            if let Some((delivery, _, _)) = pending.take() {
                                repo.mark_sent(delivery.id, &worker, external_id.as_deref())?;
                            }
                        }
                    }
                    AdapterFrame::Error {
                        request_id,
                        retryable,
                        ..
                    } => {
                        if pending
                            .as_ref()
                            .is_some_and(|(_, id, _)| request_id.as_deref() == Some(id.as_str()))
                        {
                            if let Some((delivery, _, _)) = pending.take() {
                                repo.mark_failed(
                                    delivery.id,
                                    &worker,
                                    "adapter reported a transport failure",
                                    retryable,
                                )?;
                            }
                        } else if request_id.is_none() {
                            anyhow::ensure!(announced && retryable, "adapter initialization or transport failed; check its private configuration");
                            ready = false;
                            unavailable_since.get_or_insert_with(Instant::now);
                        }
                    }
                    AdapterFrame::Connection { state: connection } => {
                        if connection == chat::protocol::ConnectionState::PairingRequired {
                            anyhow::ensure!(
                                mode == Mode::Pair,
                                "adapter needs pairing; run `teamctl adapter pair {}`",
                                spec.name
                            );
                        }
                        match connection {
                            chat::protocol::ConnectionState::Connected if announced => {
                                ready = true;
                                unavailable_since = None;
                            }
                            chat::protocol::ConnectionState::Disconnected
                            | chat::protocol::ConnectionState::Connecting
                            | chat::protocol::ConnectionState::Starting
                            | chat::protocol::ConnectionState::PairingRequired => {
                                ready = false;
                                unavailable_since.get_or_insert_with(Instant::now);
                            }
                            chat::protocol::ConnectionState::AuthFailed
                            | chat::protocol::ConnectionState::Failed => {
                                anyhow::bail!("adapter authentication or connection failed")
                            }
                            _ => {}
                        }
                    }
                    AdapterFrame::Pairing { qr } => {
                        if mode == Mode::Pair {
                            show_pairing(&qr)?;
                        } else {
                            anyhow::bail!(
                                "adapter needs pairing; run `teamctl adapter pair {}`",
                                spec.name
                            );
                        }
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("adapter protocol reader stopped")
                }
            }
            if process.stopped()? {
                anyhow::bail!("adapter process exited");
            }
            if let Some((delivery, _, time)) = &pending {
                if time.elapsed() > Duration::from_secs(45) {
                    repo.mark_failed(
                        delivery.id,
                        &worker,
                        "adapter delivery response timed out",
                        true,
                    )?;
                    anyhow::bail!("adapter delivery timed out; queued work retained");
                }
            }
            if ready && mode == Mode::Run && !identify && pending.is_none() {
                if let Some(delivery) = repo
                    .claim_deliveries(&spec.name, &worker, now(), 60.0, 1)?
                    .into_iter()
                    .next()
                {
                    if !live_typing(&delivery.message, now()) {
                        repo.mark_failed(
                            delivery.id,
                            &worker,
                            "typing status expired or malformed",
                            false,
                        )?;
                        continue;
                    }
                    if !spec
                        .config
                        .allow_peers
                        .iter()
                        .any(|peer| peer == &delivery.peer_id)
                    {
                        repo.mark_failed(
                            delivery.id,
                            &worker,
                            "destination is no longer allowed",
                            false,
                        )?;
                        continue;
                    }
                    if !capabilities.iter().any(|c| c == &delivery.message.kind) {
                        repo.mark_failed(
                            delivery.id,
                            &worker,
                            "adapter does not support this content kind",
                            false,
                        )?;
                        continue;
                    }
                    let request_id = format!("delivery:{}", delivery.id);
                    pending = Some((delivery, request_id, Instant::now()));
                    if let Some((delivery, request_id, _)) = &pending {
                        process.send(&HostFrame::Send {
                            request_id: request_id.clone(),
                            peer_id: delivery.peer_id.clone(),
                            message: OutboundMessage {
                                text: delivery.message.text.clone(),
                                kind: delivery.message.kind.clone(),
                                payload: delivery.message.payload.clone(),
                                reply_to_external_id: delivery.reply_to_external_id.clone(),
                            },
                        })?;
                    }
                }
            }
        }
        Ok(())
    })();
    if let Some((delivery, _, _)) = pending {
        // Release a delivery immediately on orderly stop/failure. A crashing
        // process still recovers through the repository's bounded lease expiry.
        let release = repo.mark_failed(
            delivery.id,
            &worker,
            "adapter host stopped before transport confirmation",
            true,
        );
        if result.is_ok() {
            release?;
        }
    }
    result
}

fn live_typing(message: &chat::ChatMessage, clock: f64) -> bool {
    if message.kind != "typing" {
        return true;
    }
    message
        .payload
        .as_ref()
        .and_then(|payload| payload.get("expires_at"))
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|expiry| expiry.is_finite() && expiry > clock && expiry <= clock + 10.0)
}

fn show_identity(peer: &str, sender: &str) -> Result<()> {
    use std::io::Write;
    let encoded = |value: &str| -> Result<String> {
        let json = serde_json::to_string(value)?;
        Ok(json
            .chars()
            .map(|c| {
                if matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
                    format!("\\u{:04x}", c as u32)
                } else {
                    c.to_string()
                }
            })
            .collect())
    };
    writeln!(
        std::io::stdout().lock(),
        "peer_id={} sender_id={}",
        encoded(peer)?,
        encoded(sender)?
    )?;
    Ok(())
}

struct Signals {
    stop: Arc<AtomicBool>,
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    fn new() -> Result<Self> {
        let mut signals = Self {
            stop: Arc::new(AtomicBool::new(false)),
            ids: Vec::new(),
        };
        for signal in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
        ] {
            signals
                .ids
                .push(signal_hook::flag::register(signal, signals.stop.clone())?);
        }
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            let stop = signals.stop.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                let mut byte = [0u8; 1];
                while !stop.load(Ordering::Relaxed) {
                    match std::io::stdin().read(&mut byte) {
                        Ok(0) | Err(_) => {
                            stop.store(true, Ordering::Relaxed);
                            break;
                        }
                        Ok(_) => {}
                    }
                }
            });
        }
        Ok(signals)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn show_pairing(secret: &str) -> Result<()> {
    use qrcodegen::{QrCode, QrCodeEcc};
    let qr = QrCode::encode_text(secret, QrCodeEcc::Low)
        .map_err(|_| anyhow::anyhow!("pairing code cannot be rendered"))?;
    println!("Scan this code with your messenger's linked-device screen:");
    for y in (-2..qr.size() + 2).step_by(2) {
        let mut row = String::new();
        for x in -2..qr.size() + 2 {
            row.push(match (qr.get_module(x, y), qr.get_module(x, y + 1)) {
                (false, false) => '█',
                (true, false) => '▄',
                (false, true) => '▀',
                (true, true) => ' ',
            });
        }
        println!("{row}");
    }
    Ok(())
}
