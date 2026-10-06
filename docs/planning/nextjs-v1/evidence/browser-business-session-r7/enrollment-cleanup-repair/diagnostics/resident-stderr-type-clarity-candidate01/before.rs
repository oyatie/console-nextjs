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
    BrowserCleanup,
    OwnedTemp,
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
    owned_temp_seen: bool,
    owned_temp_removed: bool,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
enum SetupStage {
    Initial,
    TemporaryScope,
    BrowserLaunch,
    BrowserConnect,
    Context,
    Route,
    Page,
    Navigate,
    SecureUrl,
    SecureState,
    CdpSession,
    WebauthnEnable,
    VirtualAuthenticator,
    BrowserIdentity,
    Ready,
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct SetupDiagnostic {
    stage: SetupStage,
    setup_age_ms: u32,
    stage_age_ms: u32,
    age_capped: bool,
    budget_ms: Option<u32>,
    budget_elapsed: Option<bool>,
}

impl SetupDiagnostic {
    fn bounded(&self) -> bool {
        self.setup_age_ms <= 600_000
            && self.stage_age_ms <= 600_000
            && match self.budget_ms {
                None => self.budget_elapsed.is_none(),
                Some(5_000 | 20_000) => self.budget_elapsed.is_some(),
                Some(_) => false,
            }
    }
}

#[derive(Debug)]
enum FirstFrameCategory {
    Ready,
    FixtureFailure,
    Cleaned,
    Other,
}

type Result<T> = std::result::Result<T, &'static str>;

// Private fixture failures simulate thread setup errors, not OS exhaustion.
// They never replace a browser, credential ceremony or provider result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartupFault {
    Writer,
    Reader,
    Stderr,
}

#[derive(Debug)]
struct CleanupObservation {
    result: Result<()>,
    node_pid: u32,
    node_reaped: bool,
    workers_settled: bool,
    created_workers: [bool; 3],
    remaining_worker_handles: [bool; 3],
    join_succeeded: [Option<bool>; 3],
    forced: bool,
    phase_deadline_exceeded: [bool; 2],
}

pub(super) struct ResidentAuthenticator {
    child: Child,
    input: Option<SyncSender<Vec<u8>>>,
    output: Option<Receiver<Result<Value>>>,
    writes: Receiver<Result<()>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    error_reader: Option<JoinHandle<(usize, Option<CleanupDiagnostic>, Option<SetupDiagnostic>)>>,
    sent: u64,
    received: u64,
    browser_pid: Option<u32>,
    failed: bool,
    constructing: bool,
    close_result: Option<Result<()>>,
    node_reaped: bool,
    workers_settled: bool,
    spawned_at: Instant,
    last_ceremony_read: Option<Instant>,
    cleanup_probe: Option<mpsc::Sender<CleanupObservation>>,
}

impl ResidentAuthenticator {
    pub(super) fn new() -> Result<Self> {
        Self::new_observed(None, None)
    }

    fn new_observed(
        fault: Option<StartupFault>,
        cleanup_probe: Option<mpsc::Sender<CleanupObservation>>,
    ) -> Result<Self> {
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
        let (input, commands) = mpsc::sync_channel::<Vec<u8>>(1);
        let (written, writes) = mpsc::sync_channel(1);
        let (frames, output) = mpsc::sync_channel(1);
        let spawned_at = Instant::now();
        let child = command
            .spawn()
            .map_err(|_| "infrastructure: resident Node start")?;
        // Install the existing cleanup owner before taking pipes or creating a
        // fallible worker. Partial construction never abandons the Child.
        let mut value = Self {
            child,
            input: Some(input),
            output: Some(output),
            writes,
            writer: None,
            reader: None,
            error_reader: None,
            sent: 0,
            received: 0,
            browser_pid: None,
            failed: true,
            constructing: true,
            close_result: None,
            node_reaped: false,
            workers_settled: false,
            spawned_at,
            last_ceremony_read: None,
            cleanup_probe,
        };
        let mut stdin = value
            .child
            .stdin
            .take()
            .ok_or("infrastructure: resident stdin")?;
        let mut stdout = value
            .child
            .stdout
            .take()
            .ok_or("infrastructure: resident stdout")?;
        let mut stderr = value
            .child
            .stderr
            .take()
            .ok_or("infrastructure: resident stderr")?;
        // Separate I/O workers let every synchronous public operation have a
        // deadline without blocking an async database/lock-test task indefinitely.
        if fault == Some(StartupFault::Writer) {
            return Err("infrastructure: resident writer start");
        }
        value.writer = Some(
            thread::Builder::new()
                .spawn(move || {
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
                })
                .map_err(|_| "infrastructure: resident writer start")?,
        );
        if fault == Some(StartupFault::Reader) {
            return Err("infrastructure: resident reader start");
        }
        value.reader = Some(
            thread::Builder::new()
                .spawn(move || {
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
                })
                .map_err(|_| "infrastructure: resident reader start")?,
        );
        if fault == Some(StartupFault::Stderr) {
            return Err("infrastructure: resident stderr start");
        }
        value.error_reader = Some(
            thread::Builder::new()
                .spawn(move || {
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
                    let setup_diagnostic = prefix.split(|byte| *byte == b'\n').find_map(|line| {
                        let payload = line.strip_prefix(b"RESIDENT_SETUP_DIAGNOSTIC ")?;
                        serde_json::from_slice::<SetupDiagnostic>(payload)
                            .ok()
                            .filter(SetupDiagnostic::bounded)
                    });
                    (count, diagnostic, setup_diagnostic)
                })
                .map_err(|_| "infrastructure: resident stderr start")?,
        );
        // Observe the unchanged readiness wait from its actual start. Spawn and
        // worker setup age do not consume read(PHASE)'s existing budget.
        let readiness_started_at = Instant::now();
        let ready = value.read(PHASE)?;
        if ready["kind"] != "ready" || ready["origin"] != ORIGIN || ready["secure_context"] != true
        {
            // read() already checked frame version/sequence. Classify only fixed
            // literals; never print the received JSON, URL, code or browser ID.
            let category = match ready["kind"].as_str() {
                Some("ready") => FirstFrameCategory::Ready,
                Some("error") if ready["code"] == "fixture-failure" => {
                    FirstFrameCategory::FixtureFailure
                }
                Some("cleaned") => FirstFrameCategory::Cleaned,
                _ => FirstFrameCategory::Other,
            };
            let age_ms = value.spawned_at.elapsed().as_millis();
            let readiness_age_ms = readiness_started_at.elapsed().as_millis();
            let _ = writeln!(
                std::io::stderr().lock(),
                "RESIDENT_FIRST_FRAME_FAILURE category={category:?} owner_age_ms={} owner_age_capped={} readiness_age_ms={} phase_budget_elapsed={} origin_matches={} secure_context_true={}",
                age_ms.min(PHASE.as_millis()),
                age_ms > PHASE.as_millis(),
                readiness_age_ms.min(PHASE.as_millis()),
                readiness_age_ms >= PHASE.as_millis(),
                ready["origin"] == ORIGIN,
                ready["secure_context"] == true
            );
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
        value.constructing = false;
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
        if let Some(result) = self.close_result {
            return result;
        }
        let cleanup = self.close_owned();
        let result = cleanup.result;
        // A finalized cleanup attempt is not necessarily successful. Retain its
        // failure, and never start another budget during Drop or a second close.
        self.close_result = Some(result);
        // Test observation is emitted only after the real attempt and cache.
        // An unbounded private channel cannot block the resource owner.
        if let Some(probe) = &self.cleanup_probe {
            let _ = probe.send(cleanup);
        }
        result
    }

    fn close_owned(&mut self) -> CleanupObservation {
        let created_workers = [
            self.writer.is_some(),
            self.reader.is_some(),
            self.error_reader.is_some(),
        ];
        let mut join_succeeded = [None; 3];
        let owner_age_ms = self.spawned_at.elapsed().as_millis();
        let ceremony_idle_ms = self
            .last_ceremony_read
            .map(|time| time.elapsed().as_millis());
        let sent_before = self.sent;
        let received_before = self.received;
        let panicking = thread::panicking();
        let partial = self.constructing
            && (self.writer.is_none() || self.reader.is_none() || self.error_reader.is_none());
        let deadline = Instant::now() + CLEANUP;
        let done = if self.failed || panicking || partial {
            false
        } else {
            self.write(json!({"kind":"done"}), CLEANUP).is_ok()
        };
        if !done {
            self.input.take();
        }
        // Ordinary cleanup retains its complete ACK window. Uncreated workers
        // cannot manufacture an ACK; partial startup is already a failure.
        let acknowledgement = if partial {
            Err("infrastructure: resident startup workers incomplete")
        } else {
            (|| loop {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or("infrastructure: resident cleanup deadline")?;
                let frame = self.read(remaining)?;
                if Instant::now() >= deadline {
                    return Err("infrastructure: resident cleanup deadline");
                }
                match frame["kind"].as_str() {
                    Some("cleaned") => break Ok(frame),
                    Some("ready" | "registered" | "authenticated" | "error") => {}
                    _ => break Err("infrastructure: resident cleanup frame"),
                }
            })()
        };
        self.input.take();
        self.output.take(); // Release any reader blocked sending into its queue.
        self.child.stdin.take(); // Also closes a pipe not yet moved into a worker.
        let mut failure = acknowledgement.as_ref().err().copied();
        let mut status = None;
        let mut forced = false;
        loop {
            match self.child.try_wait() {
                Ok(Some(observed)) => {
                    status = Some(observed);
                    if Instant::now() >= deadline {
                        failure.get_or_insert("infrastructure: resident cleanup deadline");
                    }
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    failure.get_or_insert("infrastructure: resident wait");
                    forced = true;
                    let _ = self.child.kill();
                    break;
                }
            }
            if Instant::now() >= deadline {
                failure
                    .get_or_insert("infrastructure: resident cleanup unconfirmed; possible orphan");
                forced = true;
                let _ = self.child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let first_phase_deadline_exceeded = Instant::now() >= deadline;
        // Reuse the existing worker phase for actual reaping and worker drain.
        // Force-kill is sticky failure, never a substitute for natural exit/ACK.
        let workers_deadline = Instant::now() + CLEANUP;
        let workers_finished = |value: &Self| {
            value.writer.as_ref().is_none_or(JoinHandle::is_finished)
                && value.reader.as_ref().is_none_or(JoinHandle::is_finished)
                && value
                    .error_reader
                    .as_ref()
                    .is_none_or(JoinHandle::is_finished)
        };
        while (status.is_none() || !workers_finished(self)) && Instant::now() < workers_deadline {
            if status.is_none() {
                match self.child.try_wait() {
                    Ok(Some(observed)) => status = Some(observed),
                    Ok(None) => {}
                    Err(_) => {
                        failure.get_or_insert("infrastructure: resident wait");
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.node_reaped = status.is_some();
        self.workers_settled = workers_finished(self);
        if !self.node_reaped {
            failure.get_or_insert("infrastructure: resident Node reap unconfirmed");
        }
        let second_phase_deadline_exceeded = Instant::now() >= workers_deadline;
        if !self.workers_settled || second_phase_deadline_exceeded {
            failure.get_or_insert("infrastructure: resident worker cleanup deadline");
        }
        if !self.constructing
            && (self.writer.is_none() || self.reader.is_none() || self.error_reader.is_none())
        {
            failure.get_or_insert("infrastructure: resident cleanup worker missing");
        }
        for (index, worker) in [&mut self.writer, &mut self.reader].into_iter().enumerate() {
            if worker.as_ref().is_some_and(JoinHandle::is_finished)
                && let Some(worker) = worker.take()
            {
                let succeeded = worker.join().is_ok();
                join_succeeded[index] = Some(succeeded);
                if !succeeded {
                    failure.get_or_insert("infrastructure: resident I/O worker failed");
                }
            }
        }
        let mut count = 0;
        let mut diagnostic = None;
        let mut setup_diagnostic = None;
        if self
            .error_reader
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
            && let Some(worker) = self.error_reader.take()
        {
            match worker.join() {
                Ok(observed) => {
                    join_succeeded[2] = Some(true);
                    count = observed.0;
                    diagnostic = observed.1;
                    setup_diagnostic = observed.2;
                }
                Err(_) => {
                    join_succeeded[2] = Some(false);
                    failure.get_or_insert("infrastructure: resident stderr worker failed");
                }
            }
        }
        // A constructor error may have a successful cleanup. Preserve its fixed
        // setup observation independently of the cleanup result.
        if self.constructing
            && let Some(diagnostic) = &setup_diagnostic
        {
            let _ = writeln!(
                std::io::stderr().lock(),
                "RESIDENT_SETUP_DIAGNOSTIC stage={:?} setup_age_ms={} stage_age_ms={} age_capped={} budget_ms={:?} budget_elapsed={:?}",
                diagnostic.stage,
                diagnostic.setup_age_ms,
                diagnostic.stage_age_ms,
                diagnostic.age_capped,
                diagnostic.budget_ms,
                diagnostic.budget_elapsed
            );
        }
        let result = (|| {
            if let Some(reason) = failure {
                return Err(reason);
            }
            let acknowledgement = acknowledgement?;
            if count > 1_048_576 || acknowledgement["kind"] != "cleaned" {
                return Err("infrastructure: resident cleanup acknowledgement");
            }
            if let Some(pid) = self.browser_pid
                && (acknowledgement["browser_pid"] != pid
                    || acknowledgement["browser_exited"] != true)
            {
                return Err("infrastructure: resident browser cleanup identity");
            }
            let expected = match acknowledgement["outcome"].as_str() {
                Some("completed") if done => 0,
                Some("aborted") if !done => 2,
                Some("failed") if self.failed => 1,
                _ => return Err("infrastructure: resident cleanup outcome"),
            };
            if status.and_then(|status| status.code()) != Some(expected) {
                return Err("infrastructure: resident cleanup exit");
            }
            Ok(())
        })();
        if let Err(reason) = result {
            if let Some(diagnostic) = &diagnostic {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "RESIDENT_CLEANUP_DIAGNOSTIC stage={:?} child_exit_seen={} child_close_seen={} close_settled={} kill_settled={} child_exited={} owned_temp_seen={} owned_temp_removed={}",
                    diagnostic.stage,
                    diagnostic.child_exit_seen,
                    diagnostic.child_close_seen,
                    diagnostic.close_settled,
                    diagnostic.kill_settled,
                    diagnostic.child_exited,
                    diagnostic.owned_temp_seen,
                    diagnostic.owned_temp_removed
                );
            }
            let _ = writeln!(
                std::io::stderr().lock(),
                "RESIDENT_CLEANUP_FAILURE reason={reason} exit_status={status:?} stderr_bytes={count} sent_before={sent_before} checked_frames_before={received_before} sent={} checked_frames={} done={done} failed={} panicking={panicking} owner_age_ms={owner_age_ms} ceremony_idle_ms={ceremony_idle_ms:?} constructing={} forced={forced} node_reaped={} workers_settled={}",
                self.sent,
                self.received,
                self.failed,
                self.constructing,
                self.node_reaped,
                self.workers_settled
            );
        }
        CleanupObservation {
            result,
            node_pid: self.child.id(),
            node_reaped: self.node_reaped,
            workers_settled: self.workers_settled,
            created_workers,
            remaining_worker_handles: [
                self.writer.is_some(),
                self.reader.is_some(),
                self.error_reader.is_some(),
            ],
            join_succeeded,
            forced,
            phase_deadline_exceeded: [
                first_phase_deadline_exceeded,
                second_phase_deadline_exceeded,
            ],
        }
    }
}

impl Drop for ResidentAuthenticator {
    fn drop(&mut self) {
        if let Err(message) = self.close() {
            if self.constructing || thread::panicking() {
                let _ = writeln!(std::io::stderr().lock(), "{message}");
            } else {
                panic!("{message}");
            }
        }
    }
}

// These tests run the actual Node/Chromium fixture. Private fault injection
// covers ownership after spawn; it does not claim empirical thread exhaustion.
fn assert_setup_failure(
    fault: StartupFault,
    expected_error: &'static str,
    expected_created_workers: [bool; 3],
) {
    let (observed, observations) = mpsc::channel();
    let outcome = std::panic::catch_unwind(|| {
        ResidentAuthenticator::new_observed(Some(fault), Some(observed))
    });
    let error = match outcome {
        Ok(Err(error)) => error,
        Ok(Ok(mut owner)) => {
            let _ = owner.close();
            panic!("private setup failure did not trigger");
        }
        Err(_) => panic!("constructor error replaced by cleanup panic"),
    };
    assert_eq!(error, expected_error);
    let cleanup = observations
        .try_recv()
        .expect("real cleanup observation before constructor returns");
    assert_eq!(
        cleanup.result,
        Err("infrastructure: resident startup workers incomplete")
    );
    assert!(cleanup.node_pid > 0);
    assert!(cleanup.node_reaped, "actual Node status was not observed");
    assert!(cleanup.workers_settled, "created workers did not finish");
    assert_eq!(cleanup.created_workers, expected_created_workers);
    assert_eq!(cleanup.remaining_worker_handles, [false; 3]);
    assert_eq!(
        cleanup.join_succeeded,
        expected_created_workers.map(|created| created.then_some(true))
    );
    assert!(
        !cleanup.forced,
        "constructor cleanup needed forced Node kill"
    );
    assert_eq!(cleanup.phase_deadline_exceeded, [false; 2]);
    assert!(matches!(
        observations.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    // Partial construction has no trusted ready browser identity or independent
    // process-group absence witness. Never infer Chromium absence from Node.
    let _ = writeln!(
        std::io::stderr().lock(),
        "RESIDENT_STARTUP_PROBE fault={fault:?} simulated_setup_error=true node_reaped=true created_workers_joined_successfully=true forced=false phase_deadlines_exceeded=false browser_custody=UNKNOWN"
    );
}

#[test]
fn injected_writer_setup_failure_preserves_constructor_error_and_resource_custody() {
    assert_setup_failure(
        StartupFault::Writer,
        "infrastructure: resident writer start",
        [false, false, false],
    );
}

#[test]
fn injected_reader_setup_failure_preserves_constructor_error_and_resource_custody() {
    assert_setup_failure(
        StartupFault::Reader,
        "infrastructure: resident reader start",
        [true, false, false],
    );
}

#[test]
fn injected_stderr_setup_failure_preserves_constructor_error_and_resource_custody() {
    assert_setup_failure(
        StartupFault::Stderr,
        "infrastructure: resident stderr start",
        [true, true, false],
    );
}

#[test]
fn failed_close_is_cached_without_restarting_cleanup_and_normal_drop_stays_failing() {
    let (observed, observations) = mpsc::channel();
    let mut owner = ResidentAuthenticator::new_observed(None, Some(observed))
        .expect("real resident fixture ready");
    // Dispose of the actual receiver. No frame, ACK or provider is fabricated.
    owner.output.take();
    owner.failed = true;
    let first = owner.close();
    assert_eq!(first, Err("infrastructure: resident stdout closed"));
    let cleanup = observations
        .try_recv()
        .expect("first real cleanup observation");
    assert_eq!(cleanup.result, first);
    assert!(cleanup.node_pid > 0);
    assert!(cleanup.node_reaped);
    assert!(cleanup.workers_settled);
    assert_eq!(cleanup.created_workers, [true; 3]);
    assert_eq!(cleanup.remaining_worker_handles, [false; 3]);
    assert_eq!(cleanup.join_succeeded, [Some(true); 3]);
    assert!(
        !cleanup.forced,
        "cached-failure cleanup needed forced Node kill"
    );
    assert_eq!(cleanup.phase_deadline_exceeded, [false; 2]);
    assert_eq!(owner.close(), first);
    assert!(matches!(
        observations.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(owner)));
    let expected_panic = match dropped {
        Err(payload) => {
            payload
                .downcast_ref::<&'static str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                == Some("infrastructure: resident stdout closed")
        }
        Ok(()) => false,
    };
    assert!(
        expected_panic,
        "normal Drop did not preserve fixed cleanup failure"
    );
    assert!(matches!(
        observations.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    let _ = writeln!(
        std::io::stderr().lock(),
        "RESIDENT_CACHED_FAILURE_PROBE real_node_reaped=true created_workers_joined_successfully=true forced=false phase_deadlines_exceeded=false cleanup_attempts=1 repeated_error_preserved=true normal_drop_expected_panic=true browser_custody=UNKNOWN"
    );
}
