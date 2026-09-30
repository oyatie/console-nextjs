//! IaC tier-discipline gate — keeps cloud-vendor lock-in out of app manifests.
//!
//! Lifted and adapted from oyatie's `oya-check-iac-tier-discipline`
//! (ADR-0202 three-tier IaC model) for maintenance's `deploy/` layout:
//!
//! - Tier A — ArgoCD/Kustomize app manifests: `deploy/argocd/**`, `deploy/apps/**`.
//! - Tier B — OpenTofu cloud primitives: `deploy/opentofu/**`.
//!
//! oyatie also models a Tier C (Cluster API) and two further violation kinds:
//! `ArgocdProjectBootstrappedFromTierA` and `TerraformResidual`. Neither is
//! ported here — maintenance has no Cluster API tier, no Terraform migration
//! history (its `deploy/opentofu` module has always been OpenTofu), and its
//! ArgoCD `AppProject` (`deploy/argocd/project.yaml`) is deliberately
//! self-bootstrapped from Tier A (a legitimate app-of-apps pattern, not a
//! lock-in smell) — porting that check would flag a false positive on the
//! current tree. The two ported checks are the ones the boundary is actually
//! for: cloud-vendor primitives must live in Tier B, not Tier A, and vice
//! versa for per-pod manifests.
//!
//! Violations:
//! - `OpenTofuDefinesPodManifest` — a Tier-B `.tf` file declares a
//!   `kubernetes_*` resource; per-pod manifests belong to Tier A.
//! - `ArgocdAppReferencesCloudPrimitive` — a Tier-A YAML file names a
//!   cloud-vendor resource kind (`aws_*` / `google_*` / `azurerm_*` / `oci_*`)
//!   that must come from Tier B instead.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum IacTier {
    /// Tier A — ArgoCD/Kustomize: app deploy manifests.
    TierAArgoCd,
    /// Tier B — OpenTofu: cloud-side resources.
    TierBOpenTofu,
}

impl fmt::Display for IacTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IacTier::TierAArgoCd => f.write_str("tier-a-argocd"),
            IacTier::TierBOpenTofu => f.write_str("tier-b-opentofu"),
        }
    }
}

/// One IaC artifact under audit. Tier drives which boundary rules apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IacArtifact {
    pub tier: IacTier,
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ViolationKind {
    /// Tier-B artifact declares a per-pod manifest (Tier-A territory).
    OpenTofuDefinesPodManifest,
    /// Tier-A artifact references a cloud primitive (Tier-B territory).
    ArgocdAppReferencesCloudPrimitive,
}

/// Violation record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Violation {
    pub tier: IacTier,
    pub path: PathBuf,
    pub kind: ViolationKind,
    pub summary: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            ViolationKind::OpenTofuDefinesPodManifest => "OPENTOFU_DEFINES_POD_MANIFEST",
            ViolationKind::ArgocdAppReferencesCloudPrimitive => {
                "ARGOCD_APP_REFERENCES_CLOUD_PRIMITIVE"
            }
        };
        write!(
            f,
            "[{kind}] {} ({}): {}",
            self.path.display(),
            self.tier,
            self.summary
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

// Tier-A territory markers that must NOT appear in a Tier-B artifact.
const POD_MANIFEST_KINDS: &[&str] = &[
    "kubernetes_deployment",
    "kubernetes_stateful_set",
    "kubernetes_daemon_set",
    "kubernetes_pod",
    "kubernetes_replica_set",
];

// Tier-B territory markers that must NOT appear in a Tier-A artifact.
// Ported verbatim from oyatie's CLOUD_PRIMITIVE_KINDS, plus `oci_` (maintenance's
// cloud is OCI — see deploy/opentofu/providers.tf's `oracle/oci` provider —
// and oyatie's AWS/GCP/Azure-only list predates that).
const CLOUD_PRIMITIVE_KINDS: &[&str] = &[
    "aws_iam_role",
    "aws_iam_policy",
    "aws_vpc",
    "aws_subnet",
    "aws_kms_key",
    "aws_route53_zone",
    "aws_rds_instance",
    "aws_sesv2_email_identity",
    "google_compute_network",
    "google_kms_key_ring",
    "azurerm_virtual_network",
    "azurerm_key_vault",
    "oci_",
];

/// Audit a batch of artifacts against the tier boundary table.
#[must_use]
pub fn audit(artifacts: &[IacArtifact]) -> GateResult {
    let mut violations: Vec<Violation> = Vec::new();

    for art in artifacts {
        match art.tier {
            IacTier::TierBOpenTofu => {
                for kind in POD_MANIFEST_KINDS {
                    let needle = format!("resource \"{kind}\"");
                    if art.contents.contains(&needle) {
                        violations.push(Violation {
                            tier: art.tier,
                            path: art.path.clone(),
                            kind: ViolationKind::OpenTofuDefinesPodManifest,
                            summary: format!(
                                "Tier-B OpenTofu artifact declares `{kind}` — per-pod manifests belong to Tier-A (ArgoCD)"
                            ),
                        });
                    }
                }
            }
            IacTier::TierAArgoCd => {
                for kind in CLOUD_PRIMITIVE_KINDS {
                    if art.contents.contains(kind) {
                        violations.push(Violation {
                            tier: art.tier,
                            path: art.path.clone(),
                            kind: ViolationKind::ArgocdAppReferencesCloudPrimitive,
                            summary: format!(
                                "Tier-A ArgoCD artifact references cloud primitive `{kind}` — belongs to Tier-B (OpenTofu)"
                            ),
                        });
                    }
                }
            }
        }
    }

    violations.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(a.kind.cmp(&b.kind))
            .then(a.path.cmp(&b.path))
    });
    GateResult { violations }
}

// ---------------------------------------------------------------------------
// Repo discovery + artifact collection
// ---------------------------------------------------------------------------

/// Resolve the git repository root containing `dir` (works from any subdir).
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

/// Lists git-tracked files under `root`, returned as absolute paths.
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
        .map(|l| root.join(l))
        .collect())
}

/// Walk the git-tracked files under `repo_root` and classify the ones that
/// live in a known IaC tier directory into `IacArtifact`s. Everything else
/// (docs, examples, `.gitignore`, non-manifest files) is skipped.
pub fn collect_artifacts(repo_root: &Path) -> Result<Vec<IacArtifact>, String> {
    let mut artifacts = Vec::new();
    for path in git_tracked_files(repo_root)? {
        let rel = path.strip_prefix(repo_root).unwrap_or(path.as_path());
        let rel_str = rel.to_string_lossy();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let tier = if (rel_str.starts_with("deploy/argocd/") || rel_str.starts_with("deploy/apps/"))
            && matches!(ext, "yaml" | "yml")
        {
            Some(IacTier::TierAArgoCd)
        } else if rel_str.starts_with("deploy/opentofu/") && matches!(ext, "tf" | "tofu") {
            Some(IacTier::TierBOpenTofu)
        } else {
            None
        };

        let Some(tier) = tier else { continue };
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue; // binary/unreadable — not a text IaC artifact
        };
        artifacts.push(IacArtifact {
            tier,
            path: rel.to_path_buf(),
            contents,
        });
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(tier: IacTier, path: &str, contents: &str) -> IacArtifact {
        IacArtifact {
            tier,
            path: PathBuf::from(path),
            contents: contents.into(),
        }
    }

    #[test]
    fn clean_tiers_have_no_violations() {
        let arts = vec![
            art(
                IacTier::TierBOpenTofu,
                "deploy/opentofu/network.tf",
                "resource \"oci_core_vcn\" \"mnt\" { cidr_block = \"10.0.0.0/16\" }",
            ),
            art(
                IacTier::TierAArgoCd,
                "deploy/argocd/apps/console.yaml",
                "apiVersion: argoproj.io/v1alpha1\nkind: Application\nspec:\n  source: { repoURL: 'maintenance' }",
            ),
        ];
        let result = audit(&arts);
        assert!(
            result.passed(),
            "expected clean, got {:?}",
            result.violations
        );
    }

    #[test]
    fn opentofu_defining_pod_manifest_is_flagged() {
        let arts = vec![art(
            IacTier::TierBOpenTofu,
            "deploy/opentofu/wrong.tf",
            "resource \"kubernetes_deployment\" \"app\" { metadata { name = \"x\" } }",
        )];
        let result = audit(&arts);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::OpenTofuDefinesPodManifest
        );
    }

    #[test]
    fn argocd_app_referencing_aws_iam_role_is_flagged() {
        let arts = vec![art(
            IacTier::TierAArgoCd,
            "deploy/argocd/apps/bad.yaml",
            "spec:\n  source:\n    helm:\n      values: |\n        roleArn: arn:aws:iam aws_iam_role/foo",
        )];
        let result = audit(&arts);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::ArgocdAppReferencesCloudPrimitive
        );
    }

    #[test]
    fn argocd_app_referencing_oci_primitive_is_flagged() {
        let arts = vec![art(
            IacTier::TierAArgoCd,
            "deploy/apps/console/base/bad.yaml",
            "spec:\n  values: |\n    bucket: oci_objectstorage_bucket.evidence.name\n",
        )];
        let result = audit(&arts);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0].kind,
            ViolationKind::ArgocdAppReferencesCloudPrimitive
        );
    }

    #[test]
    fn violations_are_sorted_by_tier_kind_path() {
        let arts = vec![
            art(IacTier::TierAArgoCd, "z-late.yaml", "aws_vpc"),
            art(
                IacTier::TierBOpenTofu,
                "a-early.tf",
                "resource \"kubernetes_pod\" \"x\" {}",
            ),
            art(IacTier::TierAArgoCd, "m-middle.yaml", "aws_vpc"),
        ];
        let result = audit(&arts);
        assert_eq!(result.violations.len(), 3);
        // TierAArgoCd < TierBOpenTofu in our enum ordering — Tier A first.
        assert_eq!(result.violations[0].tier, IacTier::TierAArgoCd);
        assert_eq!(result.violations[1].tier, IacTier::TierAArgoCd);
        assert_eq!(result.violations[2].tier, IacTier::TierBOpenTofu);
        assert_eq!(result.violations[0].path, PathBuf::from("m-middle.yaml"));
        assert_eq!(result.violations[1].path, PathBuf::from("z-late.yaml"));
    }

    /// Regression proof: the real `deploy/` tree on this branch must stay
    /// clean. This is the CI-native form of "the gate must pass on current
    /// main" — a false positive here means the adaptation is too aggressive,
    /// not a real finding.
    #[test]
    fn real_deploy_tree_has_no_violations() -> Result<(), String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let root = git_root(&cwd)?;
        let artifacts = collect_artifacts(&root)?;
        assert!(
            artifacts.len() > 10,
            "expected multiple deploy/ IaC artifacts to be discovered, got {}",
            artifacts.len()
        );
        let result = audit(&artifacts);
        assert!(
            result.passed(),
            "expected clean deploy/ tree, got: {:?}",
            result
                .violations
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
        );
        Ok(())
    }
}
