use console_gate_vendor_lockin::{
    Registry, RegistryEntry, TextArtifact, ViolationKind, audit, load_registry,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn artifact(contents: &str) -> TextArtifact {
    TextArtifact {
        path: PathBuf::from("deploy/opentofu/storage.tf"),
        contents: contents.to_owned(),
    }
}

fn registry(entries: Vec<RegistryEntry>) -> Registry {
    Registry {
        schema_version: 1,
        entry_type: "managed_cloud_dependency_phaseout".to_owned(),
        entries,
    }
}

fn entry(
    name: &str,
    replacement_path: &str,
    kernel_trait: &str,
    adapters: &[&str],
) -> RegistryEntry {
    RegistryEntry {
        name: name.to_owned(),
        replacement_path: replacement_path.to_owned(),
        kernel_trait: kernel_trait.to_owned(),
        adapters: adapters
            .iter()
            .map(|adapter| (*adapter).to_owned())
            .collect(),
    }
}

#[test]
fn valid_registered_managed_cloud_seam_passes() {
    let result = audit(
        &[artifact(
            "resource \"oci_objectstorage_bucket\" \"evidence\" { name = \"mnt-evidence\" }",
        )],
        &registry(vec![entry(
            "oci-object-storage",
            "self-hosted-s3-compatible-object-storage",
            "S3ObjectStore",
            &["oci-object-storage-s3-adapter", "self-hosted-s3-adapter"],
        )]),
    );

    assert!(
        result.passed(),
        "expected registered object-storage seam to pass, got: {:?}",
        result.violations
    );
}

#[test]
fn missing_registry_entry_identifies_offending_dependency() {
    let result = audit(
        &[artifact(
            "resource \"oci_objectstorage_bucket\" \"evidence\" { name = \"mnt-evidence\" }",
        )],
        &registry(Vec::new()),
    );

    assert!(!result.passed(), "expected missing registry entry to fail");
    assert_eq!(
        result.violations[0].kind,
        ViolationKind::MissingRegistryEntry
    );
    let rendered = result.violations[0].to_string();
    assert!(rendered.contains("oci-object-storage"), "{rendered}");
    assert!(rendered.contains("missing registry entry"), "{rendered}");
}

#[test]
fn missing_replacement_path_identifies_requirement() {
    let result = audit(
        &[artifact(
            "production runbook references OCI Object Storage for backups",
        )],
        &registry(vec![entry(
            "oci-object-storage",
            "",
            "S3ObjectStore",
            &["oci-object-storage-s3-adapter", "self-hosted-s3-adapter"],
        )]),
    );

    assert!(
        !result.passed(),
        "expected malformed registry entry to fail"
    );
    assert_eq!(
        result.violations[0].kind,
        ViolationKind::MissingReplacementPath
    );
    let rendered = result.violations[0].to_string();
    assert!(rendered.contains("oci-object-storage"), "{rendered}");
    assert!(rendered.contains("replacement_path"), "{rendered}");
}

#[test]
fn missing_kernel_trait_identifies_requirement() {
    let result = audit(
        &[artifact(
            "deploy/SECRETS.md documents OCI Vault as the current source",
        )],
        &registry(vec![entry(
            "oci-vault",
            "openbao-external-secrets",
            "",
            &["oci-vault-adapter", "openbao-adapter"],
        )]),
    );

    assert!(
        !result.passed(),
        "expected malformed registry entry to fail"
    );
    assert_eq!(result.violations[0].kind, ViolationKind::MissingKernelTrait);
    let rendered = result.violations[0].to_string();
    assert!(rendered.contains("oci-vault"), "{rendered}");
    assert!(rendered.contains("kernel_trait"), "{rendered}");
}

#[test]
fn fewer_than_two_adapters_identifies_requirement() {
    let result = audit(
        &[artifact("OCI Logging exports must have a replacement")],
        &registry(vec![entry(
            "oci-logging",
            "opentelemetry-lgtm-stack",
            "TelemetrySink",
            &["oci-logging-adapter"],
        )]),
    );

    assert!(
        !result.passed(),
        "expected malformed registry entry to fail"
    );
    assert_eq!(result.violations[0].kind, ViolationKind::TooFewAdapters);
    let rendered = result.violations[0].to_string();
    assert!(rendered.contains("oci-logging"), "{rendered}");
    assert!(rendered.contains("at least two adapters"), "{rendered}");
}

#[test]
fn load_registry_reads_phaseout_index_json() -> Result<(), Box<dyn std::error::Error>> {
    let dir = temp_workspace("load-registry")?;
    let registry_path = dir.join("registry/vendor-lockin-phaseout/index.json");
    write_file(
        &registry_path,
        r#"{
  "schema_version": 1,
  "entry_type": "managed_cloud_dependency_phaseout",
  "entries": [
    {
      "name": "oci-vault",
      "replacement_path": "openbao-external-secrets",
      "kernel_trait": "SecretStore",
      "adapters": ["oci-vault-adapter", "openbao-adapter"]
    }
  ]
}
"#,
    )?;

    let loaded = load_registry(&registry_path)?;
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].name, "oci-vault");
    Ok(())
}

#[test]
fn binary_exits_non_zero_on_registry_violation() -> Result<(), Box<dyn std::error::Error>> {
    let repo = temp_workspace("binary-red")?;
    run(&repo, "git", &["init", "-q"])?;
    write_file(
        &repo.join("deploy/opentofu/storage.tf"),
        "resource \"oci_objectstorage_bucket\" \"evidence\" { name = \"mnt-evidence\" }\n",
    )?;
    write_file(
        &repo.join("registry/vendor-lockin-phaseout/index.json"),
        r#"{
  "schema_version": 1,
  "entry_type": "managed_cloud_dependency_phaseout",
  "entries": []
}
"#,
    )?;
    run(
        &repo,
        "git",
        &[
            "add",
            "deploy/opentofu/storage.tf",
            "registry/vendor-lockin-phaseout/index.json",
        ],
    )?;

    let mut command = git_fixture_command(env!("CARGO_BIN_EXE_console-gate-vendor-lockin"));
    let output = command.current_dir(&repo).output()?;

    assert!(
        !output.status.success(),
        "expected binary to fail on missing registry entry"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("oci-object-storage"), "{stderr}");
    assert!(stderr.contains("missing registry entry"), "{stderr}");
    Ok(())
}

fn temp_workspace(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!(
        "console-gate-vendor-lockin-{name}-{}",
        std::process::id()
    ));
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_file(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn git_fixture_command(program: &str) -> Command {
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    command
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let output = git_fixture_command(program)
        .args(args)
        .current_dir(dir)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{program} {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}
