#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("check-production-promotion-authority.py")
ROLLBACK_FLOOR = "f6ff236b9770c79301a3d07da6afb56be1e27bbf"
AUTH_PATH = "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json"
EVIDENCE_PATH = "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json"
OVERLAY_PATH = "deploy/apps/console/overlays/prod/kustomization.yaml"

GIT_FIXTURE_DEFAULTS = {
    "GIT_ASKPASS": "/bin/false",
    "GIT_CONFIG_COUNT": "0",
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_CONFIG_SYSTEM": "/dev/null",
    "GIT_NO_REPLACE_OBJECTS": "1",
    "GIT_TERMINAL_PROMPT": "0",
    "LC_ALL": "C",
}


def git_fixture_environment(overrides: dict[str, str] | None = None) -> dict[str, str]:
    environment = {
        name: value for name, value in os.environ.items() if not name.startswith("GIT_")
    }
    environment.update(GIT_FIXTURE_DEFAULTS)
    environment.update(overrides or {})
    return environment


class PromotionAuthorityTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.remote = self.root / "remote.git"
        self.repo = self.root / "repo"
        subprocess.run(
            ["git", "init", "--bare", str(self.remote)],
            check=True,
            capture_output=True,
            env=git_fixture_environment(),
        )
        subprocess.run(
            ["git", "init", "-b", "dev", str(self.repo)],
            check=True,
            capture_output=True,
            env=git_fixture_environment(),
        )
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        self.git("remote", "add", "origin", str(self.remote))
        self.write(
            "docs/release/PR-473-EXPAND-CONTRACT.gate.json",
            json.dumps(
                {
                    "schema_version": 1,
                    "pull_request": 473,
                    "rollback_floor": ROLLBACK_FLOOR,
                    "release_phase": "expand",
                    "deployment_authorized": False,
                    "command_only_claim_authorized": False,
                    "production_authority": {
                        "production_cardinality": False,
                        "old_runtime_drain": False,
                        "rollback_floor_raise": False,
                    },
                    "guarded_tests": [],
                }
            ),
        )
        self.write(EVIDENCE_PATH, json.dumps({"status": "TEMPLATE_NOT_EVIDENCE"}) + "\n")
        self.write(OVERLAY_PATH, "images: []\n")
        self.write_authorization(False)
        self.git("add", ".")
        self.git("commit", "-m", "candidate source with false authorization")
        self.candidate_sha = self.git("rev-parse", "HEAD")

        self.write_evidence()
        self.write_authorization(False)
        self.git("add", EVIDENCE_PATH, AUTH_PATH)
        self.git("commit", "-m", "prepare immutable production evidence")
        self.evidence_sha = self.git("rev-parse", "HEAD")

        self.write_authorization(True)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "authorize once")
        self.authorized_sha = self.git("rev-parse", "HEAD")
        self.git("push", "-u", "origin", "dev")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def git(self, *args: str, check: bool = True) -> str:
        return subprocess.run(
            ["git", *args],
            cwd=self.repo,
            check=check,
            capture_output=True,
            text=True,
            env=git_fixture_environment(),
        ).stdout.strip()

    def write(self, path: str, value: str) -> None:
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value, encoding="utf-8")

    def evidence(self) -> dict[str, object]:
        return {
            "schema_version": 1,
            "target": "production",
            "release_phase": "expand",
            "candidate_source_sha": self.candidate_sha,
            "observed_running_revision": self.candidate_sha,
            "observed_database_topology": {
                "cluster_name": "console-db",
                "namespace": "console",
                "writer_endpoint": "console-db-rw.console.svc.cluster.local",
                "reader_endpoint": "console-db-ro.console.svc.cluster.local",
                "instances": [
                    {"name": "console-db-1", "role": "primary", "ready": True, "zone": "zone-a"},
                    {"name": "console-db-2", "role": "replica", "ready": True, "zone": "zone-b"},
                    {"name": "console-db-3", "role": "replica", "ready": True, "zone": "zone-c"},
                ],
            },
            "capacity_headroom": {
                "window_started_at": "2026-07-19T10:00:00Z",
                "window_ended_at": "2026-07-19T11:00:00Z",
                "cpu_peak_percent": 55.5,
                "memory_peak_percent": 60,
                "storage_used_percent": 45,
                "connection_peak": 30,
                "connection_limit": 100,
                "minimum_headroom_percent": 25,
            },
            "backup_restore_proof": {
                "backup_id": "backup-20260719-1100",
                "backup_completed_at": "2026-07-19T11:05:00Z",
                "isolated_restore_id": "restore-20260719-1110",
                "isolated_restore_completed_at": "2026-07-19T11:20:00Z",
                "restored_revision": self.candidate_sha,
                "validation_checks": ["schema inventory matches", "application read smoke passes"],
            },
            "evidence_author": {
                "github_login": "production-evidence-author",
                "identity_provider_subject": "oidc:production:evidence-author",
            },
            "independent_reviewer": {
                "github_login": "production-independent-reviewer",
                "identity_provider_subject": "oidc:production:independent-reviewer",
                "team_id": 424242,
            },
            "charter": {
                "charter_id": "oyatie-production-change-authority-v1",
                "trust_domain_id": "oyatie-production-independent-review",
            },
            "observed_at": "2026-07-19T11:25:00Z",
            "prepared_at": "2026-07-19T11:30:00Z",
            "reviewed_at": "2026-07-19T12:00:00Z",
        }

    def write_evidence(self, mutate=None) -> None:
        evidence = self.evidence()
        if mutate:
            mutate(evidence)
        self.write(EVIDENCE_PATH, json.dumps(evidence, indent=2) + "\n")

    def authorization(self, authorized: bool) -> dict[str, object]:
        digest = hashlib.sha256((self.repo / EVIDENCE_PATH).read_bytes()).hexdigest()
        return {
            "schema_version": 2,
            "pull_request": 473,
            "target": "production",
            "release_phase": "expand",
            "rollback_floor": ROLLBACK_FLOOR,
            "desired_state_authority_cutover": False,
            "deployment_authorized": authorized,
            "command_only": False,
            "production_cardinality_evidence": {
                "path": EVIDENCE_PATH,
                "sha256": digest,
                "verified": authorized,
            },
            "contract_authorities": {
                "old_runtime_drain": False,
                "rollback_floor_raise": False,
            },
        }

    def write_authorization(self, authorized: bool, mutate=None) -> None:
        record = self.authorization(authorized)
        if mutate:
            mutate(record)
        self.write(AUTH_PATH, json.dumps(record, indent=2) + "\n")

    def run_gate(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python3", str(SCRIPT), *args],
            cwd=self.repo,
            capture_output=True,
            text=True,
            env=git_fixture_environment(),
        )

    def initial(self, sha: str | None = None) -> subprocess.CompletedProcess[str]:
        return self.run_gate(
            "initial",
            "--expected-sha",
            sha or self.authorized_sha,
            "--expected-ref",
            "refs/heads/dev",
            "--require-local-branch",
        )

    def force_remote(self, sha: str) -> None:
        self.git("push", "--force", "origin", f"{sha}:dev")

    def commit_bad_evidence(self, mutate) -> str:
        self.git("reset", "--hard", self.candidate_sha)
        self.write_evidence(mutate)
        self.write_authorization(False)
        self.git("add", EVIDENCE_PATH, AUTH_PATH)
        self.git("commit", "-m", "bad evidence preparation")
        self.write_authorization(True)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "authorize bad evidence")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        return sha

    def prepare_promotion_commit(self, *, overlay: bool = False) -> str:
        self.git("reset", "--hard", self.authorized_sha)
        self.force_remote(self.authorized_sha)
        result = self.run_gate("reset", "--expected-sha", self.authorized_sha)
        self.assertEqual(result.returncode, 0, result.stderr)
        if overlay:
            self.write(OVERLAY_PATH, "images: [changed]\n")
        self.git("add", AUTH_PATH, OVERLAY_PATH)
        self.git("commit", "-m", "consume authorization")
        return self.git("rev-parse", "HEAD")

    def test_valid_lineage_remains_blocked_before_any_mutation(self) -> None:
        before = self.git("status", "--porcelain")
        result = self.initial()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("desired_state_authority_cutover is immutable false", result.stderr)
        self.assertEqual(self.git("status", "--porcelain"), before)

        result = self.run_gate("reviewer-context", "--expected-sha", self.authorized_sha)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "team_id": 424242,
                "evidence_author_login": "production-evidence-author",
                "independent_reviewer_login": "production-independent-reviewer",
            },
        )

    def test_current_false_record_blocks_production(self) -> None:
        self.git("reset", "--hard", self.evidence_sha)
        self.force_remote(self.evidence_sha)
        result = self.initial(self.evidence_sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("deployment_authorized", result.stderr)

    def test_cutover_authority_cannot_be_enabled_or_omitted_in_schema_v2(self) -> None:
        for mutate in (
            lambda record: record.update({"desired_state_authority_cutover": True}),
            lambda record: record.pop("desired_state_authority_cutover"),
        ):
            with self.subTest(case=mutate):
                self.git("reset", "--hard", self.evidence_sha)
                self.write_authorization(True, mutate)
                self.git("add", AUTH_PATH)
                self.git("commit", "-m", "attempt cutover bypass")
                sha = self.git("rev-parse", "HEAD")
                self.force_remote(sha)
                result = self.initial(sha)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("production authorization", result.stderr)

    def test_rejects_evidence_unknown_missing_wrong_type_and_placeholder(self) -> None:
        cases = [
            lambda e: e.update({"future": False}),
            lambda e: e.pop("target"),
            lambda e: e.update({"schema_version": True}),
            lambda e: e["observed_database_topology"].update({"instances": "three"}),
            lambda e: e["capacity_headroom"].update({"connection_peak": True}),
            lambda e: e["backup_restore_proof"].update({"backup_id": "TBD"}),
            lambda e: e["backup_restore_proof"].update({"backup_id": "TEMPLATE_NOT_EVIDENCE"}),
            lambda e: e["independent_reviewer"].update({"team_id": 0}),
            lambda e: e["independent_reviewer"].update(
                {"github_login": e["evidence_author"]["github_login"]}
            ),
            lambda e: e.update({"reviewed_at": "2026-07-19"}),
            lambda e: e["charter"].update({"charter_id": "another-valid-charter"}),
            lambda e: e["charter"].update({"trust_domain_id": "another-valid-trust-domain"}),
            lambda e: e.update({"observed_running_revision": "1" * 40}),
        ]
        for mutate in cases:
            with self.subTest(case=mutate):
                sha = self.commit_bad_evidence(mutate)
                self.assertNotEqual(self.initial(sha).returncode, 0)

    def test_rejects_noncanonical_evidence_json_bytes(self) -> None:
        self.git("reset", "--hard", self.candidate_sha)
        self.write(EVIDENCE_PATH, json.dumps(self.evidence()) + "\n")
        self.write_authorization(False)
        self.git("add", EVIDENCE_PATH, AUTH_PATH)
        self.git("commit", "-m", "noncanonical evidence preparation")
        self.write_authorization(True)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "authorize noncanonical evidence")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        result = self.initial(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("canonical two-space JSON", result.stderr)

    def test_rejects_hash_mismatch_and_noncanonical_evidence_path(self) -> None:
        for mutate in (
            lambda r: r["production_cardinality_evidence"].update({"sha256": "0" * 64}),
            lambda r: r["production_cardinality_evidence"].update({"path": "docs/release/bytes.txt"}),
        ):
            with self.subTest(case=mutate):
                self.git("reset", "--hard", self.evidence_sha)
                self.write_authorization(True, mutate)
                self.git("add", AUTH_PATH)
                self.git("commit", "-m", "bad evidence reference")
                sha = self.git("rev-parse", "HEAD")
                self.force_remote(sha)
                self.assertNotEqual(self.initial(sha).returncode, 0)

    def test_rejects_extra_engineering_gate_key(self) -> None:
        self.git("reset", "--hard", self.authorized_sha)
        gate_path = self.repo / "docs/release/PR-473-EXPAND-CONTRACT.gate.json"
        gate = json.loads(gate_path.read_text())
        gate["future"] = False
        gate_path.write_text(json.dumps(gate), encoding="utf-8")
        self.git("add", str(gate_path.relative_to(self.repo)))
        self.git("commit", "-m", "unknown engineering key")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        result = self.initial(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("engineering expand gate keys are not exact", result.stderr)

    def test_rejects_evidence_preparation_with_unrelated_change(self) -> None:
        self.git("reset", "--hard", self.candidate_sha)
        self.write_evidence()
        self.write_authorization(False)
        self.write("unrelated", "changed\n")
        self.git("add", EVIDENCE_PATH, AUTH_PATH, "unrelated")
        self.git("commit", "-m", "mixed evidence preparation")
        self.write_authorization(True)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "authorize")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        result = self.initial(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("evidence-preparation commit must change exactly", result.stderr)

    def test_rejects_non_one_shot_authorization_commit(self) -> None:
        self.git("reset", "--hard", self.evidence_sha)
        self.write_authorization(True)
        self.write("unrelated", "changed\n")
        self.git("add", AUTH_PATH, "unrelated")
        self.git("commit", "-m", "mixed authorization")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        result = self.initial(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("only the production authorization", result.stderr)

    def test_authorized_commit_must_have_exactly_one_parent(self) -> None:
        self.git("reset", "--hard", self.evidence_sha)
        self.git("checkout", "-b", "side")
        self.git("commit", "--allow-empty", "-m", "side")
        self.git("checkout", "dev")
        self.git("reset", "--hard", self.evidence_sha)
        self.write_authorization(True)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "authorize")
        self.git("merge", "--no-ff", "side", "-m", "merge authorization")
        sha = self.git("rev-parse", "HEAD")
        self.force_remote(sha)
        result = self.initial(sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("authorized commit must have exactly one parent", result.stderr)

    def test_reset_and_pre_push_accept_optional_overlay_and_canonical_false(self) -> None:
        for change_overlay in (False, True):
            with self.subTest(change_overlay=change_overlay):
                self.prepare_promotion_commit(overlay=change_overlay)
                result = self.run_gate("pre-push", "--expected-sha", self.authorized_sha)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_pre_push_dirty_tree_failure_is_isolated(self) -> None:
        self.prepare_promotion_commit()
        self.write(OVERLAY_PATH, "dirty\n")
        result = self.run_gate("pre-push", "--expected-sha", self.authorized_sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("tracked working-tree changes", result.stderr)

    def test_pre_push_wrong_parent_failure_is_isolated(self) -> None:
        self.git("reset", "--hard", self.authorized_sha)
        self.force_remote(self.authorized_sha)
        self.git("commit", "--allow-empty", "-m", "intermediate")
        self.write_authorization(False)
        self.git("add", AUTH_PATH)
        self.git("commit", "-m", "consume from wrong parent")
        result = self.run_gate("pre-push", "--expected-sha", self.authorized_sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("parent is not the exact authorized SHA", result.stderr)

    def test_pre_push_remote_race_failure_is_isolated(self) -> None:
        self.prepare_promotion_commit()
        self.force_remote(self.evidence_sha)
        result = self.run_gate("pre-push", "--expected-sha", self.authorized_sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("origin/dev advanced after authorization", result.stderr)

    def test_promotion_commit_must_have_exactly_one_parent(self) -> None:
        promotion_sha = self.prepare_promotion_commit()
        self.git("branch", "promotion-side", self.authorized_sha)
        self.git("checkout", "promotion-side")
        self.git("commit", "--allow-empty", "-m", "promotion side")
        self.git("checkout", "dev")
        self.git("reset", "--hard", promotion_sha)
        self.git("merge", "--no-ff", "promotion-side", "-m", "merge promotion")
        result = self.run_gate("pre-push", "--expected-sha", self.authorized_sha)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("promotion commit must have exactly one parent", result.stderr)

    def test_remote_mode_requires_exact_revision(self) -> None:
        self.assertEqual(
            self.run_gate("remote", "--expected-sha", self.authorized_sha).returncode,
            0,
        )
        self.assertNotEqual(
            self.run_gate("remote", "--expected-sha", "b" * 40).returncode,
            0,
        )


if __name__ == "__main__":
    unittest.main()
