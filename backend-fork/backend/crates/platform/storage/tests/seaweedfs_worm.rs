use console_platform_storage::{S3ObjectStore, S3StorageConfig, SeaweedS3Storage};
use std::io;
use time::{Duration, OffsetDateTime};

#[tokio::test]
#[ignore = "requires docker-compose SeaweedFS S3 endpoint"]
async fn seaweedfs_compliance_retention_protects_locked_object_version()
-> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::var("CONSOLE_STORAGE_TEST_S3_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:18333".to_owned());
    let bucket = format!("worm-{}", uuid::Uuid::new_v4());
    let key = format!("evidence/{}.txt", uuid::Uuid::new_v4());
    let storage = SeaweedS3Storage::from_config(&S3StorageConfig {
        endpoint_url: endpoint,
        region: "us-east-1".to_owned(),
        access_key_id: "test".to_owned(),
        secret_access_key: "test".to_owned(),
        primary_bucket: bucket.clone(),
        replica_bucket: bucket.clone(),
        force_path_style: true,
    })
    .await?;

    storage.create_bucket(&bucket, true).await?;
    let put = storage
        .put_bytes_with_result(&bucket, &key, "text/plain", b"original evidence".to_vec())
        .await?;
    let version_id = put
        .version_id
        .ok_or_else(|| io::Error::other("SeaweedFS did not return a version id"))?;
    let retain_until = OffsetDateTime::now_utc() + Duration::days(1);
    storage
        .put_compliance_retention(&bucket, &key, retain_until)
        .await?;

    let retention = storage
        .get_object_retention(bucket.clone(), key.clone())
        .await?;
    assert_eq!(retention.mode.as_deref(), Some("COMPLIANCE"));
    assert!(retention.retain_until.is_some());

    let delete_result = storage
        .delete_object_version(&bucket, &key, &version_id)
        .await;
    assert!(
        delete_result.is_err(),
        "COMPLIANCE retention must deny deleting the locked version before retain-until"
    );

    let second_put = storage
        .put_bytes_with_result(&bucket, &key, "text/plain", b"tampered evidence".to_vec())
        .await?;
    let original = storage
        .head_object_version(&bucket, &key, &version_id)
        .await?;
    assert_eq!(original.size_bytes, 17);
    assert_eq!(original.object_lock_mode.as_deref(), Some("COMPLIANCE"));
    assert!(original.retain_until.is_some());
    assert_ne!(
        second_put.version_id.as_deref(),
        Some(version_id.as_str()),
        "same-key put must not mutate the locked version"
    );
    Ok(())
}
