//! Real executable-protocol fixtures. No browser, account or model requests.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use team_core::chat;
use tempfile::TempDir;

const PROVIDER: &str = r#"
import json, os, sys, time, threading, subprocess
init = json.loads(sys.stdin.readline())
config = init['config']; mode = config.get('fixture_mode','normal')
record = config['record']
def log(value):
    with open(record,'a') as f: f.write(json.dumps(value)+'\n')
def emit(value):
    print(json.dumps(value),flush=True)
log({'event':'started','pid':os.getpid(),'state_dir':init['state_dir'],
     'token_ok':os.environ.get('DEST_TOKEN')==config.get('expected_token'),
     'unrelated_leaked':'UNRELATED_SECRET' in os.environ})
if mode in ['fork_exit','fork_shutdown','fork_term']:
    helper=subprocess.Popen([sys.executable,'-c',"import time,signal; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)"])
    log({'event':'helper','pid':helper.pid})
    if mode=='fork_exit':
        emit({'type':'ready','version':1,'capabilities':['text'],'display_name':'Fixture'})
        sys.exit(0)
message={'type':'message','external_id':'source:1','peer_id':'peer:one',
         'sender_id':'human:one','text':'DO_NOT_DISPLAY سلام é 📎'}
if mode=='before_ready': emit(message)
if mode=='qr': emit({'type':'pairing','qr':'FAKE_QR_SECRET_DO_NOT_LOG'})
if mode=='oversized': print('x'*300000,flush=True)
if mode=='malformed': print('{"type":"pairing","qr":"FAKE_QR_SECRET_DO_NOT_LOG","extra":1}',flush=True)
if mode!='silent': emit({'type':'ready','version':2 if mode=='version' else 1,
    'capabilities':config.get('capabilities',['text','quoted_reply']),'display_name':'Fixture'})
if mode not in ['silent','before_ready','qr','oversized','malformed','version']:
    emit(message);emit(message)
    denied=dict(message,external_id='denied:1',sender_id='human:forbidden');emit(denied)
if mode=='reconnect':
    def reconnect():
        time.sleep(1.2);emit({'type':'connection','state':'disconnected'});time.sleep(.2)
        emit({'type':'connection','state':'connected'});log({'event':'reconnected'})
    threading.Thread(target=reconnect,daemon=True).start()
sends=0
for line in sys.stdin:
    frame=json.loads(line);log({'event':'host_frame','frame':frame})
    if frame['type']=='shutdown':
        if mode!='fork_term': break
    if frame['type']=='send':
        sends+=1
        if mode=='retry' and sends==1:
            emit({'type':'error','request_id':frame['request_id'],'code':'disconnected',
                  'message':'PRIVATE_PROVIDER_ERROR','retryable':True})
        elif mode!='hang_send':
            emit({'type':'sent','request_id':'stale:request','external_id':'wrong'})
            emit({'type':'sent','request_id':frame['request_id'],'external_id':'provider:sent'})
log({'event':'closed'})
"#;

pub(super) struct Fixture {
    pub(super) temp: TempDir,
    pub(super) root: PathBuf,
    pub(super) home: PathBuf,
    pub(super) state: PathBuf,
    pub(super) python: PathBuf,
    pub(super) path: std::ffi::OsString,
    pub(super) tmux: Option<PathBuf>,
    pub(super) socket: PathBuf,
}

pub(super) fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl Fixture {
    pub(super) fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("team with 'quotes'");
        let home = temp.path().join("home");
        let state = temp.path().join("operator-state");
        let bin = temp.path().join("bin");
        for directory in [
            &root,
            &home,
            &state,
            &bin,
            &root.join("projects"),
            &root.join("runtimes"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        let original = std::env::var_os("PATH").unwrap_or_default();
        let find = |name: &str| {
            std::env::split_paths(&original)
                .map(|p| p.join(name))
                .find(|p| p.is_file())
        };
        let python = find("python3").expect("offline adapter fixtures require python3");
        let tmux = find("tmux");
        let socket = temp.path().join("tmux.sock");
        if let Some(binary) = &tmux {
            executable(
                &bin.join("tmux"),
                &format!(
                    "#!/bin/sh\nexec {} -S {} \"$@\"\n",
                    quote(binary.to_str().unwrap()),
                    quote(socket.to_str().unwrap())
                ),
            );
        }
        executable(&bin.join("caffeinate"), "#!/bin/sh\nexit 0\n");
        fs::write(root.join("provider.py"), PROVIDER).unwrap();
        fs::write(root.join("projects/fixture.yaml"),"version: 2\nproject: {id: p, name: Fixture, cwd: .}\nmanagers:\n  manager: {runtime: fake}\nworkers:\n  worker: {runtime: fake, reports_to: manager}\n").unwrap();
        fs::write(
            root.join("runtimes/fake.yaml"),
            "binary: /bin/sleep\nargs: ['60']\n",
        )
        .unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&original));
        let path = std::env::join_paths(paths).unwrap();
        let fixture = Self {
            temp,
            root,
            home,
            state,
            python,
            path,
            tmux,
            socket,
        };
        fixture.configure(mode, 1, "audit-");
        fixture.database();
        fixture
    }
    pub(super) fn configure(&self, mode: &str, count: usize, prefix: &str) {
        let interfaces:Vec<_>=(0..count).map(|index|json!({"name":if index==0{"bridge"}else{"other"},"type":"process","config":{
            "agent":"p:manager","command":self.python,"args":[self.root.join("provider.py")],"allow_senders":["human:one"],"allow_peers":["peer:one"],
            "startup_timeout_secs":1,"env":{"DEST_TOKEN":"FIXTURE_SOURCE"},"options":{"fixture_mode":mode,"record":self.record(if index==0{"bridge"}else{"other"}),"expected_token":"first line\nsecond line '$()'"}}})).collect();
        let value = json!({"version":"2.0.0","broker":{"type":"sqlite","path":"custom/mail.db"},"supervisor":{"type":"tmux","tmux_prefix":prefix,"drain_timeout_secs":0},"projects":[{"file":"projects/fixture.yaml"}],"interfaces":interfaces});
        fs::write(
            self.root.join("team-compose.yaml"),
            serde_yaml::to_string(&value).unwrap(),
        )
        .unwrap();
    }
    pub(super) fn database(&self) -> rusqlite::Connection {
        let conn = chat::open(self.root.join("custom/mail.db")).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO projects(id,name) VALUES ('p','Fixture')",
            [],
        )
        .unwrap();
        for id in ["p:manager", "p:worker"] {
            conn.execute("INSERT OR IGNORE INTO agents(id,project_id,role,runtime,is_manager) VALUES (?1,'p','fixture','fake',1)",[id]).unwrap();
        }
        conn
    }
    pub(super) fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamctl"));
        command
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("PATH", &self.path)
            .env("FIXTURE_SOURCE", "first line\nsecond line '$()'")
            .env("UNRELATED_SECRET", "must-not-leak")
            .env_remove("TEAMCTL_ROOT")
            .env_remove("TMUX");
        command
    }
    pub(super) fn cli(&self, args: &[&str]) -> Output {
        let out = self.command(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
    pub(super) fn spawn(&self) -> Host {
        Host::spawn(self.command(&["adapter", "run", "bridge"]))
    }
    pub(super) fn record(&self, name: &str) -> PathBuf {
        self.temp.path().join(format!("{name}-events.jsonl"))
    }
    pub(super) fn events(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.record(name))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
    pub(super) fn wait(&self, name: &str, condition: impl Fn(&[Value]) -> bool) {
        wait(|| condition(&self.events(name)));
    }
    pub(super) fn stop(&self) {
        self.cli(&["adapter", "stop", "bridge"]);
    }
    pub(super) fn applied(&self) -> BTreeMap<String, Value> {
        serde_json::from_slice(&fs::read(self.root.join("state/adapters.json")).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(tmux) = &self.tmux {
            let _ = Command::new(tmux)
                .arg("-S")
                .arg(&self.socket)
                .arg("kill-server")
                .output();
        }
    }
}

pub(super) struct Host {
    pub(super) child: Child,
    pub(super) output: Arc<Mutex<Vec<u8>>>,
}
impl Host {
    pub(super) fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        for mut stream in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let out = output.clone();
            thread::spawn(move || {
                let mut bytes = [0; 4096];
                while let Ok(count) = stream.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    let mut out = out.lock().unwrap();
                    if out.len() + count <= 64 * 1024 {
                        out.extend_from_slice(&bytes[..count]);
                    }
                }
            });
        }
        Self { child, output }
    }
    pub(super) fn finish(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                start.elapsed() < Duration::from_secs(12),
                "host did not exit: {}",
                self.text()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
    pub(super) fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            let _ = self.finish();
        }
    }
}
pub(super) fn wait(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fixture timed out"
        );
        thread::sleep(Duration::from_millis(25));
    }
}
