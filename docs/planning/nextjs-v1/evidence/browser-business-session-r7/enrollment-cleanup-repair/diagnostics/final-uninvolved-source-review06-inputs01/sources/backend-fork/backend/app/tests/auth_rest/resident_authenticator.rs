//! Test-only native crypto fixture. No business API or credential injection.
//! Import as an auth_rest child module; keep production/C2 browser proof separate.
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use url::Url;
use webauthn_rs::prelude::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

const ORIGIN: &str = "https://auth.example.com";
const MAX_FRAME: usize = 65_536;
const MAX_FRAMES: u64 = 128;
const PHASE: Duration = Duration::from_secs(25);
const CLEANUP: Duration = Duration::from_secs(15);
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
enum CleanupStage {
    Startup,
    BrowserIdentity,
    BrowserClose,
    BrowserKill,
    BrowserExit,
    Ack,
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CleanupDiagnostic {
    stage: CleanupStage,
    child_exit_seen: bool,
    child_close_seen: bool,
    close_settled: bool,
    kill_settled: bool,
    child_exited: bool,
}

type Result<T> = std::result::Result<T, &'static str>;

pub(super) struct ResidentAuthenticator {
    child: Child,
    input: Option<SyncSender<Vec<u8>>>,
    output: Option<Receiver<Result<Value>>>,
    writes: Receiver<Result<()>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    error_reader: Option<JoinHandle<(usize, Option<CleanupDiagnostic>)>>,
    sent: u64,
    received: u64,
    browser_pid: Option<u32>,
    failed: bool,
    cleaned: bool,
    spawned_at: Instant,
    last_ceremony_read: Option<Instant>,
}

impl ResidentAuthenticator {
    pub(super) fn new() -> Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let runner = root.join("tools/test-resident-authenticator.mjs");
        if !runner.is_file() || !root.join("package-lock.json").is_file() {
            return Err("infrastructure: resident fixture/locked frontend missing");
        }
        let mut command = Command::new("node");
        command
            .arg(runner)
            .current_dir(root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "LANG",
            "LC_ALL",
            "PLAYWRIGHT_BROWSERS_PATH",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let spawned_at = Instant::now();
        let mut child = command
            .spawn()
            .map_err(|_| "infrastructure: resident Node start")?;
        let mut stdin = child.stdin.take().ok_or("infrastructure: resident stdin")?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or("infrastructure: resident stdout")?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or("infrastructure: resident stderr")?;
        let (input, commands) = mpsc::sync_channel::<Vec<u8>>(1);
        let (written, writes) = mpsc::sync_channel(1);
        // Separate I/O workers let every synchronous public operation have a
        // deadline without blocking an async database/lock-test task indefinitely.
        let writer = thread::spawn(move || {
            while let Ok(bytes) = commands.recv() {
                let outcome = stdin
                    .write_all(&(bytes.len() as u32).to_be_bytes())
                    .and_then(|()| stdin.write_all(&bytes))
                    .and_then(|()| stdin.flush())
                    .map_err(|_| "infrastructure: resident frame write");
                let failed = outcome.is_err();
                if written.send(outcome).is_err() || failed {
                    break;
                }
            }
            // Disconnect/Drop closes stdin, so the Node owner performs cleanup.
        });
        let (frames, output) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            for _ in 0..MAX_FRAMES {
                let outcome = (|| {
                    let mut header = [0_u8; 4];
                    stdout
                        .read_exact(&mut header)
                        .map_err(|_| "infrastructure: resident frame header")?;
                    let length = u32::from_be_bytes(header) as usize;
                    if length == 0 || length > MAX_FRAME {
                        return Err("infrastructure: resident frame size");
                    }
                    let mut bytes = vec![0_u8; length];
                    stdout
                        .read_exact(&mut bytes)
                        .map_err(|_| "infrastructure: resident frame body")?;
                    serde_json::from_slice(&bytes)
                        .map_err(|_| "infrastructure: resident frame JSON")
                })();
                let failed = outcome.is_err();
                if frames.send(outcome).is_err() || failed {
                    break;
                }
            }
        });
        let error_reader = thread::spawn(move || {
            let mut bytes = [0_u8; 8192];
            let mut count = 0_usize;
            let mut prefix = Vec::with_capacity(512);
            while let Ok(length) = stderr.read(&mut bytes) {
                if length == 0 {
                    break;
                }
                count = count.saturating_add(length);
                let retained = length.min(512 - prefix.len());
                prefix.extend_from_slice(&bytes[..retained]);
            }
            let diagnostic = prefix.split(|byte| *byte == b'\n').find_map(|line| {
                let payload = line.strip_prefix(b"RESIDENT_CLEANUP_DIAGNOSTIC ")?;
                serde_json::from_slice::<CleanupDiagnostic>(payload).ok()
            });
            (count, diagnostic)
        });
        let mut value = Self {
            child,
            input: Some(input),
            output: Some(output),
            writes,
            writer: Some(writer),
            reader: Some(reader),
            error_reader: Some(error_reader),
            sent: 0,
            received: 0,
            browser_pid: None,
            failed: true,
            cleaned: false,
            spawned_at,
            last_ceremony_read: None,
        };
        let ready = value.read(PHASE)?;
        if ready["kind"] != "ready" || ready["origin"] != ORIGIN || ready["secure_context"] != true
        {
            return Err("infrastructure: resident secure origin not ready");
        }
        value.browser_pid = ready["browser_pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0);
        if value.browser_pid.is_none() {
            return Err("infrastructure: resident browser process identity");
        }
        value.failed = false;
        Ok(value)
    }

    pub(super) fn do_registration(
        &mut self,
        origin: Url,
        options: CreationChallengeResponse,
    ) -> Result<RegisterPublicKeyCredential> {
        self.perform(origin, "register", "registered", options)
    }

    pub(super) fn do_authentication(
        &mut self,
        origin: Url,
        options: RequestChallengeResponse,
    ) -> Result<PublicKeyCredential> {
        self.perform(origin, "authenticate", "authenticated", options)
    }

    fn perform<T: serde::de::DeserializeOwned>(
        &mut self,
        origin: Url,
        kind: &str,
        expected: &str,
        options: impl serde::Serialize,
    ) -> Result<T> {
        self.failed = true;
        if origin.as_str() != format!("{ORIGIN}/") {
            return Err("infrastructure: resident origin mismatch");
        }
        self.write(
            json!({"kind":kind,"origin":ORIGIN,"options":options}),
            PHASE,
        )?;
        let result = self.read(PHASE)?;
        if result["kind"] != expected || result["resident"] != true {
            return Err("infrastructure: genuine resident ceremony failed");
        }
        let credential = serde_json::from_value(result["credential"].clone())
            .map_err(|_| "infrastructure: resident credential codec")?;
        self.last_ceremony_read = Some(Instant::now());
        self.failed = false;
        Ok(credential)
    }

    fn write(&mut self, mut value: Value, deadline: Duration) -> Result<()> {
        if self.sent >= MAX_FRAMES {
            return Err("infrastructure: resident frame count");
        }
        value["v"] = json!(1);
        value["seq"] = json!(self.sent);
        let bytes =
            serde_json::to_vec(&value).map_err(|_| "infrastructure: resident frame encode")?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME {
            return Err("infrastructure: resident frame size");
        }
        self.input
            .as_ref()
            .ok_or("infrastructure: resident stdin closed")?
            .try_send(bytes)
            .map_err(|_| "infrastructure: resident writer unavailable")?;
        self.writes
            .recv_timeout(deadline)
            .map_err(|_| "infrastructure: resident write deadline")??;
        self.sent += 1;
        Ok(())
    }

    fn read(&mut self, deadline: Duration) -> Result<Value> {
        let frame = self
            .output
            .as_ref()
            .ok_or("infrastructure: resident stdout closed")?
            .recv_timeout(deadline)
            .map_err(|_| "infrastructure: resident read deadline")??;
        if self.received >= MAX_FRAMES || frame["v"] != 1 || frame["seq"] != self.received {
            return Err("infrastructure: resident frame identity");
        }
        self.received += 1;
        Ok(frame)
    }

    pub(super) fn close(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        let owner_age_ms = self.spawned_at.elapsed().as_millis();
        let ceremony_idle_ms = self
            .last_ceremony_read
            .map(|time| time.elapsed().as_millis());
        let sent_before = self.sent;
        let received_before = self.received;
        let panicking = thread::panicking();
        let deadline = Instant::now() + CLEANUP;
        // Successful close requests done. Error/panic uses EOF, as C2 does.
        let done = if self.failed || thread::panicking() {
            false
        } else {
            self.write(json!({"kind":"done"}), CLEANUP).is_ok()
        };
        if !done {
            self.input.take();
        }
        // A timed-out command may still deliver its ordinary response before
        // cleanup. Drain those checked frames under one total cleanup deadline.
        let acknowledgement = (|| loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("infrastructure: resident cleanup deadline")?;
            let frame = self.read(remaining)?;
            match frame["kind"].as_str() {
                Some("cleaned") => break Ok(frame),
                Some("ready" | "registered" | "authenticated" | "error") => {}
                _ => break Err("infrastructure: resident cleanup frame"),
            }
        })();
        self.input.take();
        // Release any reader blocked sending EOF/error into the one-slot queue
        // before waiting for worker exit. Never join with its Receiver alive.
        self.output.take();
        let status = loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| "infrastructure: resident wait")?
            {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                self.cleaned = true;
                return Err("infrastructure: resident cleanup unconfirmed; possible orphan");
            }
            thread::sleep(Duration::from_millis(20));
        };
        self.cleaned = true;
        let workers_deadline = Instant::now() + CLEANUP;
        let workers_finished = || {
            self.writer.as_ref().is_some_and(JoinHandle::is_finished)
                && self.reader.as_ref().is_some_and(JoinHandle::is_finished)
                && self
                    .error_reader
                    .as_ref()
                    .is_some_and(JoinHandle::is_finished)
        };
        while !workers_finished() && Instant::now() < workers_deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if !workers_finished() {
            return Err("infrastructure: resident worker cleanup deadline");
        }
        for worker in [&mut self.writer, &mut self.reader] {
            if let Some(worker) = worker.take() {
                worker
                    .join()
                    .map_err(|_| "infrastructure: resident I/O worker failed")?;
            }
        }
        let (count, diagnostic) = self
            .error_reader
            .take()
            .ok_or("infrastructure: resident stderr worker missing")?
            .join()
            .map_err(|_| "infrastructure: resident stderr worker failed")?;
        let acknowledgement = acknowledgement.inspect_err(|reason| {
            if let Some(diagnostic) = &diagnostic {
                eprintln!(
                    "RESIDENT_CLEANUP_DIAGNOSTIC stage={:?} child_exit_seen={} child_close_seen={} close_settled={} kill_settled={} child_exited={}",
                    diagnostic.stage, diagnostic.child_exit_seen, diagnostic.child_close_seen,
                    diagnostic.close_settled, diagnostic.kill_settled, diagnostic.child_exited
                );
            }
            eprintln!(
                "RESIDENT_CLEANUP_FAILURE reason={reason} exit_status={status:?} stderr_bytes={count} sent_before={sent_before} checked_frames_before={received_before} sent={} checked_frames={} done={done} failed={} panicking={panicking} owner_age_ms={owner_age_ms} ceremony_idle_ms={ceremony_idle_ms:?}",
                self.sent, self.received, self.failed
            );
        })?;
        if count > 1_048_576 || acknowledgement["kind"] != "cleaned" {
            return Err("infrastructure: resident cleanup acknowledgement");
        }
        if let Some(pid) = self.browser_pid
            && (acknowledgement["browser_pid"] != pid || acknowledgement["browser_exited"] != true)
        {
            return Err("infrastructure: resident browser cleanup identity");
        }
        let expected = match acknowledgement["outcome"].as_str() {
            Some("completed") if done => 0,
            Some("aborted") if !done => 2,
            Some("failed") if self.failed => 1,
            _ => return Err("infrastructure: resident cleanup outcome"),
        };
        if status.code() != Some(expected) {
            return Err("infrastructure: resident cleanup exit");
        }
        Ok(())
    }
}

impl Drop for ResidentAuthenticator {
    fn drop(&mut self) {
        if let Err(message) = self.close() {
            if thread::panicking() {
                eprintln!("{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}
