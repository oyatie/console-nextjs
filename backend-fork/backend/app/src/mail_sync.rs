//! Inbound webmail sync worker wiring (B-mail-3).
//!
//! A single background task (mirroring how `start_postgres_listener` is spawned)
//! ticks on a fixed cadence. Each tick:
//!   1. ENUMERATES the accounts due for a sync pass across ALL tenants via the
//!      `comms_due_email_accounts` SECURITY DEFINER function (id-only, no
//!      secrets) — the one read that legitimately spans tenants;
//!   2. for each `(org, account)`, ARMS `app.current_org` to that tenant (every
//!      store call runs through `with_org_conn`/`with_audit` armed to the org, so
//!      a pass for org A can never read or write org B), re-reads the FULL sealed
//!      account under RLS, decrypts the IMAP password in-memory, and runs ONE
//!      incremental sync pass (`SyncService::sync_account`).
//!
//! A bounded [`Semaphore`] caps concurrent passes so a burst of due accounts
//! cannot exhaust connections/CPU. The whole worker is GRACEFUL: it only starts
//! when the master KEK is present, storage is configured, and `CONSOLE_MAIL_ENABLED`
//! is truthy — otherwise it is a no-op (the app boots normally, mail endpoints
//! still mount and return 503/empty as appropriate).
//!
//! No secret, recipient, body, or host is logged here — only non-secret counts +
//! the org/account ids + a fixed transport code on failure.

use std::sync::Arc;
use std::time::Duration;

use console_comms_adapter_imap::AsyncImapClient;
use console_comms_adapter_postgres::PgMailStore;
use console_comms_application::{
    MailAttachmentStore, MailFuture, MailReadStore, MailServiceError, MailStore, SyncService,
};
use console_comms_credential_cipher::EnvelopeCredentialCipher;
use console_kernel_core::Timestamp;
use console_platform_request_context::scope_org;
use console_platform_storage::{PresignGetRequest, S3ObjectStore, SeaweedS3Storage};
use tokio::sync::{Semaphore, watch};

/// Default seconds between scheduler ticks. Each tick dispatches the due batch;
/// per-account cadence is enforced by the `comms_due_email_accounts` function
/// (an account is "due" only when `last_sync_at` is older than its own
/// `sync_cadence_secs`, default 120). 120s here keeps the tick aligned.
const DEFAULT_TICK_SECS: u64 = 120;

/// Max concurrent account sync passes per tick (bounds connections/CPU).
const MAX_CONCURRENT_SYNCS: usize = 4;

/// The claim lease the DB stamps on a due account (migration 0116, mirrored by
/// the adapter's `SYNC_CLAIM_LEASE_SECS`). After it elapses another worker may
/// reclaim the account — so a single pass MUST finish before then.
const SYNC_CLAIM_LEASE_SECS: u64 = 600;

/// Hard ceiling on ONE account's sync pass. Set SAFELY below the claim lease so a
/// hung/slow pass is aborted before the lease can expire and a second worker
/// reclaim the account (which would run an overlapping sync). The ~120s margin
/// covers the fenced lease-clear write + clock skew between worker replicas.
const SYNC_ACCOUNT_TIMEOUT: Duration = Duration::from_secs(480);

// Compile-time guard on the safety margin: the per-pass timeout must stay below
// the lease, or a slow pass could outlive its claim and be double-synced.
const _: () = assert!(SYNC_ACCOUNT_TIMEOUT.as_secs() < SYNC_CLAIM_LEASE_SECS);

/// Presigned-GET lifetime for inbound attachments served to the UI.
const ATTACHMENT_GET_TTL: Duration = Duration::from_secs(300);

/// A handle that stops the sync worker on drop / explicit shutdown.
#[derive(Debug)]
pub struct MailSyncHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl MailSyncHandle {
    /// Signal the worker loop to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// The object-storage adapter bridging `console-platform-storage`'s `S3ObjectStore`
/// to the application's [`MailAttachmentStore`] port. Uploads inbound attachment
/// bytes under org-prefixed keys and issues short-lived presigned GETs.
#[derive(Clone)]
pub struct S3MailAttachmentStore {
    store: SeaweedS3Storage,
    bucket: String,
}

impl S3MailAttachmentStore {
    #[must_use]
    pub fn new(store: SeaweedS3Storage, bucket: String) -> Self {
        Self { store, bucket }
    }
}

impl MailAttachmentStore for S3MailAttachmentStore {
    fn put<'a>(
        &'a self,
        key: String,
        content_type: String,
        bytes: Vec<u8>,
    ) -> MailFuture<'a, Result<(), MailServiceError>> {
        Box::pin(async move {
            self.store
                .put_object(self.bucket.clone(), key, content_type, bytes)
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "attachment_upload_failed",
                })
        })
    }

    fn presign_get<'a>(&'a self, key: &'a str) -> MailFuture<'a, Result<String, MailServiceError>> {
        Box::pin(async move {
            self.store
                .presign_get(PresignGetRequest {
                    bucket: self.bucket.clone(),
                    key: key.to_owned(),
                    expires_in: ATTACHMENT_GET_TTL,
                })
                .await
                .map_err(|_| MailServiceError::Transport {
                    code: "attachment_presign_failed",
                })
        })
    }
}

/// Spawn the inbound sync worker. Returns `None` (a no-op, app boots normally)
/// when the feature is not fully configured. The worker is GRACEFUL: missing KEK
/// / storage / disabled flag → not spawned; a failing pass logs a non-secret code
/// and the loop continues.
#[must_use]
pub fn spawn(
    pool: sqlx::PgPool,
    cipher: Option<Arc<EnvelopeCredentialCipher>>,
    storage: Option<(SeaweedS3Storage, String)>,
    enabled: bool,
) -> Option<MailSyncHandle> {
    if !enabled {
        tracing::info!("CONSOLE_MAIL_ENABLED is not set; the inbound webmail sync worker is OFF");
        return None;
    }
    let Some(cipher) = cipher else {
        tracing::info!(
            "CONSOLE_MAIL_MASTER_KEY absent; the inbound webmail sync worker is OFF (no credential cipher)"
        );
        return None;
    };
    let Some((object_store, bucket)) = storage else {
        tracing::info!("object storage is not configured; the inbound webmail sync worker is OFF");
        return None;
    };

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let attachments = S3MailAttachmentStore::new(object_store, bucket);
    let store = PgMailStore::new(pool);

    tokio::spawn(async move {
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_SYNCS));
        let mut ticker = tokio::time::interval(Duration::from_secs(DEFAULT_TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            tick_secs = DEFAULT_TICK_SECS,
            "inbound webmail sync worker started"
        );

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        tracing::info!("inbound webmail sync worker stopping");
                        break;
                    }
                }
                _ = ticker.tick() => {
                    run_tick(&store, &attachments, &cipher, &semaphore).await;
                }
            }
        }
    });

    Some(MailSyncHandle { shutdown_tx })
}

/// One scheduler tick. Acquires the currently-FREE concurrency permits FIRST,
/// then claims EXACTLY that many due accounts (cross-tenant, id-only) — never
/// more. Claiming ahead of capacity used to stamp a 600s lease on accounts that
/// then sat queued behind the semaphore before their pass even started, since
/// `SYNC_ACCOUNT_TIMEOUT` only starts counting once the pass runs; a long queue
/// wait could burn most of the lease before syncing began, letting another
/// worker reclaim an account that was in fact still (about to be) worked. By
/// claiming only as many accounts as we can start immediately, every claimed
/// pass begins right away and the lease comfortably covers the ≤480s timeout.
/// (This trades tick throughput — at most `MAX_CONCURRENT_SYNCS` claims per
/// tick instead of up to 100 — for correctness; raise `MAX_CONCURRENT_SYNCS` if
/// more throughput is needed.)
async fn run_tick(
    store: &PgMailStore,
    attachments: &S3MailAttachmentStore,
    cipher: &Arc<EnvelopeCredentialCipher>,
    semaphore: &Arc<Semaphore>,
) {
    let mut permits = Vec::new();
    while let Ok(permit) = Arc::clone(semaphore).try_acquire_owned() {
        permits.push(permit);
    }
    if permits.is_empty() {
        // Fully busy with a prior tick's (long-running) passes; try next tick.
        return;
    }

    let now = Timestamp::now_utc();
    let due = match store.list_due_accounts(now, permits.len() as i32).await {
        Ok(due) => due,
        Err(err) => {
            tracing::warn!(
                code = err.transport_code(),
                "mail sync: enumerate due accounts failed"
            );
            return;
        }
    };
    if due.is_empty() {
        return;
    }
    tracing::debug!(due = due.len(), "mail sync tick: dispatching due accounts");

    let mut handles = Vec::new();
    // due.len() <= permits.len() (the claim LIMIT was permits.len()), so every
    // claimed account gets an already-held permit and starts immediately; any
    // surplus permits are simply dropped (released) when this loop ends.
    for (account, permit) in due.into_iter().zip(permits) {
        let store = store.clone();
        let attachments = attachments.clone();
        let cipher = Arc::clone(cipher);
        handles.push(tokio::spawn(async move {
            // Hold the permit for the whole pass; dropped on completion.
            let _permit = permit;
            sync_one_account(store, attachments, cipher, account).await;
        }));
    }
    for handle in handles {
        let _ = handle.await;
    }
}

/// Run ONE account's sync pass under its tenant's armed org. This is the critical
/// RLS-in-a-background-loop step: `scope_org` enters the tenant scope so the
/// account re-read + every sync write is RLS-armed to exactly this org; a pass
/// for org A can therefore never touch org B's rows. The IMAP password is
/// decrypted only inside `SyncService` and dropped when the pass ends.
async fn sync_one_account(
    store: PgMailStore,
    attachments: S3MailAttachmentStore,
    cipher: Arc<EnvelopeCredentialCipher>,
    account: console_comms_application::DueAccount,
) {
    let org = account.org_id;
    let token = account.claim_token;
    let result = scope_org(org, async move {
        // Re-read the full sealed account under the armed org (the enumeration
        // gave us only ids). If it vanished/changed since enumeration, skip — but
        // RELEASE the claim we just took (fenced by our token) so the account is
        // not stranded leased until the timeout, stalling throughput. No sync
        // attempt was actually made, so `release_claim` — NOT
        // `record_sync_result` — clears the lease without stamping the lifecycle
        // (last_sync_at/sync_status/error), which would otherwise misreport an
        // unsynced account as a successful pass.
        let Some(stored) = store.get_account().await? else {
            let _ = store.release_claim(org, account.account_id, token).await;
            return Ok::<_, MailServiceError>(None);
        };
        if stored.id != account.account_id {
            // The tenant's single mailbox changed identity since enumeration;
            // skip this stale dispatch rather than syncing the wrong account.
            let _ = store.release_claim(org, account.account_id, token).await;
            return Ok(None);
        }
        // Keep a handle to release the claim if the pass is aborted by the timeout
        // below (the pass owns `store`, so we clone before moving it in).
        let release_store = store.clone();
        // Owned values satisfy the SyncService bounds directly; the cipher is
        // shared via the Arc blanket `CredentialCipher` impl.
        let service = SyncService::new(store, AsyncImapClient::new(), attachments, cipher);
        // BOUND the pass below the claim lease: a hung/slow sync is aborted before
        // another worker could reclaim the account at lease expiry (overlapping
        // sync). On timeout the pass's own lifecycle stamp never ran, so release
        // the lease here (fenced) and surface a timeout error.
        match tokio::time::timeout(SYNC_ACCOUNT_TIMEOUT, service.sync_account(&stored, token)).await
        {
            Ok(inner) => inner.map(Some),
            Err(_elapsed) => {
                let _ = release_store
                    .record_sync_result(
                        org,
                        account.account_id,
                        token,
                        "UNREACHABLE",
                        Some("sync_timeout"),
                    )
                    .await;
                Err(MailServiceError::Transport {
                    code: "sync_timeout",
                })
            }
        }
    })
    .await;

    match result {
        Ok(Some(outcome)) => {
            tracing::info!(
                org = %org,
                account = %account.account_id,
                folders = outcome.folders_synced,
                upserted = outcome.messages_upserted,
                duplicates = outcome.messages_skipped_duplicate,
                "mail sync pass complete"
            );
        }
        Ok(None) => {}
        Err(err) => {
            // The lifecycle status was already stamped inside sync_account; here we
            // log only the non-secret code + ids.
            tracing::warn!(
                org = %org,
                account = %account.account_id,
                code = err.transport_code(),
                "mail sync pass failed"
            );
        }
    }
}
