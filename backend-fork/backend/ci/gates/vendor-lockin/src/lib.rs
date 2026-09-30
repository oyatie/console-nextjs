//! Vendor lock-in phase-out gate.
//!
//! The gate scans git-tracked repository text for managed-cloud seams that have
//! a known portability/phase-out contract, then requires each detected seam to
//! have a conforming entry in `registry/vendor-lockin-phaseout/index.json`.
//! A conforming entry names the replacement path, the kernel trait that isolates
//! the seam, and at least two non-empty adapters so OCI remains one supported
//! implementation rather than a hard dependency.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

const REGISTRY_ENTRY_TYPE: &str = "managed_cloud_dependency_phaseout";
const REGISTRY_REL_PATH: &str = "registry/vendor-lockin-phaseout/index.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct Registry {
    pub schema_version: u32,
    pub entry_type: String,
    pub entries: Vec<RegistryEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct RegistryEntry {
    pub name: String,
    pub replacement_path: String,
    pub kernel_trait: String,
    pub adapters: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextArtifact {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectedSeam {
    pub dependency: String,
    pub marker: String,
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ViolationKind {
    MissingRegistryEntry,
    MissingReplacementPath,
    MissingKernelTrait,
    TooFewAdapters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub dependency: String,
    pub path: PathBuf,
    pub line: usize,
    pub marker: String,
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ViolationKind::MissingRegistryEntry => "MISSING_REGISTRY_ENTRY",
            ViolationKind::MissingReplacementPath => "MISSING_REPLACEMENT_PATH",
            ViolationKind::MissingKernelTrait => "MISSING_KERNEL_TRAIT",
            ViolationKind::TooFewAdapters => "TOO_FEW_ADAPTERS",
        };
        write!(
            f,
            "[{kind}] {} at {}:{} matched `{}`: {}",
            self.dependency,
            self.path.display(),
            self.line,
            self.marker,
            self.detail
        )
    }
}

#[derive(Debug, Default)]
pub struct GateResult {
    pub violations: Vec<Violation>,
}

impl GateResult {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

struct SeamPattern {
    dependency: &'static str,
    markers: &'static [&'static str],
}

// Keep this list intentionally scoped to managed-cloud seams with explicit
// replacement contracts in registry/vendor-lockin-phaseout. Broad IaC tiering
// (for example every `oci_core_*` network/compute primitive) belongs to
// console-gate-iac-tier; this gate enforces phase-out registry hygiene for seams
// such as object storage, vault/secrets, and managed logging.
const MANAGED_CLOUD_SEAMS: &[SeamPattern] = &[
    SeamPattern {
        dependency: "oci-logging",
        markers: &["OCI Logging", "oci_logging_", "oci logging", "oci-logging"],
    },
    SeamPattern {
        dependency: "oci-object-storage",
        markers: &[
            "OCI Object Storage",
            "oci_objectstorage_",
            "objectstorage.",
            "oci-objectstore",
            "oci os bucket",
        ],
    },
    SeamPattern {
        dependency: "oci-vault",
        markers: &[
            "OCI Vault",
            "oci_kms_vault",
            "oci_vault",
            "oci_secrets_",
            "oci-vault",
        ],
    },
];

#[must_use]
pub fn registry_rel_path() -> &'static str {
    REGISTRY_REL_PATH
}

pub fn load_registry(path: &Path) -> Result<Registry, Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read vendor lock-in registry {}: {e}",
            path.display()
        )
    })?;
    let registry = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "cannot parse vendor lock-in registry {}: {e}",
            path.display()
        )
    })?;
    Ok(registry)
}

#[must_use]
pub fn audit(artifacts: &[TextArtifact], registry: &Registry) -> GateResult {
    let detected = detect_managed_cloud_seams(artifacts);
    let entries = registry
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut violations = Vec::new();

    for seam in detected {
        let Some(entry) = entries.get(seam.dependency.as_str()) else {
            violations.push(violation(
                ViolationKind::MissingRegistryEntry,
                &seam,
                format!(
                    "missing registry entry in {REGISTRY_REL_PATH}; add `{}` with replacement_path, kernel_trait, and at least two adapters",
                    seam.dependency
                ),
            ));
            continue;
        };

        if entry.replacement_path.trim().is_empty() {
            violations.push(violation(
                ViolationKind::MissingReplacementPath,
                &seam,
                format!(
                    "registry entry `{}` is missing replacement_path",
                    seam.dependency
                ),
            ));
        }
        if entry.kernel_trait.trim().is_empty() {
            violations.push(violation(
                ViolationKind::MissingKernelTrait,
                &seam,
                format!(
                    "registry entry `{}` is missing kernel_trait",
                    seam.dependency
                ),
            ));
        }
        let adapter_count = entry
            .adapters
            .iter()
            .filter(|adapter| !adapter.trim().is_empty())
            .count();
        if adapter_count < 2 {
            violations.push(violation(
                ViolationKind::TooFewAdapters,
                &seam,
                format!(
                    "registry entry `{}` must declare at least two adapters; found {adapter_count}",
                    seam.dependency
                ),
            ));
        }
    }

    if registry.entry_type != REGISTRY_ENTRY_TYPE {
        for seam in detect_managed_cloud_seams(artifacts) {
            violations.push(violation(
                ViolationKind::MissingRegistryEntry,
                &seam,
                format!(
                    "registry entry_type must be `{REGISTRY_ENTRY_TYPE}`; found `{}`",
                    registry.entry_type
                ),
            ));
        }
    }

    violations.sort_by(|a, b| {
        a.dependency
            .cmp(&b.dependency)
            .then(a.kind.cmp(&b.kind))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    GateResult { violations }
}

#[must_use]
pub fn detect_managed_cloud_seams(artifacts: &[TextArtifact]) -> Vec<DetectedSeam> {
    let mut detected = BTreeMap::<&'static str, DetectedSeam>::new();

    for artifact in artifacts {
        for seam in MANAGED_CLOUD_SEAMS {
            if detected.contains_key(seam.dependency) {
                continue;
            }
            if let Some((marker, line)) = first_marker_match(&artifact.contents, seam.markers) {
                detected.insert(
                    seam.dependency,
                    DetectedSeam {
                        dependency: seam.dependency.to_owned(),
                        marker: marker.to_owned(),
                        path: artifact.path.clone(),
                        line,
                    },
                );
            }
        }
    }

    detected.into_values().collect()
}

fn first_marker_match<'a>(contents: &str, markers: &'a [&str]) -> Option<(&'a str, usize)> {
    for (line_index, line) in contents.lines().enumerate() {
        let normalized = line.to_ascii_lowercase();
        for marker in markers {
            if normalized.contains(&marker.to_ascii_lowercase()) {
                return Some((marker, line_index.saturating_add(1)));
            }
        }
    }
    None
}

fn violation(kind: ViolationKind, seam: &DetectedSeam, detail: String) -> Violation {
    Violation {
        kind,
        dependency: seam.dependency.clone(),
        path: seam.path.clone(),
        line: seam.line,
        marker: seam.marker.clone(),
        detail,
    }
}

pub fn git_root(dir: &Path) -> Result<PathBuf, String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to run git rev-parse: {e}"))?;
    if !out.status.success() {
        return Err("git rev-parse --show-toplevel failed".to_owned());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_owned(),
    ))
}

pub fn git_tracked_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let out = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files failed".to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|line| root.join(line))
        .collect())
}

pub fn collect_artifacts(repo_root: &Path) -> Result<Vec<TextArtifact>, String> {
    let mut artifacts = Vec::new();
    for path in git_tracked_files(repo_root)? {
        let rel = match path.strip_prefix(repo_root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => path.clone(),
        };
        if rel == Path::new(REGISTRY_REL_PATH) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        artifacts.push(TextArtifact {
            path: rel,
            contents,
        });
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(path: &str, contents: &str) -> TextArtifact {
        TextArtifact {
            path: PathBuf::from(path),
            contents: contents.to_owned(),
        }
    }

    #[test]
    fn detector_canonicalizes_known_oci_managed_cloud_markers() {
        let seams = detect_managed_cloud_seams(&[
            artifact("deploy/SECRETS.md", "current source is OCI Vault"),
            artifact(
                "deploy/opentofu/storage.tf",
                "resource \"oci_objectstorage_bucket\" \"db_backups\" {}",
            ),
            artifact("ops/logging.md", "OCI Logging feeds must be replaceable"),
        ]);

        let names = seams
            .iter()
            .map(|seam| seam.dependency.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["oci-logging", "oci-object-storage", "oci-vault"]
        );
    }
}
