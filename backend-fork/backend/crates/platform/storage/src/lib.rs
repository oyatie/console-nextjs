//! Evidence object storage.
//!
//! T1.4 owns the S3 port, SeaweedFS-compatible adapter, evidence media rows,
//! and WORM replica verification state used by the work-order completion
//! interlock.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration as StdDuration;

use console_kernel_core::{
    AuditAction, AuditEvent, BranchId, EvidenceId, KernelError, OrgId, Timestamp, TraceContext,
    UserId, WorkOrderId,
};
use console_platform_db::{DbError, with_audit, with_org_conn};
use console_platform_request_context::current_org;
use console_workorder_domain::AttachmentStage;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use url::Url;

pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Domain(#[from] KernelError),

    #[error("S3 operation failed: {0}")]
    S3(String),

    #[error("presigning failed: {0}")]
    Presign(String),

    #[error("replica verification failed: {0}")]
    Verification(String),

    #[error("media processing failed: {0}")]
    Processing(String),
}

impl From<sqlx::Error> for StorageError {
    fn from(value: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(value))
    }
}

macro_rules! storage_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl $name {
            #[must_use]
            pub const fn as_db_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub fn from_db_str(value: &str) -> Result<Self, KernelError> {
                match value {
                    $($wire => Ok(Self::$variant),)+
                    other => Err(KernelError::validation(format!(
                        "unknown {} value {other:?}",
                        stringify!($name)
                    ))),
                }
            }
        }
    };
}

storage_enum! {
    pub enum WormReplicaStatus {
        Pending => "PENDING",
        Verified => "VERIFIED",
        Failed => "FAILED",
    }
}

storage_enum! {
    /// Server-side media-processing lifecycle for an evidence row.
    ///
    /// `PROCESSING` — the mechanic's ORIGINAL sits at `staging_s3_key`; a
    /// transcode job is queued. `READY` — the optimized 1080p/recompressed
    /// artifact is at `s3_key` (+ `thumbnail_s3_key`) and the staging original
    /// has been deleted. `FAILED` — processing errored; the staging original is
    /// retained for retry and `processing_error` records the cause.
    pub enum ProcessingStatus {
        Processing => "PROCESSING",
        Ready => "READY",
        Failed => "FAILED",
    }
}

/// Coarse media class derived from the ORIGINAL upload's content type. Selects
/// the processing pipeline (ffmpeg transcode vs. image recompress) and the
/// per-kind size cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    /// Maximum accepted ORIGINAL upload size, per kind. Video <= 200 MiB,
    /// image <= 25 MiB. Caps the authz-gated storage-exhaustion / cost
    /// amplification vector before any presigned URL is issued.
    #[must_use]
    pub const fn max_upload_bytes(self) -> i64 {
        match self {
            Self::Image => 25 * 1024 * 1024,
            Self::Video => 200 * 1024 * 1024,
        }
    }

    /// Classify a request's media type. Returns `None` for any content type
    /// outside the evidence allowlist so the caller rejects it before presign.
    #[must_use]
    pub fn from_content_type(content_type: &str) -> Option<Self> {
        let media_type = normalize_media_type(content_type);
        if ALLOWED_EVIDENCE_IMAGE_TYPES.contains(&media_type.as_str()) {
            Some(Self::Image)
        } else if ALLOWED_EVIDENCE_VIDEO_TYPES.contains(&media_type.as_str()) {
            Some(Self::Video)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct S3StorageConfig {
    pub endpoint_url: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub primary_bucket: String,
    pub replica_bucket: String,
    pub force_path_style: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignPutRequest {
    pub bucket: String,
    pub key: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: Option<String>,
    pub expires_in: StdDuration,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PresignedUpload {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub expires_in_secs: u64,
}

/// Inputs for a short-lived presigned GET URL. Used to hand the web UI a
/// time-boxed link to a thumbnail without ever exposing the raw object key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresignGetRequest {
    pub bucket: String,
    pub key: String,
    pub expires_in: StdDuration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutObjectResult {
    pub version_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyObjectRequest {
    pub source_bucket: String,
    pub source_key: String,
    pub destination_bucket: String,
    pub destination_key: String,
    pub retain_until: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHead {
    pub size_bytes: i64,
    pub e_tag: Option<String>,
    pub checksum_sha256: Option<String>,
    pub object_lock_mode: Option<String>,
    pub retain_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionInfo {
    pub mode: Option<String>,
    pub retain_until: Option<String>,
}

pub trait S3ObjectStore: Send + Sync {
    fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload>;

    /// Issue a short-lived presigned GET URL (used to serve a thumbnail to the
    /// web UI without exposing the raw object key).
    fn presign_get(&self, request: PresignGetRequest) -> StorageFuture<'_, String>;

    fn copy_object(&self, request: CopyObjectRequest) -> StorageFuture<'_, ()>;

    fn head_object(&self, bucket: String, key: String) -> StorageFuture<'_, ObjectHead>;

    fn get_object_retention(&self, bucket: String, key: String)
    -> StorageFuture<'_, RetentionInfo>;

    /// Download an object's bytes (used by the transcode worker to fetch the
    /// staging original it must process).
    fn get_object(&self, bucket: String, key: String) -> StorageFuture<'_, Vec<u8>>;

    /// Upload bytes (the optimized artifact / thumbnail the worker produces).
    fn put_object(
        &self,
        bucket: String,
        key: String,
        content_type: String,
        body: Vec<u8>,
    ) -> StorageFuture<'_, ()>;

    /// Delete an object (the staging original, after a successful transcode).
    fn delete_object(&self, bucket: String, key: String) -> StorageFuture<'_, ()>;
}

#[derive(Debug, Clone)]
pub struct SeaweedS3Storage {
    client: reqwest::Client,
    endpoint_url: Url,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    force_path_style: bool,
}

impl SeaweedS3Storage {
    /// Build an S3-compatible HTTP client for SeaweedFS path-style buckets.
    pub async fn from_config(config: &S3StorageConfig) -> Result<Self, StorageError> {
        let endpoint_url = Url::parse(&config.endpoint_url)
            .map_err(|err| StorageError::S3(format!("invalid S3 endpoint URL: {err}")))?;
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| StorageError::S3("could not create object client".into()))?,
            endpoint_url,
            region: config.region.clone(),
            access_key_id: config.access_key_id.clone(),
            secret_access_key: config.secret_access_key.clone(),
            force_path_style: config.force_path_style,
        })
    }

    #[must_use]
    pub fn from_parts(
        client: reqwest::Client,
        endpoint_url: Url,
        region: String,
        access_key_id: String,
        secret_access_key: String,
        force_path_style: bool,
    ) -> Self {
        Self {
            client,
            endpoint_url,
            region,
            access_key_id,
            secret_access_key,
            force_path_style,
        }
    }

    pub async fn create_bucket(&self, bucket: &str, object_lock: bool) -> Result<(), StorageError> {
        let mut headers = HeaderMap::new();
        if object_lock {
            headers.insert(
                HeaderName::from_static("x-amz-bucket-object-lock-enabled"),
                HeaderValue::from_static("true"),
            );
        }
        let response = self
            .client
            .put(self.bucket_url(bucket)?)
            .headers(headers)
            .send()
            .await
            .map_err(reqwest_error)?;
        if response.status().is_success()
            || response.status().as_u16() == 409
            || response.status().as_u16() == 412
        {
            if object_lock {
                self.enable_bucket_versioning(bucket).await?;
                self.enable_bucket_object_lock(bucket).await?;
            }
            Ok(())
        } else {
            Err(s3_response_error("create bucket", response).await)
        }
    }

    pub async fn put_bytes(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<(), StorageError> {
        self.put_bytes_with_result(bucket, key, content_type, body)
            .await
            .map(|_| ())
    }

    pub async fn put_bytes_with_result(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<PutObjectResult, StorageError> {
        let response = self
            .client
            .put(self.object_url(bucket, key)?)
            .header(CONTENT_TYPE, content_type)
            .body(body)
            .send()
            .await
            .map_err(reqwest_error)?;
        if !response.status().is_success() {
            return Err(s3_response_error("put object", response).await);
        }
        Ok(PutObjectResult {
            version_id: header_string(response.headers(), "x-amz-version-id"),
        })
    }

    pub async fn put_compliance_retention(
        &self,
        bucket: &str,
        key: &str,
        retain_until: Timestamp,
    ) -> Result<(), StorageError> {
        let retain_until = retain_until
            .format(&Rfc3339)
            .map_err(|err| StorageError::S3(format!("invalid retain-until timestamp: {err}")))?;
        let response = self
            .client
            .put(self.object_subresource_url(bucket, key, "retention")?)
            .header(CONTENT_TYPE, "application/xml")
            .body(retention_xml(&retain_until))
            .send()
            .await
            .map_err(reqwest_error)?;
        ensure_success("put object retention", response).await
    }

    /// Fetch an object's bytes and content type for internal processing. The
    /// caller must enforce its own input-size limit before using this method.
    pub async fn get_bytes(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<(Vec<u8>, Option<String>), StorageError> {
        let response = self.signed_get_response(bucket, key).await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(StorageError::S3(format!(
                "get object failed with {}",
                response.status()
            )));
        }
        let content_type = header_string(response.headers(), "content-type");
        let bytes = response
            .bytes()
            .await
            .map_err(|_| StorageError::S3("object body fetch failed".into()))?;
        Ok((bytes.to_vec(), content_type))
    }

    async fn signed_get_response(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<reqwest::Response, StorageError> {
        let url = self.presign_get_url(PresignGetRequest {
            bucket: bucket.to_owned(),
            key: key.to_owned(),
            expires_in: StdDuration::from_secs(60),
        })?;
        tokio::time::timeout(StdDuration::from_secs(15), self.client.get(url).send())
            .await
            .map_err(|_| StorageError::S3("object fetch timed out".into()))?
            .map_err(|_| StorageError::S3("object fetch failed".into()))
    }

    /// Forward a public object without buffering it in the server. The response
    /// body is bounded even when the object store omits or lies about its length.
    pub async fn get_bounded_stream(
        &self,
        bucket: &str,
        key: &str,
        max_bytes: u64,
    ) -> Result<
        Option<(
            impl futures::Stream<Item = Result<bytes::Bytes, StorageError>> + Send + 'static,
            Option<String>,
        )>,
        StorageError,
    > {
        let response = self.signed_get_response(bucket, key).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(StorageError::S3(format!(
                "get object failed with {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes)
        {
            return Err(StorageError::S3("object exceeds public media limit".into()));
        }
        let content_type = header_string(response.headers(), "content-type");
        // Drain under an independent deadline. A slow downstream must not keep
        // the S3 response open by simply not polling the returned stream.
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let (done_sender, done_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = tokio::time::timeout(StdDuration::from_secs(60), async {
                let mut response = response;
                let mut read = 0_u64;
                loop {
                    let Some(chunk) =
                        tokio::time::timeout(StdDuration::from_secs(15), response.chunk())
                            .await
                            .map_err(|_| StorageError::S3("object body stalled".into()))?
                            .map_err(|_| StorageError::S3("object body fetch failed".into()))?
                    else {
                        return Ok::<(), StorageError>(());
                    };
                    read = bounded_length(read, chunk.len(), max_bytes)?;
                    if sender.send(chunk).await.is_err() {
                        return Ok(());
                    }
                }
            })
            .await;
            let _ = done_sender.send(match result {
                Ok(result) => result,
                Err(_) => Err(StorageError::S3("object body timed out".into())),
            });
        });
        let stream = futures::stream::unfold(
            (receiver, Some(done_receiver)),
            |(mut receiver, mut done)| async move {
                if let Some(chunk) = receiver.recv().await {
                    return Some((Ok(chunk), (receiver, done)));
                }
                match done.take()?.await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some((Err(error), (receiver, done))),
                    Err(_) => Some((
                        Err(StorageError::S3("object stream worker failed".into())),
                        (receiver, done),
                    )),
                }
            },
        );
        Ok(Some((stream, content_type)))
    }

    pub async fn delete_object(&self, bucket: &str, key: &str) -> Result<(), StorageError> {
        let response = self
            .client
            .delete(self.object_url(bucket, key)?)
            .send()
            .await
            .map_err(reqwest_error)?;
        ensure_success("delete object", response).await
    }

    pub async fn delete_object_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Result<(), StorageError> {
        let response = self
            .client
            .delete(self.object_version_url(bucket, key, version_id)?)
            .send()
            .await
            .map_err(reqwest_error)?;
        ensure_success("delete object version", response).await
    }

    pub async fn head_object_version(
        &self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Result<ObjectHead, StorageError> {
        let response = self
            .client
            .head(self.object_version_url(bucket, key, version_id)?)
            .send()
            .await
            .map_err(reqwest_error)?;
        if !response.status().is_success() {
            return Err(s3_response_error("head object version", response).await);
        }
        let headers = response.headers();
        Ok(ObjectHead {
            size_bytes: header_i64(headers, &CONTENT_LENGTH).unwrap_or_default(),
            e_tag: header_string(headers, "etag"),
            checksum_sha256: header_string(headers, "x-amz-checksum-sha256"),
            object_lock_mode: header_string(headers, "x-amz-object-lock-mode"),
            retain_until: header_string(headers, "x-amz-object-lock-retain-until-date"),
        })
    }

    fn presign_put_url(&self, request: PresignPutRequest) -> Result<PresignedUpload, StorageError> {
        let expires_in_secs = request.expires_in.as_secs();
        let mut url = self.object_url(&request.bucket, &request.key)?;
        let host = host_header(&url)?;
        let now = OffsetDateTime::now_utc();
        let date = sigv4_date(now);
        let amz_date = sigv4_timestamp(now);
        let credential_scope = format!("{}/{}/s3/aws4_request", date, self.region);
        let credential = format!("{}/{}", self.access_key_id, credential_scope);

        let mut signed_headers = vec![
            ("content-length".to_owned(), request.size_bytes.to_string()),
            ("content-type".to_owned(), request.content_type.clone()),
            ("host".to_owned(), host),
        ];
        if let Some(checksum) = request.checksum_sha256.as_deref() {
            signed_headers.push(("x-amz-checksum-sha256".to_owned(), checksum.to_owned()));
        }
        signed_headers.sort_by(|left, right| left.0.cmp(&right.0));
        let signed_header_names = signed_headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("X-Amz-Algorithm", "AWS4-HMAC-SHA256");
            query.append_pair("X-Amz-Credential", &credential);
            query.append_pair("X-Amz-Date", &amz_date);
            query.append_pair("X-Amz-Expires", &expires_in_secs.to_string());
            query.append_pair("X-Amz-SignedHeaders", &signed_header_names);
        }

        let canonical_headers = signed_headers
            .iter()
            .map(|(name, value)| format!("{name}:{}\n", value.trim()))
            .collect::<String>();
        let canonical_request = format!(
            "PUT\n{}\n{}\n{}\n{}\nUNSIGNED-PAYLOAD",
            url.path(),
            url.query().unwrap_or_default(),
            canonical_headers,
            signed_header_names
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            sha256_hex(canonical_request.as_bytes())
        );
        let signature = sigv4_signature(
            &self.secret_access_key,
            &date,
            &self.region,
            "s3",
            &string_to_sign,
        )?;
        url.query_pairs_mut()
            .append_pair("X-Amz-Signature", &signature);

        let mut headers = vec![
            ("content-length".to_owned(), request.size_bytes.to_string()),
            ("content-type".to_owned(), request.content_type),
        ];
        if let Some(checksum) = request.checksum_sha256 {
            headers.push(("x-amz-checksum-sha256".to_owned(), checksum));
        }
        Ok(PresignedUpload {
            method: "PUT".to_owned(),
            url: url.to_string(),
            headers,
            expires_in_secs,
        })
    }

    /// SigV4-presign a GET for `request.key`. Only `host` is signed (a GET has
    /// no body / content headers), so the returned URL is directly fetchable by
    /// a browser within the expiry window. Used to serve evidence thumbnails
    /// without ever exposing the raw object key to the client.
    fn presign_get_url(&self, request: PresignGetRequest) -> Result<String, StorageError> {
        let expires_in_secs = request.expires_in.as_secs();
        let mut url = self.object_url(&request.bucket, &request.key)?;
        let host = host_header(&url)?;
        let now = OffsetDateTime::now_utc();
        let date = sigv4_date(now);
        let amz_date = sigv4_timestamp(now);
        let credential_scope = format!("{}/{}/s3/aws4_request", date, self.region);
        let credential = format!("{}/{}", self.access_key_id, credential_scope);

        let signed_header_names = "host";

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("X-Amz-Algorithm", "AWS4-HMAC-SHA256");
            query.append_pair("X-Amz-Credential", &credential);
            query.append_pair("X-Amz-Date", &amz_date);
            query.append_pair("X-Amz-Expires", &expires_in_secs.to_string());
            query.append_pair("X-Amz-SignedHeaders", signed_header_names);
        }

        let canonical_headers = format!("host:{}\n", host.trim());
        let canonical_request = format!(
            "GET\n{}\n{}\n{}\n{}\nUNSIGNED-PAYLOAD",
            url.path(),
            url.query().unwrap_or_default(),
            canonical_headers,
            signed_header_names
        );
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            credential_scope,
            sha256_hex(canonical_request.as_bytes())
        );
        let signature = sigv4_signature(
            &self.secret_access_key,
            &date,
            &self.region,
            "s3",
            &string_to_sign,
        )?;
        url.query_pairs_mut()
            .append_pair("X-Amz-Signature", &signature);
        Ok(url.to_string())
    }

    fn bucket_url(&self, bucket: &str) -> Result<Url, StorageError> {
        self.path_style_url(bucket, None)
    }

    fn bucket_subresource_url(&self, bucket: &str, subresource: &str) -> Result<Url, StorageError> {
        let mut url = self.bucket_url(bucket)?;
        url.query_pairs_mut().append_pair(subresource, "");
        Ok(url)
    }

    fn object_url(&self, bucket: &str, key: &str) -> Result<Url, StorageError> {
        self.path_style_url(bucket, Some(key))
    }

    fn object_subresource_url(
        &self,
        bucket: &str,
        key: &str,
        subresource: &str,
    ) -> Result<Url, StorageError> {
        let mut url = self.object_url(bucket, key)?;
        url.query_pairs_mut().append_pair(subresource, "");
        Ok(url)
    }

    fn object_version_url(
        &self,
        bucket: &str,
        key: &str,
        version_id: &str,
    ) -> Result<Url, StorageError> {
        let mut url = self.object_url(bucket, key)?;
        url.query_pairs_mut().append_pair("versionId", version_id);
        Ok(url)
    }

    fn path_style_url(&self, bucket: &str, key: Option<&str>) -> Result<Url, StorageError> {
        if !self.force_path_style {
            return Err(StorageError::S3(
                "virtual-hosted S3 URLs are not supported by the SeaweedFS adapter".to_owned(),
            ));
        }
        let mut url = self.endpoint_url.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                StorageError::S3("S3 endpoint URL cannot be used as a base".to_owned())
            })?;
            segments.clear();
            segments.push(bucket);
            if let Some(key) = key {
                for segment in key.split('/') {
                    segments.push(segment);
                }
            }
        }
        Ok(url)
    }

    async fn enable_bucket_versioning(&self, bucket: &str) -> Result<(), StorageError> {
        let response = self
            .client
            .put(self.bucket_subresource_url(bucket, "versioning")?)
            .header(CONTENT_TYPE, "application/xml")
            .body("<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>")
            .send()
            .await
            .map_err(reqwest_error)?;
        ensure_success("enable bucket versioning", response).await
    }

    async fn enable_bucket_object_lock(&self, bucket: &str) -> Result<(), StorageError> {
        let response = self
            .client
            .put(self.bucket_subresource_url(bucket, "object-lock")?)
            .header(CONTENT_TYPE, "application/xml")
            .body(
                "<ObjectLockConfiguration><ObjectLockEnabled>Enabled</ObjectLockEnabled></ObjectLockConfiguration>",
            )
            .send()
            .await
            .map_err(reqwest_error)?;
        ensure_success("enable bucket object lock", response).await
    }
}

fn bounded_length(read: u64, chunk_len: usize, max_bytes: u64) -> Result<u64, StorageError> {
    let next = read
        .checked_add(chunk_len as u64)
        .ok_or_else(|| StorageError::S3("object exceeds public media limit".into()))?;
    if next > max_bytes {
        return Err(StorageError::S3("object exceeds public media limit".into()));
    }
    Ok(next)
}

#[cfg(test)]
mod bounded_stream_tests {
    use super::{SeaweedS3Storage, bounded_length};
    use futures::StreamExt;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;
    use url::Url;

    #[test]
    fn rejects_unannounced_oversize_chunks() {
        assert_eq!(bounded_length(4, 3, 7).unwrap(), 7);
        assert!(bounded_length(4, 4, 7).is_err());
        assert!(bounded_length(u64::MAX, 1, u64::MAX).is_err());
    }

    #[tokio::test]
    async fn signed_private_get_streams_and_stops_after_unannounced_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (continue_tx, continue_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0_u8; 8192];
            let len = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..len]);
            assert!(request.starts_with("GET /photos/photo.png?"));
            assert!(request.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
            assert!(request.contains("X-Amz-Signature="));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n").unwrap();
            continue_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            socket.write_all(b"4\r\nefgh\r\n0\r\n\r\n").unwrap();
        });
        let storage = SeaweedS3Storage::from_parts(
            reqwest::Client::new(),
            endpoint,
            "ap-northeast-2".into(),
            "test-access".into(),
            "test-secret".into(),
            true,
        );
        let (stream, content_type) = storage
            .get_bounded_stream("photos", "photo.png", 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(content_type.as_deref(), Some("image/png"));
        futures::pin_mut!(stream);
        assert_eq!(stream.next().await.unwrap().unwrap(), b"abcd"[..]);
        continue_tx.send(()).unwrap();
        assert!(stream.next().await.unwrap().is_err());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn missing_object_is_distinct_from_unavailable_storage() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0_u8; 8192];
            let len = socket.read(&mut request).unwrap();
            assert!(request[..len].starts_with(b"GET /photos/missing.png?"));
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        let storage = SeaweedS3Storage::from_parts(
            reqwest::Client::new(),
            endpoint,
            "ap-northeast-2".into(),
            "test-access".into(),
            "test-secret".into(),
            true,
        );
        assert!(
            storage
                .get_bounded_stream("photos", "missing.png", 7)
                .await
                .unwrap()
                .is_none()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn partial_object_is_not_reported_as_a_complete_image() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0_u8; 8192];
            let len = socket.read(&mut request).unwrap();
            assert!(request[..len].starts_with(b"GET /photos/partial.png?"));
            socket.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Type: image/png\r\nContent-Length: 1\r\n\r\na").unwrap();
        });
        let storage = SeaweedS3Storage::from_parts(
            reqwest::Client::new(),
            endpoint,
            "ap-northeast-2".into(),
            "test-access".into(),
            "test-secret".into(),
            true,
        );
        assert!(
            storage
                .get_bounded_stream("photos", "partial.png", 7)
                .await
                .is_err()
        );
        server.join().unwrap();
    }
}

impl S3ObjectStore for SeaweedS3Storage {
    fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
        Box::pin(async move { self.presign_put_url(request) })
    }

    fn presign_get(&self, request: PresignGetRequest) -> StorageFuture<'_, String> {
        Box::pin(async move { self.presign_get_url(request) })
    }

    fn copy_object(&self, request: CopyObjectRequest) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                HeaderName::from_static("x-amz-copy-source"),
                header_value(&format!(
                    "/{}/{}",
                    request.source_bucket, request.source_key
                ))?,
            );
            if let Some(retain_until) = request.retain_until {
                headers.insert(
                    HeaderName::from_static("x-amz-object-lock-mode"),
                    HeaderValue::from_static("COMPLIANCE"),
                );
                headers.insert(
                    HeaderName::from_static("x-amz-object-lock-retain-until-date"),
                    header_value(&retain_until.format(&Rfc3339).map_err(|err| {
                        StorageError::S3(format!("invalid retain-until timestamp: {err}"))
                    })?)?,
                );
            }
            let response = self
                .client
                .put(self.object_url(&request.destination_bucket, &request.destination_key)?)
                .headers(headers)
                .send()
                .await
                .map_err(reqwest_error)?;
            ensure_success("copy object", response).await
        })
    }

    fn head_object(&self, bucket: String, key: String) -> StorageFuture<'_, ObjectHead> {
        Box::pin(async move {
            let response = self
                .client
                .head(self.object_url(&bucket, &key)?)
                .send()
                .await
                .map_err(reqwest_error)?;
            if !response.status().is_success() {
                return Err(s3_response_error("head object", response).await);
            }
            let headers = response.headers();
            Ok(ObjectHead {
                size_bytes: header_i64(headers, &CONTENT_LENGTH).unwrap_or_default(),
                e_tag: header_string(headers, "etag"),
                checksum_sha256: header_string(headers, "x-amz-checksum-sha256"),
                object_lock_mode: header_string(headers, "x-amz-object-lock-mode"),
                retain_until: header_string(headers, "x-amz-object-lock-retain-until-date"),
            })
        })
    }

    fn get_object_retention(
        &self,
        bucket: String,
        key: String,
    ) -> StorageFuture<'_, RetentionInfo> {
        Box::pin(async move {
            let response = self
                .client
                .get(self.object_subresource_url(&bucket, &key, "retention")?)
                .send()
                .await
                .map_err(reqwest_error)?;
            if !response.status().is_success() {
                return Err(s3_response_error("get object retention", response).await);
            }
            let body = response.text().await.map_err(reqwest_error)?;
            Ok(RetentionInfo {
                mode: xml_tag_text(&body, "Mode"),
                retain_until: xml_tag_text(&body, "RetainUntilDate"),
            })
        })
    }

    fn get_object(&self, bucket: String, key: String) -> StorageFuture<'_, Vec<u8>> {
        Box::pin(async move {
            let (bytes, _content_type) = self.get_bytes(&bucket, &key).await?;
            Ok(bytes)
        })
    }

    fn put_object(
        &self,
        bucket: String,
        key: String,
        content_type: String,
        body: Vec<u8>,
    ) -> StorageFuture<'_, ()> {
        Box::pin(async move { self.put_bytes(&bucket, &key, &content_type, body).await })
    }

    fn delete_object(&self, bucket: String, key: String) -> StorageFuture<'_, ()> {
        Box::pin(async move { SeaweedS3Storage::delete_object(self, &bucket, &key).await })
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceUploadCommand {
    pub actor: UserId,
    pub work_order_id: WorkOrderId,
    pub stage: AttachmentStage,
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// Command to begin a media-processing evidence upload: the mechanic PUTs the
/// ORIGINAL to a tenant-scoped STAGING key, and a `PROCESSING` evidence row is
/// created. A transcode job then optimizes the original into the final artifact.
#[derive(Debug, Clone)]
pub struct StagingUploadCommand {
    pub actor: UserId,
    pub work_order_id: WorkOrderId,
    pub stage: AttachmentStage,
    /// The ORIGINAL upload's content type (validated against the image/video
    /// allowlist; classified into a [`MediaKind`]).
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: Option<String>,
    pub trace: TraceContext,
    pub occurred_at: Timestamp,
}

/// The presigned STAGING upload ticket plus the freshly created `PROCESSING`
/// evidence row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StagingUploadTicket {
    pub media: EvidenceMedia,
    pub media_kind: MediaKind,
    pub upload: PresignedUpload,
}

/// A claimed media-processing job: the evidence row plus the resolved keys/kind
/// the worker needs to transcode it. Returned by
/// [`EvidenceService::claim_processing_job`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessingJob {
    pub media_id: EvidenceId,
    pub work_order_id: WorkOrderId,
    pub branch_id: BranchId,
    pub stage: AttachmentStage,
    pub media_kind: MediaKind,
    pub staging_key: String,
    pub final_key: String,
    pub thumbnail_key: String,
    pub size_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EvidenceMedia {
    pub id: EvidenceId,
    pub work_order_id: WorkOrderId,
    pub stage: AttachmentStage,
    pub s3_key: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub checksum_sha256: Option<String>,
    pub uploaded_by: UserId,
    pub worm_replica_status: WormReplicaStatus,
    pub retry_count: i32,
    pub next_retry_at: Timestamp,
    pub last_error: Option<String>,
    pub verified_at: Option<Timestamp>,
    pub upload_confirmed_at: Option<Timestamp>,
    pub confirmed_by: Option<UserId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub processing_status: ProcessingStatus,
    pub staging_s3_key: Option<String>,
    pub thumbnail_s3_key: Option<String>,
    pub original_content_type: Option<String>,
    pub processing_error: Option<String>,
    pub processed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EvidenceUploadTicket {
    pub media: EvidenceMedia,
    pub upload: PresignedUpload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationConfig {
    pub primary_bucket: String,
    pub replica_bucket: String,
    pub max_retries: i32,
    pub base_retry_delay: Duration,
    pub max_retry_delay: Duration,
    pub retention_period: Duration,
}

impl ReplicationConfig {
    #[must_use]
    pub fn local_test(primary_bucket: String, replica_bucket: String) -> Self {
        Self {
            primary_bucket,
            replica_bucket,
            max_retries: 3,
            base_retry_delay: Duration::seconds(1),
            max_retry_delay: Duration::seconds(30),
            retention_period: Duration::days(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationOutcome {
    pub media_id: EvidenceId,
    pub status: WormReplicaStatus,
    pub retry_count: i32,
}

#[derive(Debug, Clone)]
pub struct EvidenceService<S> {
    pool: PgPool,
    object_store: S,
    primary_bucket: String,
    replica_bucket: String,
    presign_expires_in: StdDuration,
    replication: ReplicationConfig,
}

impl<S> EvidenceService<S>
where
    S: S3ObjectStore,
{
    #[must_use]
    pub fn new(
        pool: PgPool,
        object_store: S,
        primary_bucket: String,
        replica_bucket: String,
    ) -> Self {
        Self {
            pool,
            object_store,
            primary_bucket: primary_bucket.clone(),
            replica_bucket: replica_bucket.clone(),
            presign_expires_in: StdDuration::from_secs(5 * 60),
            replication: ReplicationConfig::local_test(primary_bucket, replica_bucket),
        }
    }

    #[must_use]
    pub fn with_presign_expires_in(mut self, expires_in: StdDuration) -> Self {
        self.presign_expires_in = expires_in;
        self
    }

    #[must_use]
    pub fn with_replication_config(mut self, replication: ReplicationConfig) -> Self {
        self.replication = replication;
        self
    }

    pub async fn issue_presigned_upload(
        &self,
        command: EvidenceUploadCommand,
    ) -> Result<EvidenceUploadTicket, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        validate_upload_command(&command)?;
        let branch_id = branch_for_work_order(&self.pool, command.work_order_id).await?;
        let media_id = EvidenceId::new();
        let s3_key = evidence_s3_key(command.work_order_id, command.stage, media_id);
        let upload = self
            .object_store
            .presign_put(PresignPutRequest {
                bucket: self.primary_bucket.clone(),
                key: s3_key.clone(),
                content_type: command.content_type.clone(),
                size_bytes: command.size_bytes,
                checksum_sha256: command.checksum_sha256.clone(),
                expires_in: self.presign_expires_in,
            })
            .await?;
        let event = evidence_audit_event(
            "evidence.upload",
            Some(command.actor),
            branch_id,
            media_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let media = with_audit::<_, EvidenceMedia, StorageError>(&self.pool, event, |tx| {
            Box::pin(async move {
                // FIX 3: lock the parent work-order row and reject AFTER/REPORT
                // completion evidence once the work order is terminal, so the
                // WORM completion invariant cannot be invalidated post-closure.
                ensure_work_order_accepts_evidence_tx(tx, command.work_order_id, command.stage)
                    .await?;
                insert_evidence_media_tx(
                    tx,
                    NewEvidenceMedia {
                        media_id,
                        work_order_id: command.work_order_id,
                        stage: command.stage,
                        s3_key: &s3_key,
                        content_type: &command.content_type,
                        size_bytes: command.size_bytes,
                        checksum_sha256: command.checksum_sha256.as_deref(),
                        uploaded_by: command.actor,
                        occurred_at: command.occurred_at,
                    },
                    org_uuid,
                )
                .await
            })
        })
        .await?;
        Ok(EvidenceUploadTicket { media, upload })
    }

    /// Begin a media-processing evidence upload.
    ///
    /// Validates the original's MIME against the image/video allowlist and the
    /// per-kind size cap, presigns a PUT to a TENANT-PREFIXED staging key, then
    /// inserts a `PROCESSING` evidence row (RLS-armed via `with_audit` +
    /// `current_org()`). The row stamps the org-prefixed staging/final/thumbnail
    /// keys so the worker never has to recompute a tenant boundary.
    pub async fn issue_staging_upload(
        &self,
        command: StagingUploadCommand,
    ) -> Result<StagingUploadTicket, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let org_uuid = *org.as_uuid();
        let media_kind = validate_staging_command(&command)?;
        let branch_id = branch_for_work_order(&self.pool, command.work_order_id).await?;
        let media_id = EvidenceId::new();
        let staging_key = evidence_staging_key(
            org,
            command.work_order_id,
            command.stage,
            media_id,
            media_kind,
        );
        let final_key = evidence_final_key(
            org,
            command.work_order_id,
            command.stage,
            media_id,
            media_kind,
        );
        let upload = self
            .object_store
            .presign_put(PresignPutRequest {
                bucket: self.primary_bucket.clone(),
                key: staging_key.clone(),
                content_type: command.content_type.clone(),
                size_bytes: command.size_bytes,
                checksum_sha256: command.checksum_sha256.clone(),
                expires_in: self.presign_expires_in,
            })
            .await?;
        let event = evidence_audit_event(
            "evidence.staging",
            Some(command.actor),
            branch_id,
            media_id,
            command.trace,
            command.occurred_at,
        )?
        .with_org(org);
        let staging_for_insert = staging_key.clone();
        let final_for_insert = final_key.clone();
        let original_content_type = normalize_media_type(&command.content_type);
        let media = with_audit::<_, EvidenceMedia, StorageError>(&self.pool, event, |tx| {
            Box::pin(async move {
                // The completion-evidence terminal guard still applies: a
                // PROCESSING AFTER/REPORT row must not be opened on a closed WO.
                ensure_work_order_accepts_evidence_tx(tx, command.work_order_id, command.stage)
                    .await?;
                insert_processing_evidence_tx(
                    tx,
                    NewProcessingEvidence {
                        media_id,
                        work_order_id: command.work_order_id,
                        stage: command.stage,
                        // s3_key holds the FINAL deliverable key from the start;
                        // it only becomes populated in storage once READY.
                        final_key: &final_for_insert,
                        staging_key: &staging_for_insert,
                        original_content_type: &original_content_type,
                        size_bytes: command.size_bytes,
                        checksum_sha256: command.checksum_sha256.as_deref(),
                        uploaded_by: command.actor,
                        occurred_at: command.occurred_at,
                    },
                    org_uuid,
                )
                .await
            })
        })
        .await?;
        Ok(StagingUploadTicket {
            media,
            media_kind,
            upload,
        })
    }

    /// Claim the oldest still-`PROCESSING` evidence row for the armed tenant and
    /// resolve everything the worker needs to transcode it. Returns `None` when
    /// the tenant has no pending work. RLS-armed via `with_org_conn`.
    pub async fn claim_processing_job(&self) -> Result<Option<ProcessingJob>, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let media = match next_processing_media(&self.pool, org).await? {
            Some(media) => media,
            None => return Ok(None),
        };
        let media_kind = media
            .original_content_type
            .as_deref()
            .and_then(MediaKind::from_content_type)
            .ok_or_else(|| {
                StorageError::Processing(format!(
                    "evidence {} has no recognizable original content type",
                    media.id
                ))
            })?;
        let branch_id = branch_for_work_order(&self.pool, media.work_order_id).await?;
        let staging_key = media.staging_s3_key.clone().ok_or_else(|| {
            StorageError::Processing(format!("evidence {} has no staging key", media.id))
        })?;
        let thumbnail_key = evidence_thumbnail_key(org, media.work_order_id, media.stage, media.id);
        Ok(Some(ProcessingJob {
            media_id: media.id,
            work_order_id: media.work_order_id,
            branch_id,
            stage: media.stage,
            media_kind,
            staging_key,
            final_key: media.s3_key.clone(),
            thumbnail_key,
            size_bytes: media.size_bytes,
        }))
    }

    /// Run a claimed processing job end-to-end: download the staging original,
    /// transcode/optimize it (1080p H.264 video / recompressed image, EXIF
    /// stripped) + thumbnail via the [`MediaProcessor`], upload the artifacts to
    /// the tenant's FINAL keys, mark the row `READY`, and delete the staging
    /// original. On any error the row is marked `FAILED` (staging retained).
    pub async fn process_job<P: MediaProcessor>(
        &self,
        processor: &P,
        job: &ProcessingJob,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<ProcessingStatus, StorageError> {
        match self.run_processing(processor, job).await {
            Ok(content_type) => {
                self.mark_ready(job, &content_type, trace, occurred_at)
                    .await?;
                // Best-effort: delete the staging original now the deliverable
                // is durable. A leftover staging object is harmless (tenant
                // prefixed, lifecycle-expirable) and never the deliverable.
                let _ = self
                    .object_store
                    .delete_object(self.primary_bucket.clone(), job.staging_key.clone())
                    .await;
                Ok(ProcessingStatus::Ready)
            }
            Err(err) => {
                let message = err.to_string();
                tracing::error!(
                    media_id = %job.media_id,
                    work_order_id = %job.work_order_id,
                    error = %message,
                    "evidence media processing failed; staging original retained for retry"
                );
                self.mark_failed(job, message, trace, occurred_at).await?;
                Ok(ProcessingStatus::Failed)
            }
        }
    }

    async fn run_processing<P: MediaProcessor>(
        &self,
        processor: &P,
        job: &ProcessingJob,
    ) -> Result<String, StorageError> {
        // FIX (LOW): defensive size check BEFORE the in-memory download. Don't
        // trust the gateway to have honored the signed content-length: re-HEAD
        // the staging object and reject if it exceeds the per-kind cap (or the
        // declared row size). Caps memory the worker allocates for one job.
        let max_bytes = job.media_kind.max_upload_bytes();
        let declared = job.size_bytes;
        let head = self
            .object_store
            .head_object(self.primary_bucket.clone(), job.staging_key.clone())
            .await?;
        if head.size_bytes > max_bytes {
            return Err(StorageError::Processing(format!(
                "staging object is {} bytes, exceeding the {:?} maximum of {} bytes",
                head.size_bytes, job.media_kind, max_bytes
            )));
        }
        if declared >= 0 && head.size_bytes > declared {
            return Err(StorageError::Processing(format!(
                "staging object is {} bytes, exceeding the declared upload size of {} bytes",
                head.size_bytes, declared
            )));
        }

        let original = self
            .object_store
            .get_object(self.primary_bucket.clone(), job.staging_key.clone())
            .await?;

        // FIX (MEDIUM): the declared content_type is advisory — the real bytes
        // are authoritative. Sniff the magic number and reject anything whose
        // detected container/codec does not match the declared media kind (or is
        // outside the evidence allowlist) BEFORE handing it to ffmpeg.
        validate_sniffed_kind(&original, job.media_kind)?;

        let processed = processor.process(job.media_kind, original).await?;
        self.object_store
            .put_object(
                self.primary_bucket.clone(),
                job.final_key.clone(),
                processed.content_type.clone(),
                processed.artifact,
            )
            .await?;
        self.object_store
            .put_object(
                self.primary_bucket.clone(),
                job.thumbnail_key.clone(),
                "image/jpeg".to_owned(),
                processed.thumbnail,
            )
            .await?;
        Ok(processed.content_type)
    }

    async fn mark_ready(
        &self,
        job: &ProcessingJob,
        content_type: &str,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<(), StorageError> {
        // Arm the tenant so `with_audit` sets `app.current_org` for the status
        // UPDATE; without it the RLS policy filters the row out and the UPDATE
        // silently no-ops as `console_rt`.
        let org = current_org().map_err(KernelError::from)?;
        let event = evidence_audit_event(
            "evidence.process",
            None,
            job.branch_id,
            job.media_id,
            trace,
            occurred_at,
        )?
        .with_org(org);
        let media_id = job.media_id;
        let thumbnail_key = job.thumbnail_key.clone();
        let content_type = content_type.to_owned();
        with_audit::<_, (), StorageError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE evidence_media
                    SET processing_status = 'READY',
                        content_type = $2,
                        thumbnail_s3_key = $3,
                        staging_s3_key = NULL,
                        processing_error = NULL,
                        processed_at = $4,
                        updated_at = $4
                    WHERE id = $1
                    "#,
                )
                .bind(*media_id.as_uuid())
                .bind(content_type)
                .bind(thumbnail_key)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    async fn mark_failed(
        &self,
        job: &ProcessingJob,
        error: String,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<(), StorageError> {
        // Arm the tenant so the FAILED status UPDATE is RLS-visible as `console_rt`.
        let org = current_org().map_err(KernelError::from)?;
        let event = evidence_audit_event(
            "evidence.process",
            None,
            job.branch_id,
            job.media_id,
            trace,
            occurred_at,
        )?
        .with_org(org);
        let media_id = job.media_id;
        with_audit::<_, (), StorageError>(&self.pool, event, move |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE evidence_media
                    SET processing_status = 'FAILED',
                        processing_error = $2,
                        processed_at = $3,
                        updated_at = $3
                    WHERE id = $1
                    "#,
                )
                .bind(*media_id.as_uuid())
                .bind(error)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await
    }

    pub async fn evidence_media(
        &self,
        media_id: EvidenceId,
    ) -> Result<EvidenceMedia, StorageError> {
        evidence_media_by_id(&self.pool, media_id).await
    }

    /// Issue a short-lived presigned GET URL for an evidence row's generated
    /// thumbnail, served from the PRIMARY bucket. Returns `None` when the row has
    /// no thumbnail yet (still PROCESSING / FAILED). The raw object key is never
    /// returned to the client — only this time-boxed URL.
    pub async fn presigned_thumbnail_url(
        &self,
        media: &EvidenceMedia,
    ) -> Result<Option<String>, StorageError> {
        let Some(key) = media.thumbnail_s3_key.as_deref() else {
            return Ok(None);
        };
        let url = self
            .object_store
            .presign_get(PresignGetRequest {
                bucket: self.primary_bucket.clone(),
                key: key.to_owned(),
                expires_in: self.presign_expires_in,
            })
            .await?;
        Ok(Some(url))
    }

    pub async fn confirm_upload(
        &self,
        media_id: EvidenceId,
        actor: UserId,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<EvidenceMedia, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let media = evidence_media_by_id(&self.pool, media_id).await?;
        let branch_id = branch_for_work_order(&self.pool, media.work_order_id).await?;
        let event = evidence_audit_event(
            "evidence.confirm",
            Some(actor),
            branch_id,
            media_id,
            trace,
            occurred_at,
        )?
        .with_org(org);
        with_audit::<_, EvidenceMedia, StorageError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE evidence_media
                    SET upload_confirmed_at = COALESCE(upload_confirmed_at, $2),
                        confirmed_by = COALESCE(confirmed_by, $3),
                        updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(*media_id.as_uuid())
                .bind(occurred_at)
                .bind(*actor.as_uuid())
                .execute(tx.as_mut())
                .await?;
                evidence_media_by_id_tx(tx, media_id).await
            })
        })
        .await
    }

    pub async fn replicate_once(
        &self,
        media_id: EvidenceId,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<ReplicationOutcome, StorageError> {
        let media = evidence_media_by_id(&self.pool, media_id).await?;
        if media.worm_replica_status == WormReplicaStatus::Verified {
            return Ok(ReplicationOutcome {
                media_id,
                status: WormReplicaStatus::Verified,
                retry_count: media.retry_count,
            });
        }

        let result = self
            .copy_and_verify_replica(&media, occurred_at)
            .await
            .map(|()| WormReplicaStatus::Verified);
        match result {
            Ok(status) => self
                .record_replication_success(media, trace, occurred_at)
                .await
                .map(|media| ReplicationOutcome {
                    media_id,
                    status,
                    retry_count: media.retry_count,
                }),
            Err(err) => {
                let message = err.to_string();
                tracing::error!(
                    media_id = %media_id,
                    work_order_id = %media.work_order_id,
                    retry_count = media.retry_count + 1,
                    error = %message,
                    "evidence WORM replication attempt failed"
                );
                self.record_replication_failure(media, message, trace, occurred_at)
                    .await
            }
        }
    }

    async fn copy_and_verify_replica(
        &self,
        media: &EvidenceMedia,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        self.object_store
            .copy_object(CopyObjectRequest {
                source_bucket: self.primary_bucket.clone(),
                source_key: media.s3_key.clone(),
                destination_bucket: self.replica_bucket.clone(),
                destination_key: media.s3_key.clone(),
                retain_until: Some(now + self.replication.retention_period),
            })
            .await?;
        let head = self
            .object_store
            .head_object(self.replica_bucket.clone(), media.s3_key.clone())
            .await?;
        if head.size_bytes != media.size_bytes {
            return Err(StorageError::Verification(format!(
                "replica size mismatch: expected {}, got {}",
                media.size_bytes, head.size_bytes
            )));
        }
        if let (Some(expected), Some(actual)) = (
            media.checksum_sha256.as_deref(),
            head.checksum_sha256.as_deref(),
        ) && expected != actual
        {
            return Err(StorageError::Verification(
                "replica checksum_sha256 mismatch".to_owned(),
            ));
        }
        let retention = self
            .object_store
            .get_object_retention(self.replica_bucket.clone(), media.s3_key.clone())
            .await?;
        let retention_mode = retention.mode.or(head.object_lock_mode);
        if retention_mode.as_deref() != Some("COMPLIANCE") {
            return Err(StorageError::Verification(
                "replica object is not under COMPLIANCE retention".to_owned(),
            ));
        }
        let retain_until = retention.retain_until.or(head.retain_until);
        if retain_until.is_none() {
            return Err(StorageError::Verification(
                "replica object has no retain-until timestamp".to_owned(),
            ));
        }
        let _ = now;
        Ok(())
    }

    async fn record_replication_success(
        &self,
        media: EvidenceMedia,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<EvidenceMedia, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_id = branch_for_work_order(&self.pool, media.work_order_id).await?;
        let event = evidence_audit_event(
            "evidence.verify",
            None,
            branch_id,
            media.id,
            trace,
            occurred_at,
        )?
        .with_org(org);
        with_audit::<_, EvidenceMedia, StorageError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE evidence_media
                    SET worm_replica_status = 'VERIFIED',
                        verified_at = $2,
                        last_error = NULL,
                        next_retry_at = $2,
                        updated_at = $2
                    WHERE id = $1
                    "#,
                )
                .bind(*media.id.as_uuid())
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                evidence_media_by_id_tx(tx, media.id).await
            })
        })
        .await
    }

    async fn record_replication_failure(
        &self,
        media: EvidenceMedia,
        error: String,
        trace: TraceContext,
        occurred_at: Timestamp,
    ) -> Result<ReplicationOutcome, StorageError> {
        let org = current_org().map_err(KernelError::from)?;
        let branch_id = branch_for_work_order(&self.pool, media.work_order_id).await?;
        let next_retry_count = media.retry_count + 1;
        let next_status = if next_retry_count >= self.replication.max_retries {
            WormReplicaStatus::Failed
        } else {
            WormReplicaStatus::Pending
        };
        let next_retry_at = if next_status == WormReplicaStatus::Failed {
            occurred_at
        } else {
            occurred_at + retry_delay(next_retry_count, &self.replication)
        };
        if next_status == WormReplicaStatus::Failed {
            tracing::error!(
                media_id = %media.id,
                work_order_id = %media.work_order_id,
                retry_count = next_retry_count,
                "evidence WORM replication reached max retries and is visible in admin queue"
            );
        }
        let event = evidence_audit_event(
            "evidence.verify",
            None,
            branch_id,
            media.id,
            trace,
            occurred_at,
        )?
        .with_org(org);
        with_audit::<_, (), StorageError>(&self.pool, event, |tx| {
            Box::pin(async move {
                sqlx::query(
                    r#"
                    UPDATE evidence_media
                    SET worm_replica_status = $2,
                        retry_count = $3,
                        next_retry_at = $4,
                        last_error = $5,
                        updated_at = $6
                    WHERE id = $1
                    "#,
                )
                .bind(*media.id.as_uuid())
                .bind(next_status.as_db_str())
                .bind(next_retry_count)
                .bind(next_retry_at)
                .bind(error)
                .bind(occurred_at)
                .execute(tx.as_mut())
                .await?;
                Ok(())
            })
        })
        .await?;
        Ok(ReplicationOutcome {
            media_id: media.id,
            status: next_status,
            retry_count: next_retry_count,
        })
    }
}

pub fn evidence_audit_event(
    action: &str,
    actor: Option<UserId>,
    branch_id: BranchId,
    media_id: EvidenceId,
    trace: TraceContext,
    occurred_at: Timestamp,
) -> Result<AuditEvent, KernelError> {
    Ok(AuditEvent::new(
        actor,
        AuditAction::new(action)?,
        "evidence_media",
        media_id.to_string(),
        trace,
        occurred_at,
    )
    .with_branch(branch_id))
}

/// Legacy direct-upload evidence key: `work-orders/{wo}/{stage}/{media}`.
///
/// NOT org-prefixed — retained for the existing direct-upload (`issue_presigned_
/// upload`) flow whose isolation is enforced by the evidence_media RLS row + the
/// work_order_id composite FK. New media-processing uploads use the org-prefixed
/// staging/final/thumbnail keys below.
#[must_use]
pub fn evidence_s3_key(
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    media_id: EvidenceId,
) -> String {
    format!(
        "work-orders/{}/{}/{}",
        work_order_id,
        stage.as_db_str(),
        media_id
    )
}

/// Tenant-scoped STAGING key for the mechanic's raw original upload.
///
/// Every component is org-prefixed so a single shared bucket can never let one
/// tenant's presigned PUT or the worker's GET reach another tenant's object:
/// `orgs/{org}/work-orders/{wo}/{stage}/staging/{media}.{ext}`.
#[must_use]
pub fn evidence_staging_key(
    org: OrgId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    media_id: EvidenceId,
    kind: MediaKind,
) -> String {
    let ext = match kind {
        MediaKind::Image => "img",
        MediaKind::Video => "vid",
    };
    format!(
        "orgs/{}/work-orders/{}/{}/staging/{}.{}",
        org.as_uuid(),
        work_order_id,
        stage.as_db_str(),
        media_id,
        ext
    )
}

/// Tenant-scoped FINAL key for the optimized deliverable artifact.
/// `orgs/{org}/work-orders/{wo}/{stage}/{media}.{ext}`.
#[must_use]
pub fn evidence_final_key(
    org: OrgId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    media_id: EvidenceId,
    kind: MediaKind,
) -> String {
    let ext = match kind {
        MediaKind::Image => "jpg",
        MediaKind::Video => "mp4",
    };
    format!(
        "orgs/{}/work-orders/{}/{}/{}.{}",
        org.as_uuid(),
        work_order_id,
        stage.as_db_str(),
        media_id,
        ext
    )
}

/// Tenant-scoped FINAL key for the generated thumbnail / video poster.
/// `orgs/{org}/work-orders/{wo}/{stage}/{media}.thumb.jpg`.
#[must_use]
pub fn evidence_thumbnail_key(
    org: OrgId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    media_id: EvidenceId,
) -> String {
    format!(
        "orgs/{}/work-orders/{}/{}/{}.thumb.jpg",
        org.as_uuid(),
        work_order_id,
        stage.as_db_str(),
        media_id
    )
}

/// Maximum evidence object size accepted for the legacy direct-upload presign
/// (50 MiB). The media-processing pipeline applies the per-[`MediaKind`] caps
/// (`MediaKind::max_upload_bytes`) instead.
pub const MAX_EVIDENCE_SIZE_BYTES: i64 = 50 * 1024 * 1024;

/// Image content types accepted for media-processing evidence uploads. HEIC is
/// admitted because phones produce it; the worker recompresses it to JPEG.
pub const ALLOWED_EVIDENCE_IMAGE_TYPES: [&str; 4] =
    ["image/jpeg", "image/png", "image/webp", "image/heic"];

/// Video content types accepted for media-processing evidence uploads. All are
/// transcoded to H.264/AAC MP4 by the worker.
pub const ALLOWED_EVIDENCE_VIDEO_TYPES: [&str; 3] = ["video/mp4", "video/quicktime", "video/webm"];

/// Content types accepted for the legacy direct-upload evidence flow. Matched
/// case-insensitively against the media type (parameters after `;` are ignored).
pub const ALLOWED_EVIDENCE_CONTENT_TYPES: [&str; 4] =
    ["image/jpeg", "image/png", "image/heic", "application/pdf"];

/// Lower-case the bare media type, dropping any `; charset=…` parameters.
#[must_use]
pub fn normalize_media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
}

fn validate_upload_command(command: &EvidenceUploadCommand) -> Result<(), StorageError> {
    let content_type = command.content_type.trim();
    if content_type.is_empty() {
        return Err(KernelError::validation("evidence content_type is required").into());
    }
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if !ALLOWED_EVIDENCE_CONTENT_TYPES.contains(&media_type.as_str()) {
        return Err(KernelError::validation(format!(
            "evidence content_type {:?} is not allowed (permitted: {})",
            command.content_type,
            ALLOWED_EVIDENCE_CONTENT_TYPES.join(", ")
        ))
        .into());
    }
    if command.size_bytes < 0 {
        return Err(KernelError::validation("evidence size must be non-negative").into());
    }
    if command.size_bytes > MAX_EVIDENCE_SIZE_BYTES {
        return Err(KernelError::validation(format!(
            "evidence size {} exceeds the maximum of {} bytes",
            command.size_bytes, MAX_EVIDENCE_SIZE_BYTES
        ))
        .into());
    }
    Ok(())
}

/// Validate a media-processing staging command: the content type must be in the
/// image/video allowlist, and the size must be non-negative and within the
/// per-[`MediaKind`] cap. Returns the classified kind on success.
fn validate_staging_command(command: &StagingUploadCommand) -> Result<MediaKind, StorageError> {
    let content_type = command.content_type.trim();
    if content_type.is_empty() {
        return Err(KernelError::validation("evidence content_type is required").into());
    }
    let kind = MediaKind::from_content_type(content_type).ok_or_else(|| {
        KernelError::validation(format!(
            "evidence content_type {:?} is not allowed (permitted images: {}; videos: {})",
            command.content_type,
            ALLOWED_EVIDENCE_IMAGE_TYPES.join(", "),
            ALLOWED_EVIDENCE_VIDEO_TYPES.join(", ")
        ))
    })?;
    if command.size_bytes < 0 {
        return Err(KernelError::validation("evidence size must be non-negative").into());
    }
    let max = kind.max_upload_bytes();
    if command.size_bytes > max {
        return Err(KernelError::validation(format!(
            "evidence size {} exceeds the {:?} maximum of {} bytes",
            command.size_bytes, kind, max
        ))
        .into());
    }
    Ok(kind)
}

// ===========================================================================
// Media processing — the "process BEFORE storage" core.
//
// Mechanic uploads arrive in arbitrary, unoptimized formats. We transcode video
// to 1080p H.264/AAC MP4 (faststart, CRF ~23, never upscale) and recompress
// images to <= 1920px long edge JPEG (quality ~80), STRIP all metadata/EXIF/GPS
// (PII), and generate a thumbnail. The argv is built by pure, unit-testable
// functions so a test can assert the 1080p/CRF/strip-metadata flags without
// invoking ffmpeg. The actual transcode runs behind the `MediaProcessor` port so
// the worker logic + status transitions can be tested with a stub.
// ===========================================================================

/// Magic-number re-validation of the downloaded staging bytes against the
/// declared [`MediaKind`]. The declared `content_type` is advisory; these bytes
/// are authoritative. Detects the real container via [`infer`] and rejects
/// (a) bytes that are neither a recognized image nor a recognized video and
/// (b) a kind mismatch (image declared but video bytes, or vice versa) so a
/// disguised payload is never fed to the transcoder.
///
/// We gate on `infer`'s coarse `MatcherType` (Image vs Video) rather than the
/// exact MIME: `infer` reports e.g. HEIC as `image/heif` (not the `image/heic`
/// allowlist spelling), and ffmpeg ultimately re-derives the precise codec.
/// What matters here is that an IMAGE upload is image bytes and a VIDEO upload
/// is video bytes — anything else (PDF, audio, archive, executable, garbage) is
/// rejected before transcoding.
fn validate_sniffed_kind(bytes: &[u8], declared: MediaKind) -> Result<(), StorageError> {
    use infer::MatcherType;

    let detected = infer::get(bytes).ok_or_else(|| {
        StorageError::Processing(
            "uploaded content does not match declared type: unrecognized container".to_owned(),
        )
    })?;
    let detected_kind = match detected.matcher_type() {
        MatcherType::Image => MediaKind::Image,
        MatcherType::Video => MediaKind::Video,
        other => {
            return Err(StorageError::Processing(format!(
                "uploaded content does not match declared type: detected {:?} ({other:?}) is not an allowed evidence image/video",
                detected.mime_type()
            )));
        }
    };
    if detected_kind != declared {
        return Err(StorageError::Processing(format!(
            "uploaded content does not match declared type: detected {:?} ({detected_kind:?}) but row declares {declared:?}",
            detected.mime_type()
        )));
    }
    Ok(())
}

/// Long-edge cap for both video (height) and image processing.
pub const EVIDENCE_MAX_LONG_EDGE: u32 = 1920;
/// Video vertical cap (1080p); paired with [`EVIDENCE_MAX_LONG_EDGE`] width.
pub const EVIDENCE_MAX_VIDEO_HEIGHT: u32 = 1080;
/// libx264 constant-rate-factor: ~23 is visually transparent at sane bitrate.
pub const EVIDENCE_VIDEO_CRF: u32 = 23;
/// libjpeg quality for recompressed images (~80, ffmpeg `-q:v` ≈ 4).
pub const EVIDENCE_IMAGE_QUALITY: u32 = 80;
/// ffmpeg `-q:v` scale (2 best … 31 worst) corresponding to quality ~80.
pub const EVIDENCE_IMAGE_QSCALE: u32 = 4;

/// Env var overriding the per-job ffmpeg wall-clock timeout (whole seconds). A
/// transcode that exceeds it is KILLED and the row marked FAILED, bounding the
/// CPU/wall-clock a single adversarial upload can consume.
pub const FFMPEG_TIMEOUT_ENV: &str = "CONSOLE_EVIDENCE_FFMPEG_TIMEOUT_SECS";
/// Default per-job video transcode timeout (5 min). H.264 of a 200 MiB original
/// at `-preset medium` is comfortably faster than this on the worker node.
pub const FFMPEG_VIDEO_TIMEOUT_SECS: u64 = 300;
/// Default per-job image recompress timeout (30 s). A single recompress should
/// complete in well under a second; a longer run signals a bomb/hang.
pub const FFMPEG_IMAGE_TIMEOUT_SECS: u64 = 30;

/// Output filesize cap (ffmpeg `-fs <bytes>`) for the transcoded VIDEO. ffmpeg
/// stops writing once the muxed output reaches this size, bounding a
/// decompression-bomb output independently of the input size. Set to the video
/// per-kind upload cap: a faithful 1080p/CRF-23 transcode is always far smaller,
/// so this only ever trips on pathological inputs.
#[must_use]
pub const fn evidence_video_output_cap_bytes() -> i64 {
    MediaKind::Video.max_upload_bytes()
}

/// Output filesize cap (ffmpeg `-fs <bytes>`) for the recompressed IMAGE / any
/// generated thumbnail. A recompressed <=1920px JPEG at q~80 is well under the
/// image upload cap, so this only trips on a decompression bomb.
#[must_use]
pub const fn evidence_image_output_cap_bytes() -> i64 {
    MediaKind::Image.max_upload_bytes()
}

/// Resolve the per-job ffmpeg wall-clock timeout for `kind`, honoring
/// [`FFMPEG_TIMEOUT_ENV`] when set to a positive integer (applied to both
/// pipelines); otherwise the per-kind default.
#[must_use]
pub fn evidence_ffmpeg_timeout(kind: MediaKind) -> StdDuration {
    if let Some(secs) = std::env::var(FFMPEG_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
    {
        return StdDuration::from_secs(secs);
    }
    let default = match kind {
        MediaKind::Image => FFMPEG_IMAGE_TIMEOUT_SECS,
        MediaKind::Video => FFMPEG_VIDEO_TIMEOUT_SECS,
    };
    StdDuration::from_secs(default)
}

/// The optimized artifact a [`MediaProcessor`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedMedia {
    /// The optimized deliverable bytes (1080p MP4 / recompressed JPEG).
    pub artifact: Vec<u8>,
    /// The deliverable's content type (`video/mp4` or `image/jpeg`).
    pub content_type: String,
    /// The generated thumbnail / poster JPEG bytes.
    pub thumbnail: Vec<u8>,
}

/// Build the ffmpeg argv that transcodes a staged VIDEO original (read from
/// `input` path) to a 1080p H.264/AAC MP4 at `output`.
///
/// `scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease`
/// fits within 1920x1080 WITHOUT upscaling and preserves aspect ratio;
/// `-movflags +faststart` front-loads the moov atom for streaming;
/// `-map_metadata -1` STRIPS all container metadata (EXIF/GPS/PII).
#[must_use]
pub fn ffmpeg_video_args(input: &str, output: &str) -> Vec<String> {
    vec![
        "-y".to_owned(),
        "-i".to_owned(),
        input.to_owned(),
        "-vf".to_owned(),
        format!(
            "scale='min({w},iw)':'min({h},ih)':force_original_aspect_ratio=decrease",
            w = EVIDENCE_MAX_LONG_EDGE,
            h = EVIDENCE_MAX_VIDEO_HEIGHT
        ),
        "-c:v".to_owned(),
        "libx264".to_owned(),
        "-preset".to_owned(),
        "medium".to_owned(),
        "-crf".to_owned(),
        EVIDENCE_VIDEO_CRF.to_string(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        "-c:a".to_owned(),
        "aac".to_owned(),
        "-b:a".to_owned(),
        "128k".to_owned(),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
        "-map_metadata".to_owned(),
        "-1".to_owned(),
        // Bound the muxed output so a decompression-bomb input cannot produce an
        // unbounded artifact (ffmpeg stops writing at this size).
        "-fs".to_owned(),
        evidence_video_output_cap_bytes().to_string(),
        output.to_owned(),
    ]
}

/// Build the ffmpeg argv that captures a single poster frame from a staged
/// video and writes a thumbnail JPEG (scaled to fit the long edge, metadata
/// stripped).
#[must_use]
pub fn ffmpeg_video_thumbnail_args(input: &str, output: &str) -> Vec<String> {
    vec![
        "-y".to_owned(),
        "-i".to_owned(),
        input.to_owned(),
        "-frames:v".to_owned(),
        "1".to_owned(),
        "-vf".to_owned(),
        format!(
            "scale='min({w},iw)':-2:force_original_aspect_ratio=decrease",
            w = EVIDENCE_MAX_LONG_EDGE
        ),
        "-q:v".to_owned(),
        EVIDENCE_IMAGE_QSCALE.to_string(),
        "-map_metadata".to_owned(),
        "-1".to_owned(),
        // Bound the thumbnail JPEG output against a decompression-bomb frame.
        "-fs".to_owned(),
        evidence_image_output_cap_bytes().to_string(),
        output.to_owned(),
    ]
}

/// Build the ffmpeg argv that recompresses/resizes a staged IMAGE original to a
/// JPEG that fits within the long-edge cap (never upscale), quality ~80, with
/// ALL metadata/EXIF/GPS stripped.
#[must_use]
pub fn ffmpeg_image_args(input: &str, output: &str) -> Vec<String> {
    vec![
        "-y".to_owned(),
        "-i".to_owned(),
        input.to_owned(),
        "-vf".to_owned(),
        format!(
            "scale='min({w},iw)':-2:force_original_aspect_ratio=decrease",
            w = EVIDENCE_MAX_LONG_EDGE
        ),
        "-q:v".to_owned(),
        EVIDENCE_IMAGE_QSCALE.to_string(),
        "-map_metadata".to_owned(),
        "-1".to_owned(),
        // Bound the recompressed JPEG output against a decompression-bomb input.
        "-fs".to_owned(),
        evidence_image_output_cap_bytes().to_string(),
        output.to_owned(),
    ]
}

/// Port for the actual media transcode/optimize step. Implemented for real by
/// [`FfmpegMediaProcessor`]; stubbed in tests so the worker's status-transition
/// logic is exercised without invoking ffmpeg.
pub trait MediaProcessor: Send + Sync {
    fn process<'a>(
        &'a self,
        kind: MediaKind,
        original: Vec<u8>,
    ) -> StorageFuture<'a, ProcessedMedia>;
}

/// Real [`MediaProcessor`]: shells out to `ffmpeg` for both video and image
/// pipelines via temp files, using the argv built by [`ffmpeg_video_args`] /
/// [`ffmpeg_image_args`] / [`ffmpeg_video_thumbnail_args`].
#[derive(Debug, Clone)]
pub struct FfmpegMediaProcessor {
    ffmpeg_path: String,
}

impl Default for FfmpegMediaProcessor {
    fn default() -> Self {
        Self {
            ffmpeg_path: "ffmpeg".to_owned(),
        }
    }
}

impl FfmpegMediaProcessor {
    #[must_use]
    pub fn new(ffmpeg_path: impl Into<String>) -> Self {
        Self {
            ffmpeg_path: ffmpeg_path.into(),
        }
    }

    /// Run ffmpeg with a per-job wall-clock `timeout`. On elapse the child is
    /// KILLED (so it cannot keep burning CPU after we give up) and a "transcode
    /// timed out" error is returned, which `process_job` surfaces via
    /// `mark_failed`. Bounds the wall-clock/CPU a single adversarial upload can
    /// consume regardless of the `-fs` output cap.
    async fn run_ffmpeg(&self, args: &[String], timeout: StdDuration) -> Result<(), StorageError> {
        use tokio::io::AsyncReadExt;

        let mut child = tokio::process::Command::new(&self.ffmpeg_path)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| StorageError::Processing(format!("failed to spawn ffmpeg: {err}")))?;

        // Drain stderr CONCURRENTLY with the wait. ffmpeg can emit more than a
        // pipe buffer's worth of progress/log output on a long transcode; if we
        // waited on the child WITHOUT draining stderr, a full pipe would block
        // ffmpeg's writes while we block on `wait()` — a deadlock that only the
        // timeout would break. Reading stderr to end here also avoids `kill_on_drop`
        // racing the reaper. `child.wait()` keeps `child` owned so we can KILL it.
        let mut stderr_pipe = child.stderr.take();
        let drain = async {
            let mut buf = Vec::new();
            if let Some(pipe) = stderr_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf).await;
            }
            buf
        };

        let waited = tokio::time::timeout(timeout, async {
            let (status, stderr) = tokio::join!(child.wait(), drain);
            (status, stderr)
        })
        .await;

        let (status, stderr) = match waited {
            Ok((status, stderr)) => (
                status.map_err(|err| {
                    StorageError::Processing(format!("failed to wait for ffmpeg: {err}"))
                })?,
                stderr,
            ),
            Err(_elapsed) => {
                // Wall-clock budget exhausted: KILL the child so it stops burning
                // CPU, reap it, and surface a clear timeout error.
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(StorageError::Processing(format!(
                    "transcode timed out after {}s",
                    timeout.as_secs()
                )));
            }
        };

        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(StorageError::Processing(format!(
                "ffmpeg exited with {}: {}",
                status,
                stderr.lines().last().unwrap_or("").trim()
            )));
        }
        Ok(())
    }
}

impl MediaProcessor for FfmpegMediaProcessor {
    fn process<'a>(
        &'a self,
        kind: MediaKind,
        original: Vec<u8>,
    ) -> StorageFuture<'a, ProcessedMedia> {
        Box::pin(async move {
            let dir = tempdir_in_runtime()?;
            let input = dir.join("input");
            let artifact = dir.join(match kind {
                MediaKind::Image => "out.jpg",
                MediaKind::Video => "out.mp4",
            });
            let thumb = dir.join("thumb.jpg");
            tokio::fs::write(&input, &original)
                .await
                .map_err(|err| StorageError::Processing(format!("write staging input: {err}")))?;
            let input_s = path_str(&input)?;
            let artifact_s = path_str(&artifact)?;
            let thumb_s = path_str(&thumb)?;
            // Per-job wall-clock budget (env-overridable) applied to EACH ffmpeg
            // invocation so neither the transcode nor the thumbnail step can hang.
            let timeout = evidence_ffmpeg_timeout(kind);

            match kind {
                MediaKind::Image => {
                    self.run_ffmpeg(&ffmpeg_image_args(&input_s, &artifact_s), timeout)
                        .await?;
                    // The recompressed JPEG doubles as the thumbnail source.
                    self.run_ffmpeg(&ffmpeg_image_args(&artifact_s, &thumb_s), timeout)
                        .await?;
                }
                MediaKind::Video => {
                    self.run_ffmpeg(&ffmpeg_video_args(&input_s, &artifact_s), timeout)
                        .await?;
                    self.run_ffmpeg(&ffmpeg_video_thumbnail_args(&input_s, &thumb_s), timeout)
                        .await?;
                }
            }

            let artifact_bytes = tokio::fs::read(&artifact)
                .await
                .map_err(|err| StorageError::Processing(format!("read artifact: {err}")))?;
            let thumbnail_bytes = tokio::fs::read(&thumb)
                .await
                .map_err(|err| StorageError::Processing(format!("read thumbnail: {err}")))?;

            let content_type = match kind {
                MediaKind::Image => "image/jpeg",
                MediaKind::Video => "video/mp4",
            };
            Ok(ProcessedMedia {
                artifact: artifact_bytes,
                content_type: content_type.to_owned(),
                thumbnail: thumbnail_bytes,
            })
        })
    }
}

#[derive(Debug)]
struct ProcessingTempDir {
    path: PathBuf,
}

impl ProcessingTempDir {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, child: impl AsRef<Path>) -> PathBuf {
        self.path.join(child)
    }
}

impl Drop for ProcessingTempDir {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                // Do not include the temp path in logs: evidence media may be
                // tenant/user sensitive, and cleanup observability must not add
                // a new PII leak. The original processing result remains primary.
                tracing::warn!(
                    error = %err,
                    "failed to remove ffmpeg media-processing temp directory"
                );
            }
        }
    }
}

fn tempdir_in_runtime() -> Result<ProcessingTempDir, StorageError> {
    let base = std::env::temp_dir();
    for _ in 0..16 {
        let dir = base.join(format!("mnt-evidence-{}", uuid::Uuid::new_v4()));
        match create_private_temp_dir(&dir) {
            Ok(()) => return Ok(ProcessingTempDir::new(dir)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(StorageError::Processing(format!(
                    "create private temp dir: {err}"
                )));
            }
        }
    }
    Err(StorageError::Processing(
        "create private temp dir: exhausted unique names".to_owned(),
    ))
}

#[cfg(unix)]
fn create_private_temp_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)?;
    if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)) {
        let _ = std::fs::remove_dir(path);
        return Err(err);
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_temp_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().create(path)
}

fn path_str(path: &Path) -> Result<String, StorageError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| StorageError::Processing("non-utf8 temp path".to_owned()))
}

fn retry_delay(retry_count: i32, config: &ReplicationConfig) -> Duration {
    let exponent = u32::try_from(retry_count.saturating_sub(1)).unwrap_or(0);
    let multiplier = 2_i32.saturating_pow(exponent);
    let delay = config.base_retry_delay * multiplier;
    if delay > config.max_retry_delay {
        config.max_retry_delay
    } else {
        delay
    }
}

async fn branch_for_work_order(
    pool: &PgPool,
    work_order_id: WorkOrderId,
) -> Result<BranchId, StorageError> {
    let org = current_org().map_err(KernelError::from)?;
    let branch_uuid: uuid::Uuid = with_org_conn::<_, _, StorageError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(
                sqlx::query_scalar("SELECT branch_id FROM work_orders WHERE id = $1")
                    .bind(*work_order_id.as_uuid())
                    .fetch_optional(tx.as_mut())
                    .await?,
            )
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    Ok(BranchId::from_uuid(branch_uuid))
}

struct NewEvidenceMedia<'a> {
    media_id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    s3_key: &'a str,
    content_type: &'a str,
    size_bytes: i64,
    checksum_sha256: Option<&'a str>,
    uploaded_by: UserId,
    occurred_at: Timestamp,
}

/// FIX 3: lock the parent work-order row and reject AFTER/REPORT completion
/// evidence when the work order has reached a terminal status. Only the two
/// completion stages feed the `evidence_verified` interlock, so other stages
/// (REQUEST/BEFORE/DURING/OUTSOURCE_RESULT) are left insertable.
async fn ensure_work_order_accepts_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
) -> Result<(), StorageError> {
    if !matches!(stage, AttachmentStage::After | AttachmentStage::Report) {
        return Ok(());
    }
    let status: String =
        sqlx::query_scalar("SELECT status FROM work_orders WHERE id = $1 FOR UPDATE")
            .bind(*work_order_id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?
            .ok_or_else(|| KernelError::not_found("work order was not found"))?;
    if matches!(
        status.as_str(),
        "FINAL_COMPLETED" | "ARCHIVED" | "CANCELLED"
    ) {
        return Err(KernelError::conflict(format!(
            "cannot attach {} evidence to a work order in terminal status {status}",
            stage.as_db_str()
        ))
        .into());
    }
    Ok(())
}

async fn insert_evidence_media_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    media: NewEvidenceMedia<'_>,
    org_uuid: uuid::Uuid,
) -> Result<EvidenceMedia, StorageError> {
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            checksum_sha256, uploaded_by, worm_replica_status,
            retry_count, next_retry_at, created_at, updated_at, org_id
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, 'PENDING',
            0, $9, $9, $9, $10
        )
        "#,
    )
    .bind(*media.media_id.as_uuid())
    .bind(*media.work_order_id.as_uuid())
    .bind(media.stage.as_db_str())
    .bind(media.s3_key)
    .bind(media.content_type.trim())
    .bind(media.size_bytes)
    .bind(media.checksum_sha256)
    .bind(*media.uploaded_by.as_uuid())
    .bind(media.occurred_at)
    .bind(org_uuid)
    .execute(tx.as_mut())
    .await?;
    evidence_media_by_id_tx(tx, media.media_id).await
}

struct NewProcessingEvidence<'a> {
    media_id: EvidenceId,
    work_order_id: WorkOrderId,
    stage: AttachmentStage,
    final_key: &'a str,
    staging_key: &'a str,
    original_content_type: &'a str,
    size_bytes: i64,
    checksum_sha256: Option<&'a str>,
    uploaded_by: UserId,
    occurred_at: Timestamp,
}

/// Insert a `PROCESSING` evidence row for a media-processing staging upload.
///
/// `s3_key` is set to the FINAL deliverable key up front (the row keeps a stable
/// deliverable path); `staging_s3_key` holds the original until the worker
/// transcodes and deletes it. `content_type` initially mirrors the original so
/// the row is non-empty; the worker overwrites it with the optimized type on
/// READY. Caller is inside `with_audit`, so `app.current_org` is armed and the
/// RLS `org_isolation` WITH CHECK validates `org_id` against the GUC.
async fn insert_processing_evidence_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    media: NewProcessingEvidence<'_>,
    org_uuid: uuid::Uuid,
) -> Result<EvidenceMedia, StorageError> {
    sqlx::query(
        r#"
        INSERT INTO evidence_media (
            id, work_order_id, stage, s3_key, content_type, size_bytes,
            checksum_sha256, uploaded_by, worm_replica_status,
            retry_count, next_retry_at, created_at, updated_at, org_id,
            processing_status, staging_s3_key, original_content_type
        )
        VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, $8, 'PENDING',
            0, $9, $9, $9, $10,
            'PROCESSING', $11, $12
        )
        "#,
    )
    .bind(*media.media_id.as_uuid())
    .bind(*media.work_order_id.as_uuid())
    .bind(media.stage.as_db_str())
    .bind(media.final_key)
    .bind(media.original_content_type)
    .bind(media.size_bytes)
    .bind(media.checksum_sha256)
    .bind(*media.uploaded_by.as_uuid())
    .bind(media.occurred_at)
    .bind(org_uuid)
    .bind(media.staging_key)
    .bind(media.original_content_type)
    .execute(tx.as_mut())
    .await?;
    evidence_media_by_id_tx(tx, media.media_id).await
}

/// Fetch the oldest still-`PROCESSING` evidence row for the armed tenant.
/// RLS-armed via `with_org_conn`; the partial index
/// `idx_evidence_media_processing_queue` keeps the scan cheap.
async fn next_processing_media(
    pool: &PgPool,
    org: OrgId,
) -> Result<Option<EvidenceMedia>, StorageError> {
    let row = with_org_conn::<_, _, StorageError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT id, work_order_id, stage, s3_key, content_type, size_bytes,
               checksum_sha256, uploaded_by, worm_replica_status, retry_count,
               next_retry_at, last_error, verified_at, upload_confirmed_at,
               confirmed_by, created_at, updated_at,
               processing_status, staging_s3_key, thumbnail_s3_key,
               original_content_type, processing_error, processed_at
        FROM evidence_media
        WHERE processing_status = 'PROCESSING'
        ORDER BY created_at
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
            )
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?;
    row.as_ref().map(evidence_media_from_row).transpose()
}

async fn evidence_media_by_id(
    pool: &PgPool,
    id: EvidenceId,
) -> Result<EvidenceMedia, StorageError> {
    let org = current_org().map_err(KernelError::from)?;
    let row = with_org_conn::<_, _, StorageError>(pool, org, move |tx| {
        Box::pin(async move {
            Ok(sqlx::query(
                r#"
        SELECT id, work_order_id, stage, s3_key, content_type, size_bytes,
               checksum_sha256, uploaded_by, worm_replica_status, retry_count,
               next_retry_at, last_error, verified_at, upload_confirmed_at,
               confirmed_by, created_at, updated_at,
               processing_status, staging_s3_key, thumbnail_s3_key,
               original_content_type, processing_error, processed_at
        FROM evidence_media
        WHERE id = $1
        "#,
            )
            .bind(*id.as_uuid())
            .fetch_optional(tx.as_mut())
            .await?)
        })
    })
    .await?
    .ok_or_else(|| KernelError::not_found("evidence media was not found"))?;
    evidence_media_from_row(&row)
}

async fn evidence_media_by_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: EvidenceId,
) -> Result<EvidenceMedia, StorageError> {
    let row = sqlx::query(
        r#"
        SELECT id, work_order_id, stage, s3_key, content_type, size_bytes,
               checksum_sha256, uploaded_by, worm_replica_status, retry_count,
               next_retry_at, last_error, verified_at, upload_confirmed_at,
               confirmed_by, created_at, updated_at,
               processing_status, staging_s3_key, thumbnail_s3_key,
               original_content_type, processing_error, processed_at
        FROM evidence_media
        WHERE id = $1
        "#,
    )
    .bind(*id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| KernelError::not_found("evidence media was not found"))?;
    evidence_media_from_row(&row)
}

fn evidence_media_from_row(row: &sqlx::postgres::PgRow) -> Result<EvidenceMedia, StorageError> {
    let stage: String = row.try_get("stage")?;
    let status: String = row.try_get("worm_replica_status")?;
    let processing_status: String = row.try_get("processing_status")?;
    Ok(EvidenceMedia {
        id: EvidenceId::from_uuid(row.try_get("id")?),
        work_order_id: WorkOrderId::from_uuid(row.try_get("work_order_id")?),
        stage: AttachmentStage::from_db_str(&stage)?,
        s3_key: row.try_get("s3_key")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        checksum_sha256: row.try_get("checksum_sha256")?,
        uploaded_by: UserId::from_uuid(row.try_get("uploaded_by")?),
        worm_replica_status: WormReplicaStatus::from_db_str(&status)?,
        retry_count: row.try_get("retry_count")?,
        next_retry_at: row.try_get("next_retry_at")?,
        last_error: row.try_get("last_error")?,
        verified_at: row.try_get("verified_at")?,
        upload_confirmed_at: row.try_get("upload_confirmed_at")?,
        confirmed_by: row
            .try_get::<Option<uuid::Uuid>, _>("confirmed_by")?
            .map(UserId::from_uuid),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        processing_status: ProcessingStatus::from_db_str(&processing_status)?,
        staging_s3_key: row.try_get("staging_s3_key")?,
        thumbnail_s3_key: row.try_get("thumbnail_s3_key")?,
        original_content_type: row.try_get("original_content_type")?,
        processing_error: row.try_get("processing_error")?,
        processed_at: row.try_get("processed_at")?,
    })
}

async fn ensure_success(operation: &str, response: reqwest::Response) -> Result<(), StorageError> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(s3_response_error(operation, response).await)
    }
}

async fn s3_response_error(operation: &str, response: reqwest::Response) -> StorageError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|err| format!("<failed to read body: {err}>"));
    StorageError::S3(format!("{operation} failed with {status}: {body}"))
}

fn reqwest_error(value: reqwest::Error) -> StorageError {
    StorageError::S3(value.to_string())
}

fn header_value(value: &str) -> Result<HeaderValue, StorageError> {
    HeaderValue::from_str(value)
        .map_err(|err| StorageError::S3(format!("invalid S3 header value: {err}")))
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn header_i64(headers: &HeaderMap, name: &HeaderName) -> Option<i64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
}

fn host_header(url: &Url) -> Result<String, StorageError> {
    let host = url
        .host_str()
        .ok_or_else(|| StorageError::S3("S3 URL has no host".to_owned()))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

fn retention_xml(retain_until: &str) -> String {
    format!(
        "<Retention><Mode>COMPLIANCE</Mode><RetainUntilDate>{retain_until}</RetainUntilDate></Retention>"
    )
}

fn xml_tag_text(body: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{tag}>");
    let end_tag = format!("</{tag}>");
    let start = body.find(&start_tag)? + start_tag.len();
    let end = body[start..].find(&end_tag)? + start;
    Some(body[start..end].to_owned())
}

fn sigv4_date(now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

fn sigv4_timestamp(now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hmac_sha256(key: &[u8], value: &str) -> Result<Vec<u8>, StorageError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|err| StorageError::Presign(format!("invalid HMAC key: {err}")))?;
    mac.update(value.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn sigv4_signature(
    secret_access_key: &str,
    date: &str,
    region: &str,
    service: &str,
    string_to_sign: &str,
) -> Result<String, StorageError> {
    let k_date = hmac_sha256(format!("AWS4{secret_access_key}").as_bytes(), date)?;
    let k_region = hmac_sha256(&k_date, region)?;
    let k_service = hmac_sha256(&k_region, service)?;
    let k_signing = hmac_sha256(&k_service, "aws4_request")?;
    Ok(hex::encode(hmac_sha256(&k_signing, string_to_sign)?))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use console_kernel_core::OrgId;
    use console_workorder_domain::PriorityLevel;
    use sqlx::PgPool;
    use time::OffsetDateTime;

    use super::*;

    #[derive(Debug, Clone)]
    struct StaticObjectStore {
        copy_errors: Arc<Mutex<Vec<String>>>,
    }

    impl StaticObjectStore {
        fn ok() -> Self {
            Self {
                copy_errors: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn fail_copy(errors: Vec<&str>) -> Self {
            Self {
                copy_errors: Arc::new(Mutex::new(
                    errors.into_iter().map(ToOwned::to_owned).collect(),
                )),
            }
        }
    }

    impl S3ObjectStore for StaticObjectStore {
        fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
            Box::pin(async move {
                Ok(PresignedUpload {
                    method: "PUT".to_owned(),
                    url: format!("http://storage.local/{}/{}", request.bucket, request.key),
                    headers: vec![("content-type".to_owned(), request.content_type)],
                    expires_in_secs: request.expires_in.as_secs(),
                })
            })
        }

        fn presign_get(&self, request: PresignGetRequest) -> StorageFuture<'_, String> {
            Box::pin(async move {
                Ok(format!(
                    "http://storage.local/{}/{}?X-Amz-Signature=test",
                    request.bucket, request.key
                ))
            })
        }

        fn copy_object(&self, _request: CopyObjectRequest) -> StorageFuture<'_, ()> {
            Box::pin(async move {
                let mut errors = self.copy_errors.lock().unwrap();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(StorageError::S3(errors.remove(0)))
                }
            })
        }

        fn head_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, ObjectHead> {
            Box::pin(async {
                Ok(ObjectHead {
                    size_bytes: 1024,
                    e_tag: Some("\"etag\"".to_owned()),
                    checksum_sha256: None,
                    object_lock_mode: Some("COMPLIANCE".to_owned()),
                    retain_until: Some("2026-06-13T00:00:00Z".to_owned()),
                })
            })
        }

        fn get_object_retention(
            &self,
            _bucket: String,
            _key: String,
        ) -> StorageFuture<'_, RetentionInfo> {
            Box::pin(async {
                Ok(RetentionInfo {
                    mode: Some("COMPLIANCE".to_owned()),
                    retain_until: Some("2026-06-13T00:00:00Z".to_owned()),
                })
            })
        }

        fn get_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, Vec<u8>> {
            Box::pin(async { Ok(b"original-bytes".to_vec()) })
        }

        fn put_object(
            &self,
            _bucket: String,
            _key: String,
            _content_type: String,
            _body: Vec<u8>,
        ) -> StorageFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }

        fn delete_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn presign_flow_records_pending_evidence_and_upload_audit(pool: PgPool) {
        console_platform_request_context::scope_org(
            console_kernel_core::OrgId::knl(),
            async move {
                let seeded = seed_work_order(&pool).await;
                let service = EvidenceService::new(
                    pool.clone(),
                    StaticObjectStore::ok(),
                    "primary".to_owned(),
                    "replica".to_owned(),
                );

                let ticket = service
                    .issue_presigned_upload(EvidenceUploadCommand {
                        actor: seeded.uploaded_by,
                        work_order_id: seeded.work_order_id,
                        stage: AttachmentStage::After,
                        content_type: "image/jpeg".to_owned(),
                        size_bytes: 1024,
                        checksum_sha256: None,
                        trace: TraceContext::generate(),
                        occurred_at: OffsetDateTime::now_utc(),
                    })
                    .await
                    .unwrap();

                assert_eq!(ticket.upload.method, "PUT");
                assert_eq!(ticket.media.stage, AttachmentStage::After);
                assert_eq!(ticket.media.worm_replica_status, WormReplicaStatus::Pending);
                assert!(
                    ticket
                        .media
                        .s3_key
                        .contains(&seeded.work_order_id.to_string())
                );

                let audit_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM audit_events WHERE action = 'evidence.upload'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(audit_count, 1);
            },
        )
        .await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn failed_after_max_retries_is_visible_in_admin_queue(pool: PgPool) {
        console_platform_request_context::scope_org(
            console_kernel_core::OrgId::knl(),
            async move {
                let seeded = seed_work_order(&pool).await;
                let service = EvidenceService::new(
                    pool.clone(),
                    StaticObjectStore::fail_copy(vec!["source missing", "still missing"]),
                    "primary".to_owned(),
                    "replica".to_owned(),
                )
                .with_replication_config(ReplicationConfig {
                    primary_bucket: "primary".to_owned(),
                    replica_bucket: "replica".to_owned(),
                    max_retries: 2,
                    base_retry_delay: Duration::seconds(1),
                    max_retry_delay: Duration::seconds(5),
                    retention_period: Duration::days(1),
                });
                let ticket = service
                    .issue_presigned_upload(EvidenceUploadCommand {
                        actor: seeded.uploaded_by,
                        work_order_id: seeded.work_order_id,
                        stage: AttachmentStage::Report,
                        content_type: "image/jpeg".to_owned(),
                        size_bytes: 1024,
                        checksum_sha256: None,
                        trace: TraceContext::generate(),
                        occurred_at: OffsetDateTime::now_utc(),
                    })
                    .await
                    .unwrap();

                let first = service
                    .replicate_once(
                        ticket.media.id,
                        TraceContext::generate(),
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    .unwrap();
                assert_eq!(first.status, WormReplicaStatus::Pending);
                assert_eq!(first.retry_count, 1);

                let second = service
                    .replicate_once(
                        ticket.media.id,
                        TraceContext::generate(),
                        OffsetDateTime::now_utc(),
                    )
                    .await
                    .unwrap();
                assert_eq!(second.status, WormReplicaStatus::Failed);
                assert_eq!(second.retry_count, 2);

                let queued: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM unverified_evidence_admin_queue WHERE id = $1",
                )
                .bind(*ticket.media.id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(queued, 1);
            },
        )
        .await;
    }

    // FIX 3 (storage layer): AFTER/REPORT evidence must be rejected for a work
    // order in a terminal status — the WORM completion invariant cannot be
    // invalidated after FINAL_COMPLETED.
    #[sqlx::test(migrations = "../db/migrations")]
    async fn presign_rejected_for_after_evidence_on_terminal_work_order(pool: PgPool) {
        console_platform_request_context::scope_org(
            console_kernel_core::OrgId::knl(),
            async move {
                let seeded = seed_work_order_with_status(&pool, "FINAL_COMPLETED").await;
                let service = EvidenceService::new(
                    pool.clone(),
                    StaticObjectStore::ok(),
                    "primary".to_owned(),
                    "replica".to_owned(),
                );

                let err = service
                    .issue_presigned_upload(EvidenceUploadCommand {
                        actor: seeded.uploaded_by,
                        work_order_id: seeded.work_order_id,
                        stage: AttachmentStage::After,
                        content_type: "image/jpeg".to_owned(),
                        size_bytes: 1024,
                        checksum_sha256: None,
                        trace: TraceContext::generate(),
                        occurred_at: OffsetDateTime::now_utc(),
                    })
                    .await
                    .unwrap_err();
                assert!(
                    err.to_string().contains("terminal"),
                    "expected terminal-status rejection, got: {err}"
                );

                // No evidence row and no audit row should have been written.
                let media_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM evidence_media WHERE work_order_id = $1",
                )
                .bind(*seeded.work_order_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(media_count, 0);
                let audit_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM audit_events WHERE action = 'evidence.upload'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(audit_count, 0);
            },
        )
        .await;
    }

    // FIX 3 (DB trigger layer): a direct INSERT of AFTER/REPORT evidence on a
    // terminal work order must be rejected by the migration-0019 trigger even
    // when the REST/storage guard is bypassed.
    #[sqlx::test(migrations = "../db/migrations")]
    async fn db_trigger_rejects_after_evidence_insert_on_terminal_work_order(pool: PgPool) {
        let seeded = seed_work_order_with_status(&pool, "ARCHIVED").await;
        let result = sqlx::query(
            r#"
            INSERT INTO evidence_media (
                work_order_id, stage, s3_key, content_type, size_bytes,
                uploaded_by, worm_replica_status, retry_count, org_id
            )
            VALUES ($1, 'REPORT', $2, 'image/jpeg', 1024, $3, 'PENDING', 0, $4)
            "#,
        )
        .bind(*seeded.work_order_id.as_uuid())
        .bind(format!(
            "work-orders/{}/REPORT/direct",
            seeded.work_order_id
        ))
        .bind(*seeded.uploaded_by.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("terminal"),
            "expected DB trigger terminal rejection, got: {err}"
        );

        // A non-completion stage (BEFORE) remains insertable on a terminal WO.
        sqlx::query(
            r#"
            INSERT INTO evidence_media (
                work_order_id, stage, s3_key, content_type, size_bytes,
                uploaded_by, worm_replica_status, retry_count, org_id
            )
            VALUES ($1, 'BEFORE', $2, 'image/jpeg', 1024, $3, 'PENDING', 0, $4)
            "#,
        )
        .bind(*seeded.work_order_id.as_uuid())
        .bind(format!(
            "work-orders/{}/BEFORE/direct",
            seeded.work_order_id
        ))
        .bind(*seeded.uploaded_by.as_uuid())
        .bind(*OrgId::knl().as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    #[test]
    fn retry_delay_is_bounded_exponential() {
        let config = ReplicationConfig {
            primary_bucket: "primary".to_owned(),
            replica_bucket: "replica".to_owned(),
            max_retries: 5,
            base_retry_delay: Duration::seconds(2),
            max_retry_delay: Duration::seconds(5),
            retention_period: Duration::days(1),
        };
        assert_eq!(retry_delay(1, &config), Duration::seconds(2));
        assert_eq!(retry_delay(2, &config), Duration::seconds(4));
        assert_eq!(retry_delay(3, &config), Duration::seconds(5));
    }

    #[derive(Debug, Clone, Copy)]
    struct SeededEvidenceContext {
        work_order_id: WorkOrderId,
        uploaded_by: UserId,
    }

    async fn seed_work_order(pool: &PgPool) -> SeededEvidenceContext {
        seed_work_order_with_status(pool, "REPORT_SUBMITTED").await
    }

    async fn seed_work_order_with_status(pool: &PgPool, status: &str) -> SeededEvidenceContext {
        let branch_id = seed_branch(pool).await;
        let uploaded_by = seed_user(pool, "Evidence Uploader", "MECHANIC", branch_id).await;
        let requested_by = seed_user(pool, "Reception", "RECEPTIONIST", branch_id).await;
        let equipment_id = seed_equipment(pool, branch_id).await;
        let work_order_id = WorkOrderId::new();
        sqlx::query(
            r#"
            INSERT INTO work_orders (
                id, request_no, branch_id, equipment_id, customer_id, site_id,
                requested_by, status, priority, symptom, result_type, org_id
            )
            SELECT $1, $6, $2, e.id, e.customer_id, e.site_id,
                   $3, $7, $4, 'Evidence fixture', 'COMPLETED', $8
            FROM registry_equipment e
            WHERE e.id = $5
            "#,
        )
        .bind(*work_order_id.as_uuid())
        .bind(*branch_id.as_uuid())
        .bind(*requested_by.as_uuid())
        .bind(PriorityLevel::Unset.as_db_str())
        .bind(equipment_id)
        .bind(format!(
            "20260612-{:03}",
            (work_order_id.as_uuid().as_u128() % 1000) as u16
        ))
        .bind(status)
        .bind(*OrgId::knl().as_uuid())
        .execute(pool)
        .await
        .unwrap();
        SeededEvidenceContext {
            work_order_id,
            uploaded_by,
        }
    }

    async fn seed_branch(pool: &PgPool) -> BranchId {
        let region_id: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO regions (name, org_id) VALUES ($1, $2) RETURNING id")
                .bind(format!("Region {}", uuid::Uuid::new_v4()))
                .bind(*OrgId::knl().as_uuid())
                .fetch_one(pool)
                .await
                .unwrap();
        let branch_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO branches (region_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(region_id)
        .bind("HQ Storage Test")
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
        BranchId::from_uuid(branch_id)
    }

    async fn seed_user(pool: &PgPool, name: &str, role: &str, branch_id: BranchId) -> UserId {
        let user_id = UserId::new();
        sqlx::query("INSERT INTO users (id, display_name, roles, org_id) VALUES ($1, $2, $3, $4)")
            .bind(*user_id.as_uuid())
            .bind(name)
            .bind(Vec::from([role]))
            .bind(*OrgId::knl().as_uuid())
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_branches (user_id, branch_id, org_id) VALUES ($1, $2, $3)")
            .bind(*user_id.as_uuid())
            .bind(*branch_id.as_uuid())
            .bind(*OrgId::knl().as_uuid())
            .execute(pool)
            .await
            .unwrap();
        user_id
    }

    async fn seed_equipment(pool: &PgPool, branch_id: BranchId) -> uuid::Uuid {
        let customer_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO registry_customers (branch_id, name, org_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(*branch_id.as_uuid())
        .bind("Customer Storage")
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
        let site_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO registry_sites (branch_id, customer_id, name, org_id) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(*branch_id.as_uuid())
        .bind(customer_id)
        .bind("Site Storage")
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar(
            r#"
            INSERT INTO registry_equipment (
                branch_id, customer_id, site_id, equipment_no, management_no,
                manufacturer_code, kind_code, power_code, status,
                specification, ton_text, model, source_sheet, source_row, org_id
            )
            VALUES ($1, $2, $3, 'STR12-0001', 'S1',
                    'S', 'T', 'R', '임대', '좌식', '2.5', 'STORAGE', 'test', 1, $4)
            RETURNING id
            "#,
        )
        .bind(*branch_id.as_uuid())
        .bind(customer_id)
        .bind(site_id)
        .bind(*OrgId::knl().as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn upload_command(content_type: &str, size_bytes: i64) -> EvidenceUploadCommand {
        EvidenceUploadCommand {
            actor: UserId::new(),
            work_order_id: WorkOrderId::new(),
            stage: AttachmentStage::After,
            content_type: content_type.to_owned(),
            size_bytes,
            checksum_sha256: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn validate_upload_accepts_allowed_types_and_parameters() {
        for content_type in ["image/jpeg", "image/png", "image/heic", "application/pdf"] {
            validate_upload_command(&upload_command(content_type, 1024)).unwrap();
        }
        // Parameters and casing are tolerated on the media type.
        validate_upload_command(&upload_command("image/JPEG; charset=binary", 1024)).unwrap();
        // Exactly at the limit is allowed.
        validate_upload_command(&upload_command("image/png", MAX_EVIDENCE_SIZE_BYTES)).unwrap();
    }

    #[test]
    fn validate_upload_rejects_disallowed_content_type() {
        for content_type in ["text/html", "image/svg+xml", "application/octet-stream"] {
            let err = validate_upload_command(&upload_command(content_type, 1024)).unwrap_err();
            match err {
                StorageError::Domain(kernel) => {
                    assert_eq!(kernel.kind, console_kernel_core::ErrorKind::Validation);
                }
                other => panic!("expected validation error, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_upload_rejects_oversize() {
        let err =
            validate_upload_command(&upload_command("image/jpeg", MAX_EVIDENCE_SIZE_BYTES + 1))
                .unwrap_err();
        match err {
            StorageError::Domain(kernel) => {
                assert_eq!(kernel.kind, console_kernel_core::ErrorKind::Validation);
            }
            other => panic!("expected validation error, got {other:?}"),
        }
    }

    // ===================================================================
    // Media-processing pure-unit tests (no DB / no ffmpeg invocation).
    // ===================================================================

    #[test]
    fn media_kind_classifies_allowlist_and_rejects_others() {
        for ct in ["image/jpeg", "image/png", "image/webp", "image/heic"] {
            assert_eq!(MediaKind::from_content_type(ct), Some(MediaKind::Image));
        }
        for ct in ["video/mp4", "video/quicktime", "video/webm"] {
            assert_eq!(MediaKind::from_content_type(ct), Some(MediaKind::Video));
        }
        // Casing + parameters tolerated.
        assert_eq!(
            MediaKind::from_content_type("VIDEO/MP4; codecs=avc1"),
            Some(MediaKind::Video)
        );
        for ct in ["application/pdf", "text/html", "image/svg+xml", "image/gif"] {
            assert_eq!(MediaKind::from_content_type(ct), None);
        }
    }

    #[test]
    fn media_kind_size_caps_are_video_200mib_image_25mib() {
        assert_eq!(MediaKind::Image.max_upload_bytes(), 25 * 1024 * 1024);
        assert_eq!(MediaKind::Video.max_upload_bytes(), 200 * 1024 * 1024);
    }

    #[test]
    fn validate_staging_rejects_disallowed_mime_and_oversize() {
        let cmd = |ct: &str, size: i64| StagingUploadCommand {
            actor: UserId::new(),
            work_order_id: WorkOrderId::new(),
            stage: AttachmentStage::During,
            content_type: ct.to_owned(),
            size_bytes: size,
            checksum_sha256: None,
            trace: TraceContext::generate(),
            occurred_at: OffsetDateTime::now_utc(),
        };
        // application/pdf is NOT allowed on the media-processing path.
        assert!(matches!(
            validate_staging_command(&cmd("application/pdf", 1024)),
            Err(StorageError::Domain(_))
        ));
        // Image over the 25 MiB image cap is rejected even though it is < the
        // 200 MiB video cap.
        assert!(matches!(
            validate_staging_command(&cmd("image/jpeg", 26 * 1024 * 1024)),
            Err(StorageError::Domain(_))
        ));
        // A video of the same size is accepted (within the 200 MiB video cap).
        assert_eq!(
            validate_staging_command(&cmd("video/mp4", 26 * 1024 * 1024)).unwrap(),
            MediaKind::Video
        );
    }

    #[test]
    fn staging_and_final_keys_are_org_prefixed_and_distinct() {
        let org = OrgId::knl();
        let wo = WorkOrderId::new();
        let media = EvidenceId::new();
        let prefix = format!("orgs/{}/", org.as_uuid());

        let staging =
            evidence_staging_key(org, wo, AttachmentStage::Before, media, MediaKind::Video);
        let final_key =
            evidence_final_key(org, wo, AttachmentStage::Before, media, MediaKind::Video);
        let thumb = evidence_thumbnail_key(org, wo, AttachmentStage::Before, media);

        for key in [&staging, &final_key, &thumb] {
            assert!(key.starts_with(&prefix), "key not org-prefixed: {key}");
            assert!(key.contains(&wo.to_string()));
        }
        assert!(staging.contains("/staging/"));
        assert!(final_key.ends_with(".mp4"));
        assert!(thumb.ends_with(".thumb.jpg"));
        assert_ne!(staging, final_key);
    }

    #[test]
    fn ffmpeg_video_args_build_1080p_h264_faststart_strip_metadata() {
        let args = ffmpeg_video_args("/in", "/out.mp4");
        let joined = args.join(" ");
        // 1080p downscale-only filter (never upscale), preserving aspect.
        assert!(
            joined.contains(
                "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease"
            )
        );
        // H.264 + AAC.
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        // Sane CRF (~23).
        assert!(args.windows(2).any(|w| w == ["-crf", "23"]));
        // Faststart for streaming.
        assert!(args.windows(2).any(|w| w == ["-movflags", "+faststart"]));
        // STRIP all metadata/EXIF/GPS (PII).
        assert!(args.windows(2).any(|w| w == ["-map_metadata", "-1"]));
        // Output filesize cap bounds a decompression-bomb output (video cap).
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-fs" && w[1] == MediaKind::Video.max_upload_bytes().to_string())
        );
        assert_eq!(args.last().unwrap(), "/out.mp4");
    }

    #[test]
    fn ffmpeg_image_args_resize_recompress_strip_metadata() {
        let args = ffmpeg_image_args("/in", "/out.jpg");
        let joined = args.join(" ");
        // Long-edge cap, downscale-only.
        assert!(joined.contains("scale='min(1920,iw)':-2:force_original_aspect_ratio=decrease"));
        // Quality ~80 (qscale 4).
        assert!(args.windows(2).any(|w| w == ["-q:v", "4"]));
        // STRIP metadata.
        assert!(args.windows(2).any(|w| w == ["-map_metadata", "-1"]));
        // Output filesize cap bounds a decompression-bomb output (image cap).
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-fs" && w[1] == MediaKind::Image.max_upload_bytes().to_string())
        );
        assert_eq!(args.last().unwrap(), "/out.jpg");
    }

    #[test]
    fn ffmpeg_video_thumbnail_args_carry_output_filesize_cap() {
        let args = ffmpeg_video_thumbnail_args("/in", "/thumb.jpg");
        // The poster-frame JPEG is bounded by the image output cap.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-fs" && w[1] == MediaKind::Image.max_upload_bytes().to_string())
        );
        assert!(args.windows(2).any(|w| w == ["-map_metadata", "-1"]));
        assert_eq!(args.last().unwrap(), "/thumb.jpg");
    }

    #[test]
    fn ffmpeg_timeout_defaults_per_kind() {
        // With no env override, the per-kind defaults apply (video gets the
        // generous 5 min budget; image the tight 30 s one). The env-override
        // branch is intentionally not exercised here: mutating process env races
        // with other parallel tests, and this is the value the worker uses.
        if std::env::var(FFMPEG_TIMEOUT_ENV).is_err() {
            assert_eq!(
                evidence_ffmpeg_timeout(MediaKind::Video),
                StdDuration::from_secs(FFMPEG_VIDEO_TIMEOUT_SECS)
            );
            assert_eq!(
                evidence_ffmpeg_timeout(MediaKind::Image),
                StdDuration::from_secs(FFMPEG_IMAGE_TIMEOUT_SECS)
            );
        }
    }

    #[test]
    fn ffmpeg_processing_tempdir_is_private_and_removed_on_drop() {
        let dir = tempdir_in_runtime().unwrap();
        let path = dir.path().to_path_buf();
        assert!(
            path.is_dir(),
            "processing tempdir should exist while guard is live"
        );
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mnt-evidence-")),
            "processing tempdir should keep the evidence prefix without tenant identifiers"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "processing tempdir must not be group/world accessible"
            );
        }

        drop(dir);
        assert!(
            !path.exists(),
            "processing tempdir should be removed by the guard even when process returns early"
        );
    }

    #[test]
    fn validate_sniffed_kind_accepts_matching_and_rejects_mismatch_and_garbage() {
        // A real QuickTime header passes for the declared VIDEO kind.
        let qt = quicktime_magic_bytes();
        assert!(validate_sniffed_kind(&qt, MediaKind::Video).is_ok());
        // ...but the SAME video bytes declared as an IMAGE are rejected.
        let err = validate_sniffed_kind(&qt, MediaKind::Image).unwrap_err();
        assert!(err.to_string().contains("does not match declared type"));
        // Unrecognizable/garbage bytes are rejected for any declared kind.
        let err = validate_sniffed_kind(b"raw-original", MediaKind::Image).unwrap_err();
        assert!(err.to_string().contains("does not match declared type"));
    }

    // ===================================================================
    // DB-backed staging + processing lifecycle (status transitions +
    // tenant-prefixed keys). Uses a recording store + stub processor so
    // ffmpeg is never invoked.
    // ===================================================================

    /// Minimal but `infer`-recognizable QuickTime (`video/quicktime`) header: a
    /// 0x14-byte `ftyp` box with the `qt  ` major brand. Lets the processing
    /// tests exercise the post-download content re-validation with bytes that
    /// pass the magic-number sniff for the declared VIDEO kind.
    fn quicktime_magic_bytes() -> Vec<u8> {
        vec![
            0x00, 0x00, 0x00, 0x14, // box size = 20
            0x66, 0x74, 0x79, 0x70, // "ftyp"
            0x71, 0x74, 0x20, 0x20, // major brand "qt  "
            0x00, 0x00, 0x00, 0x00, // minor version
            0x71, 0x74, 0x20, 0x20, // compatible brand "qt  "
        ]
    }

    #[derive(Debug, Clone, Default)]
    struct RecordingStore {
        puts: Arc<Mutex<Vec<String>>>,
        deletes: Arc<Mutex<Vec<String>>>,
        fail_get: bool,
        /// Bytes `get_object` returns for the staging download. `None` yields a
        /// valid QuickTime header so the content re-validation passes.
        staging_bytes: Option<Vec<u8>>,
        /// `size_bytes` the `head_object` stub reports (defaults to 1).
        head_size_bytes: Option<i64>,
    }

    impl S3ObjectStore for RecordingStore {
        fn presign_put(&self, request: PresignPutRequest) -> StorageFuture<'_, PresignedUpload> {
            Box::pin(async move {
                Ok(PresignedUpload {
                    method: "PUT".to_owned(),
                    url: format!("http://storage.local/{}/{}", request.bucket, request.key),
                    headers: vec![],
                    expires_in_secs: request.expires_in.as_secs(),
                })
            })
        }
        fn presign_get(&self, request: PresignGetRequest) -> StorageFuture<'_, String> {
            Box::pin(async move {
                Ok(format!(
                    "http://storage.local/{}/{}?X-Amz-Signature=test",
                    request.bucket, request.key
                ))
            })
        }
        fn copy_object(&self, _request: CopyObjectRequest) -> StorageFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
        fn head_object(&self, _bucket: String, _key: String) -> StorageFuture<'_, ObjectHead> {
            let size_bytes = self.head_size_bytes.unwrap_or(1);
            Box::pin(async move {
                Ok(ObjectHead {
                    size_bytes,
                    e_tag: None,
                    checksum_sha256: None,
                    object_lock_mode: None,
                    retain_until: None,
                })
            })
        }
        fn get_object_retention(
            &self,
            _bucket: String,
            _key: String,
        ) -> StorageFuture<'_, RetentionInfo> {
            Box::pin(async {
                Ok(RetentionInfo {
                    mode: None,
                    retain_until: None,
                })
            })
        }
        fn get_object(&self, _bucket: String, key: String) -> StorageFuture<'_, Vec<u8>> {
            let fail = self.fail_get;
            let bytes = self
                .staging_bytes
                .clone()
                .unwrap_or_else(quicktime_magic_bytes);
            Box::pin(async move {
                if fail {
                    Err(StorageError::S3(format!("missing staging object {key}")))
                } else {
                    Ok(bytes)
                }
            })
        }
        fn put_object(
            &self,
            _bucket: String,
            key: String,
            _content_type: String,
            _body: Vec<u8>,
        ) -> StorageFuture<'_, ()> {
            let puts = self.puts.clone();
            Box::pin(async move {
                puts.lock().unwrap().push(key);
                Ok(())
            })
        }
        fn delete_object(&self, _bucket: String, key: String) -> StorageFuture<'_, ()> {
            let deletes = self.deletes.clone();
            Box::pin(async move {
                deletes.lock().unwrap().push(key);
                Ok(())
            })
        }
    }

    struct StubProcessor;
    impl MediaProcessor for StubProcessor {
        fn process<'a>(
            &'a self,
            kind: MediaKind,
            _original: Vec<u8>,
        ) -> StorageFuture<'a, ProcessedMedia> {
            Box::pin(async move {
                let content_type = match kind {
                    MediaKind::Image => "image/jpeg",
                    MediaKind::Video => "video/mp4",
                };
                Ok(ProcessedMedia {
                    artifact: b"optimized".to_vec(),
                    content_type: content_type.to_owned(),
                    thumbnail: b"thumb".to_vec(),
                })
            })
        }
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn staging_upload_then_process_transitions_to_ready_with_org_prefixed_keys(pool: PgPool) {
        console_platform_request_context::scope_org(OrgId::knl(), async move {
            let seeded = seed_work_order(&pool).await;
            let store = RecordingStore::default();
            let service = EvidenceService::new(
                pool.clone(),
                store.clone(),
                "primary".to_owned(),
                "replica".to_owned(),
            );

            let ticket = service
                .issue_staging_upload(StagingUploadCommand {
                    actor: seeded.uploaded_by,
                    work_order_id: seeded.work_order_id,
                    stage: AttachmentStage::During,
                    content_type: "video/quicktime".to_owned(),
                    size_bytes: 10 * 1024 * 1024,
                    checksum_sha256: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                })
                .await
                .unwrap();

            assert_eq!(ticket.media_kind, MediaKind::Video);
            assert_eq!(ticket.media.processing_status, ProcessingStatus::Processing);
            // Tenant-prefixed staging key.
            let org_prefix = format!("orgs/{}/", OrgId::knl().as_uuid());
            assert!(ticket.upload.url.contains(&org_prefix));
            assert!(
                ticket
                    .media
                    .staging_s3_key
                    .as_deref()
                    .unwrap()
                    .starts_with(&org_prefix)
            );
            assert!(ticket.media.s3_key.starts_with(&org_prefix));

            // The worker claims + processes.
            let job = service.claim_processing_job().await.unwrap().unwrap();
            assert_eq!(job.media_id, ticket.media.id);
            assert_eq!(job.media_kind, MediaKind::Video);
            let status = service
                .process_job(
                    &StubProcessor,
                    &job,
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(status, ProcessingStatus::Ready);

            let media = service.evidence_media(ticket.media.id).await.unwrap();
            assert_eq!(media.processing_status, ProcessingStatus::Ready);
            assert_eq!(media.content_type, "video/mp4");
            assert!(media.thumbnail_s3_key.is_some());
            assert!(media.staging_s3_key.is_none());
            assert!(media.processed_at.is_some());

            // FIX (LOW): the status response serves a presigned GET URL for the
            // thumbnail, never the raw object key. The URL is time-boxed (signed).
            let thumb_url = service.presigned_thumbnail_url(&media).await.unwrap();
            let thumb_url = thumb_url.expect("READY row has a thumbnail URL");
            assert!(thumb_url.contains("X-Amz-Signature"));
            assert!(thumb_url.contains(media.thumbnail_s3_key.as_deref().unwrap()));

            // Final artifact + thumbnail were uploaded under the tenant prefix,
            // and the staging original was deleted.
            let puts = store.puts.lock().unwrap().clone();
            assert!(puts.iter().any(|k| k == &job.final_key));
            assert!(puts.iter().any(|k| k == &job.thumbnail_key));
            assert!(puts.iter().all(|k| k.starts_with(&org_prefix)));
            assert!(store.deletes.lock().unwrap().contains(&job.staging_key));

            // No more pending work for this tenant.
            assert!(service.claim_processing_job().await.unwrap().is_none());
        })
        .await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn process_failure_marks_failed_and_retains_staging(pool: PgPool) {
        console_platform_request_context::scope_org(OrgId::knl(), async move {
            let seeded = seed_work_order(&pool).await;
            let store = RecordingStore {
                fail_get: true,
                ..Default::default()
            };
            let service = EvidenceService::new(
                pool.clone(),
                store.clone(),
                "primary".to_owned(),
                "replica".to_owned(),
            );

            let ticket = service
                .issue_staging_upload(StagingUploadCommand {
                    actor: seeded.uploaded_by,
                    work_order_id: seeded.work_order_id,
                    stage: AttachmentStage::Before,
                    content_type: "image/heic".to_owned(),
                    size_bytes: 1024,
                    checksum_sha256: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                })
                .await
                .unwrap();

            let job = service.claim_processing_job().await.unwrap().unwrap();
            let status = service
                .process_job(
                    &StubProcessor,
                    &job,
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(status, ProcessingStatus::Failed);

            let media = service.evidence_media(ticket.media.id).await.unwrap();
            assert_eq!(media.processing_status, ProcessingStatus::Failed);
            assert!(media.processing_error.is_some());
            // Staging original is RETAINED on failure for retry.
            assert!(media.staging_s3_key.is_some());
            assert!(store.deletes.lock().unwrap().is_empty());
        })
        .await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn process_rejects_content_type_mismatch_before_transcode(pool: PgPool) {
        console_platform_request_context::scope_org(OrgId::knl(), async move {
            let seeded = seed_work_order(&pool).await;
            // Declared an IMAGE, but the staging bytes are a QuickTime VIDEO: the
            // post-download magic-number re-validation must reject this and mark
            // the row FAILED WITHOUT invoking the transcoder.
            let store = RecordingStore {
                staging_bytes: Some(quicktime_magic_bytes()),
                ..Default::default()
            };
            let service = EvidenceService::new(
                pool.clone(),
                store.clone(),
                "primary".to_owned(),
                "replica".to_owned(),
            );

            let ticket = service
                .issue_staging_upload(StagingUploadCommand {
                    actor: seeded.uploaded_by,
                    work_order_id: seeded.work_order_id,
                    stage: AttachmentStage::Before,
                    content_type: "image/jpeg".to_owned(),
                    size_bytes: 1024,
                    checksum_sha256: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                })
                .await
                .unwrap();
            assert_eq!(ticket.media_kind, MediaKind::Image);

            let job = service.claim_processing_job().await.unwrap().unwrap();
            let status = service
                .process_job(
                    &StubProcessor,
                    &job,
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(status, ProcessingStatus::Failed);

            let media = service.evidence_media(ticket.media.id).await.unwrap();
            assert_eq!(media.processing_status, ProcessingStatus::Failed);
            assert!(
                media
                    .processing_error
                    .as_deref()
                    .unwrap()
                    .contains("does not match declared type")
            );
            // No artifact/thumbnail was ever uploaded and the staging original is
            // retained: the transcode never ran.
            assert!(store.puts.lock().unwrap().is_empty());
            assert!(media.staging_s3_key.is_some());
            assert!(store.deletes.lock().unwrap().is_empty());
        })
        .await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn process_rejects_staging_object_over_size_cap(pool: PgPool) {
        console_platform_request_context::scope_org(OrgId::knl(), async move {
            let seeded = seed_work_order(&pool).await;
            // The gateway "lied": the actual staging object HEADs larger than the
            // per-kind image cap. The defensive size check must reject it before
            // the in-memory download and mark the row FAILED.
            let store = RecordingStore {
                head_size_bytes: Some(MediaKind::Image.max_upload_bytes() + 1),
                ..Default::default()
            };
            let service = EvidenceService::new(
                pool.clone(),
                store.clone(),
                "primary".to_owned(),
                "replica".to_owned(),
            );

            let ticket = service
                .issue_staging_upload(StagingUploadCommand {
                    actor: seeded.uploaded_by,
                    work_order_id: seeded.work_order_id,
                    stage: AttachmentStage::Before,
                    content_type: "image/jpeg".to_owned(),
                    size_bytes: 1024,
                    checksum_sha256: None,
                    trace: TraceContext::generate(),
                    occurred_at: OffsetDateTime::now_utc(),
                })
                .await
                .unwrap();

            let job = service.claim_processing_job().await.unwrap().unwrap();
            let status = service
                .process_job(
                    &StubProcessor,
                    &job,
                    TraceContext::generate(),
                    OffsetDateTime::now_utc(),
                )
                .await
                .unwrap();
            assert_eq!(status, ProcessingStatus::Failed);

            let media = service.evidence_media(ticket.media.id).await.unwrap();
            assert_eq!(media.processing_status, ProcessingStatus::Failed);
            assert!(
                media
                    .processing_error
                    .as_deref()
                    .unwrap()
                    .contains("exceeding")
            );
            assert!(store.puts.lock().unwrap().is_empty());
            assert!(media.staging_s3_key.is_some());
        })
        .await;
    }
}
