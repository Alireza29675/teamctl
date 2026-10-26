use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};
use team_core::chat::protocol::{self, AdapterFrame, HostFrame};

type WriteRequest = (Vec<u8>, Sender<Result<()>>);
pub struct Process {
    child: Child,
    input: SyncSender<WriteRequest>,
    pub events: Receiver<Result<AdapterFrame>>,
}

impl Process {
    pub fn spawn(
        command: &str,
        args: &[String],
        root: &Path,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self> {
        let mut builder = Command::new(command);
        builder.env_clear();
        for key in [
            "PATH",
            "HOME",
            "USERPROFILE",
            "TMPDIR",
            "TEMP",
            "SYSTEMROOT",
            "WINDIR",
            "LANG",
            "LC_ALL",
        ] {
            if let Some(value) = std::env::var_os(key) {
                builder.env(key, value);
            }
        }
        builder
            .args(args)
            .current_dir(root)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            builder.process_group(0);
        }
        let mut child = builder.spawn().context("start adapter executable")?;
        let mut pipe = child.stdin.take().context("adapter stdin unavailable")?;
        let output = child.stdout.take().context("adapter stdout unavailable")?;
        let errors = child.stderr.take().context("adapter stderr unavailable")?;
        let (tx, events) = mpsc::sync_channel(64);
        let (input, writes) = mpsc::sync_channel::<WriteRequest>(8);
        std::thread::spawn(move || {
            while let Ok((bytes, confirmation)) = writes.recv() {
                let result = pipe
                    .write_all(&bytes)
                    .and_then(|_| pipe.flush())
                    .map_err(Into::into);
                let failed = result.is_err();
                let _ = confirmation.send(result);
                if failed {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let frame = read_frame(&mut reader)
                    .and_then(|bytes| protocol::decode_adapter(&bytes).map_err(Into::into));
                let failed = frame.is_err();
                if tx.send(frame).is_err() || failed {
                    break;
                }
            }
        });
        // Diagnostics are drained so a noisy plugin cannot block protocol IO.
        // Raw diagnostics may contain provider secrets, so they are not logged.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(errors), &mut std::io::sink());
        });
        Ok(Self {
            child,
            input,
            events,
        })
    }
    pub fn send(&mut self, frame: &HostFrame) -> Result<()> {
        let (confirmation, completed) = mpsc::channel();
        self.input
            .try_send((protocol::encode_host(frame)?, confirmation))
            .map_err(|_| {
                anyhow::anyhow!("adapter protocol writer is unavailable or backpressured")
            })?;
        completed
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| anyhow::anyhow!("adapter protocol write timed out"))?
    }
    pub fn stopped(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
    fn shutdown(&mut self) {
        let leader_exited = self.child.try_wait().ok().flatten().is_some();
        if leader_exited && !self.group_alive() {
            return;
        }
        if !leader_exited {
            let _ = self.send(&HostFrame::Shutdown);
            if self.wait_for_cleanup(Duration::from_secs(3)) {
                return;
            }
        }
        // Node/Puppeteer also receives SIGTERM so it can stop a detached browser.
        // Jumping straight to SIGKILL could leave that owned browser behind.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGTERM);
        }
        if self.wait_for_cleanup(Duration::from_secs(2)) {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    fn group_alive(&self) -> bool {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-(self.child.id() as i32), 0) == 0
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
            }
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
    fn wait_for_cleanup(&mut self, budget: Duration) -> bool {
        let end = Instant::now() + budget;
        while Instant::now() < end {
            if self.child.try_wait().ok().flatten().is_some() && !self.group_alive() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut frame = Vec::new();
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            bail!("adapter protocol stream closed");
        }
        let end = chunk.iter().position(|c| *c == b'\n').map(|i| i + 1);
        let count = end.unwrap_or(chunk.len());
        anyhow::ensure!(
            frame.len() + count <= protocol::MAX_FRAME_BYTES,
            "adapter frame exceeds protocol limit"
        );
        frame.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_unterminated_frame_is_bounded() {
        let data = vec![b'x'; protocol::MAX_FRAME_BYTES + 2];
        assert!(read_frame(&mut std::io::Cursor::new(data)).is_err());
    }
}
