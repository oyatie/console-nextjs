#!/usr/bin/env python3
"""Fail-closed, immutable authorization checks for production promotion."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from datetime import datetime
from pathlib import Path, PurePosixPath


ENGINEERING_GATE_PATH = "docs/release/PR-473-EXPAND-CONTRACT.gate.json"
AUTHORIZATION_PATH = "docs/release/PR-473-PRODUCTION-PROMOTION.authorization.json"
CANONICAL_EVIDENCE_PATH = "docs/release/PR-473-PRODUCTION-CARDINALITY.evidence.json"
PROD_OVERLAY_PATH = "deploy/apps/console/overlays/prod/kustomization.yaml"
MAIN_REF = "refs/heads/dev"
ROLLBACK_FLOOR = "f6ff236b9770c79301a3d07da6afb56be1e27bbf"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
LOGIN_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")
RFC3339_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$"
)
PLACEHOLDER_RE = re.compile(
    r"(?:^|[^a-z0-9])(tbd|todo|unknown|placeholder|template|replace[-_ ]?me|example)(?:$|[^a-z0-9])",
    re.IGNORECASE,
)
EXPECTED_CHARTER_ID = "oyatie-production-change-authority-v1"
EXPECTED_TRUST_DOMAIN_ID = "oyatie-production-independent-review"

ENGINEERING_GATE_KEYS = (
    "schema_version",
    "pull_request",
    "rollback_floor",
    "release_phase",
    "deployment_authorized",
    "command_only_claim_authorized",
    "production_authority",
    "guarded_tests",
)
AUTHORIZATION_KEYS = (
    "schema_version",
    "pull_request",
    "target",
    "release_phase",
    "rollback_floor",
    "desired_state_authority_cutover",
    "deployment_authorized",
    "command_only",
    "production_cardinality_evidence",
    "contract_authorities",
)
AUTHORIZATION_EVIDENCE_KEYS = ("path", "sha256", "verified")
CONTRACT_AUTHORITY_KEYS = ("old_runtime_drain", "rollback_floor_raise")
EVIDENCE_KEYS = (
    "schema_version",
    "target",
    "release_phase",
    "candidate_source_sha",
    "observed_running_revision",
    "observed_database_topology",
    "capacity_headroom",
    "backup_restore_proof",
    "evidence_author",
    "independent_reviewer",
    "charter",
    "observed_at",
    "prepared_at",
    "reviewed_at",
)
TOPOLOGY_KEYS = (
    "cluster_name",
    "namespace",
    "writer_endpoint",
    "reader_endpoint",
    "instances",
)
INSTANCE_KEYS = ("name", "role", "ready", "zone")
CAPACITY_KEYS = (
    "window_started_at",
    "window_ended_at",
    "cpu_peak_percent",
    "memory_peak_percent",
    "storage_used_percent",
    "connection_peak",
    "connection_limit",
    "minimum_headroom_percent",
)
BACKUP_KEYS = (
    "backup_id",
    "backup_completed_at",
    "isolated_restore_id",
    "isolated_restore_completed_at",
    "restored_revision",
    "validation_checks",
)
AUTHOR_KEYS = ("github_login", "identity_provider_subject")
REVIEWER_KEYS = ("github_login", "identity_provider_subject", "team_id")
CHARTER_KEYS = ("charter_id", "trust_domain_id")


class AuthorityError(RuntimeError):
    pass


def git(*args: str) -> str:
    completed = subprocess.run(
        ["git", *args], check=False, capture_output=True, text=True
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise AuthorityError(f"git {' '.join(args)} failed: {detail}")
    return completed.stdout.strip()


def require_sha(value: object, label: str = "expected SHA") -> str:
    if type(value) is not str or not SHA_RE.fullmatch(value) or value == "0" * 40:
        raise AuthorityError(f"{label} must be a non-zero full lowercase 40-character commit SHA")
    return value


def require_safe_repo_path(value: object, label: str) -> str:
    if type(value) is not str or not value:
        raise AuthorityError(f"{label} must be a non-empty repository-relative path")
    if "\\" in value or not re.fullmatch(r"[A-Za-z0-9._/-]+", value):
        raise AuthorityError(f"{label} contains unsupported path characters")
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in ("", ".", "..") for part in path.parts):
        raise AuthorityError(f"{label} must not be absolute or contain traversal")
    if str(path) != value:
        raise AuthorityError(f"{label} must use canonical POSIX repository syntax")
    return value


def git_bytes_at_sha(expected_sha: str, path: str) -> bytes:
    require_safe_repo_path(path, "immutable evidence path")
    completed = subprocess.run(
        ["git", "show", f"{expected_sha}:{path}"], check=False, capture_output=True
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode(errors="replace").strip()
        raise AuthorityError(
            f"immutable file is unavailable at {expected_sha}:{path}: {detail}"
        )
    return completed.stdout


def json_at_sha(expected_sha: str, path: str, label: str) -> dict[str, object]:
    try:
        value = json.loads(git_bytes_at_sha(expected_sha, path))
    except json.JSONDecodeError as exc:
        raise AuthorityError(
            f"{label} is malformed JSON at {expected_sha}:{path}: {exc}"
        ) from exc
    if not isinstance(value, dict):
        raise AuthorityError(f"{label} must be a JSON object")
    return value


def require_canonical_json_bytes(raw: bytes, value: dict[str, object], label: str) -> None:
    expected = (json.dumps(value, indent=2) + "\n").encode()
    if raw != expected:
        raise AuthorityError(f"{label} must use canonical two-space JSON with one trailing newline")


def require_exact_keys(
    value: dict[str, object], expected: tuple[str, ...], label: str
) -> None:
    actual = set(value)
    expected_set = set(expected)
    unknown = sorted(actual - expected_set)
    missing = sorted(expected_set - actual)
    if unknown or missing:
        details = []
        if unknown:
            details.append("unknown=" + ",".join(unknown))
        if missing:
            details.append("missing=" + ",".join(missing))
        raise AuthorityError(f"{label} keys are not exact ({'; '.join(details)})")


def require_object(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise AuthorityError(f"{label} must be an object")
    return value


def require_string(value: object, label: str) -> str:
    if type(value) is not str or not value.strip() or PLACEHOLDER_RE.search(value):
        raise AuthorityError(f"{label} must be a non-placeholder string")
    return value


def require_rfc3339(value: object, label: str) -> datetime:
    text = require_string(value, label)
    if not RFC3339_RE.fullmatch(text):
        raise AuthorityError(f"{label} must be an RFC3339 timestamp with timezone")
    try:
        parsed = datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError as exc:
        raise AuthorityError(f"{label} must be a valid RFC3339 timestamp") from exc
    if parsed.tzinfo is None:
        raise AuthorityError(f"{label} must include a timezone")
    return parsed


def require_percent(value: object, label: str) -> float:
    if type(value) not in (int, float) or not 0 <= value <= 100:
        raise AuthorityError(f"{label} must be a number from 0 through 100")
    return float(value)


def commit_parent(commit: str, label: str) -> str:
    parents = git("rev-list", "--parents", "-n", "1", commit).split()
    if len(parents) != 2:
        raise AuthorityError(f"{label} must have exactly one parent")
    return require_sha(parents[1], f"{label} parent")


def changed_paths(parent: str, commit: str) -> list[str]:
    return git(
        "diff-tree", "--no-commit-id", "--name-only", "-r", parent, commit
    ).splitlines()


def verify_engineering_gate(expected_sha: str) -> None:
    gate = json_at_sha(expected_sha, ENGINEERING_GATE_PATH, "engineering expand gate")
    require_exact_keys(gate, ENGINEERING_GATE_KEYS, "engineering expand gate")
    checks = (
        (
            type(gate["schema_version"]) is int and gate["schema_version"] == 1,
            "schema_version must equal integer 1",
        ),
        (
            type(gate["pull_request"]) is int and gate["pull_request"] == 473,
            "pull_request must equal integer 473",
        ),
        (
            gate["rollback_floor"] == ROLLBACK_FLOOR,
            "rollback_floor is not the immutable PR 471 merge",
        ),
        (gate["release_phase"] == "expand", "release_phase must equal expand"),
        (gate["deployment_authorized"] is False, "deployment_authorized must remain false"),
        (
            gate["command_only_claim_authorized"] is False,
            "command_only_claim_authorized must remain false",
        ),
        (isinstance(gate["guarded_tests"], list), "guarded_tests must be an array"),
    )
    for passed, message in checks:
        if not passed:
            raise AuthorityError(f"engineering expand gate {message}")
    expected_authority = {
        "production_cardinality": False,
        "old_runtime_drain": False,
        "rollback_floor_raise": False,
    }
    if gate["production_authority"] != expected_authority:
        raise AuthorityError(
            "engineering expand gate production_authority must remain exactly all false"
        )


def verify_authorization_schema(record: dict[str, object], *, authorized: bool) -> None:
    require_exact_keys(record, AUTHORIZATION_KEYS, "production authorization")
    expected_scalars = {
        "schema_version": 2,
        "pull_request": 473,
        "target": "production",
        "release_phase": "expand",
        "rollback_floor": ROLLBACK_FLOOR,
        "desired_state_authority_cutover": False,
        "deployment_authorized": authorized,
        "command_only": False,
    }
    for key, expected in expected_scalars.items():
        actual = record[key]
        if type(actual) is not type(expected) or actual != expected:
            raise AuthorityError(f"production authorization {key} must equal {expected!r}")

    evidence = require_object(
        record["production_cardinality_evidence"], "production_cardinality_evidence"
    )
    require_exact_keys(
        evidence, AUTHORIZATION_EVIDENCE_KEYS, "production_cardinality_evidence"
    )
    if evidence["path"] != CANONICAL_EVIDENCE_PATH:
        raise AuthorityError(
            f"production_cardinality_evidence.path must equal {CANONICAL_EVIDENCE_PATH}"
        )
    require_safe_repo_path(evidence["path"], "production_cardinality_evidence.path")
    if type(evidence["sha256"]) is not str or not SHA256_RE.fullmatch(
        evidence["sha256"]
    ):
        raise AuthorityError(
            "production_cardinality_evidence.sha256 must be lowercase sha256"
        )
    if evidence["verified"] is not authorized:
        raise AuthorityError(
            f"production_cardinality_evidence.verified must be {authorized!r}"
        )

    authorities = require_object(record["contract_authorities"], "contract_authorities")
    require_exact_keys(authorities, CONTRACT_AUTHORITY_KEYS, "contract_authorities")
    if authorities != {"old_runtime_drain": False, "rollback_floor_raise": False}:
        raise AuthorityError(
            "expand authorization must not authorize drain or rollback-floor raise"
        )


def verify_identity(value: object, keys: tuple[str, ...], label: str) -> dict[str, object]:
    identity = require_object(value, label)
    require_exact_keys(identity, keys, label)
    login = require_string(identity["github_login"], f"{label}.github_login")
    if not LOGIN_RE.fullmatch(login):
        raise AuthorityError(f"{label}.github_login must be a valid GitHub login")
    require_string(identity["identity_provider_subject"], f"{label}.identity_provider_subject")
    return identity


def verify_evidence_schema(evidence: dict[str, object], candidate_sha: str) -> None:
    require_exact_keys(evidence, EVIDENCE_KEYS, "production cardinality evidence")
    scalars = {
        "schema_version": 1,
        "target": "production",
        "release_phase": "expand",
    }
    for key, expected in scalars.items():
        if type(evidence[key]) is not type(expected) or evidence[key] != expected:
            raise AuthorityError(f"production cardinality evidence {key} must equal {expected!r}")
    if require_sha(evidence["candidate_source_sha"], "evidence candidate_source_sha") != candidate_sha:
        raise AuthorityError("evidence candidate_source_sha must equal the evidence-preparation parent")
    observed_running_revision = require_sha(
        evidence["observed_running_revision"], "evidence observed_running_revision"
    )
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", observed_running_revision, candidate_sha],
        check=False,
        capture_output=True,
    )
    if ancestor.returncode != 0:
        raise AuthorityError(
            "evidence observed_running_revision must be a commit ancestor of candidate_source_sha"
        )

    topology = require_object(evidence["observed_database_topology"], "observed_database_topology")
    require_exact_keys(topology, TOPOLOGY_KEYS, "observed_database_topology")
    for key in ("cluster_name", "namespace", "writer_endpoint", "reader_endpoint"):
        require_string(topology[key], f"observed_database_topology.{key}")
    instances = topology["instances"]
    if not isinstance(instances, list) or not instances:
        raise AuthorityError("observed_database_topology.instances must be a non-empty array")
    instance_names: set[str] = set()
    primary_count = 0
    for index, raw_instance in enumerate(instances):
        label = f"observed_database_topology.instances[{index}]"
        instance = require_object(raw_instance, label)
        require_exact_keys(instance, INSTANCE_KEYS, label)
        name = require_string(instance["name"], f"{label}.name")
        if name in instance_names:
            raise AuthorityError("observed database instance names must be unique")
        instance_names.add(name)
        if instance["role"] not in ("primary", "replica"):
            raise AuthorityError(f"{label}.role must equal primary or replica")
        primary_count += int(instance["role"] == "primary")
        if instance["ready"] is not True:
            raise AuthorityError(f"{label}.ready must be true")
        require_string(instance["zone"], f"{label}.zone")
    if primary_count != 1:
        raise AuthorityError("observed database topology must contain exactly one primary")

    capacity = require_object(evidence["capacity_headroom"], "capacity_headroom")
    require_exact_keys(capacity, CAPACITY_KEYS, "capacity_headroom")
    window_start = require_rfc3339(capacity["window_started_at"], "capacity_headroom.window_started_at")
    window_end = require_rfc3339(capacity["window_ended_at"], "capacity_headroom.window_ended_at")
    if window_end <= window_start:
        raise AuthorityError("capacity_headroom window must end after it starts")
    peaks = [
        require_percent(capacity[key], f"capacity_headroom.{key}")
        for key in ("cpu_peak_percent", "memory_peak_percent", "storage_used_percent")
    ]
    minimum_headroom = require_percent(
        capacity["minimum_headroom_percent"], "capacity_headroom.minimum_headroom_percent"
    )
    if minimum_headroom <= 0 or any(100 - peak < minimum_headroom for peak in peaks):
        raise AuthorityError("capacity peaks do not preserve the declared minimum headroom")
    connection_peak = capacity["connection_peak"]
    connection_limit = capacity["connection_limit"]
    if (
        type(connection_peak) is not int
        or type(connection_limit) is not int
        or connection_peak < 0
        or connection_limit <= 0
        or connection_peak >= connection_limit
    ):
        raise AuthorityError("capacity connection_peak must be a non-negative integer below connection_limit")
    if 100 * (connection_limit - connection_peak) / connection_limit < minimum_headroom:
        raise AuthorityError("connection capacity does not preserve the declared minimum headroom")

    backup = require_object(evidence["backup_restore_proof"], "backup_restore_proof")
    require_exact_keys(backup, BACKUP_KEYS, "backup_restore_proof")
    for key in ("backup_id", "isolated_restore_id"):
        require_string(backup[key], f"backup_restore_proof.{key}")
    backup_at = require_rfc3339(backup["backup_completed_at"], "backup_restore_proof.backup_completed_at")
    restore_at = require_rfc3339(
        backup["isolated_restore_completed_at"],
        "backup_restore_proof.isolated_restore_completed_at",
    )
    if restore_at < backup_at:
        raise AuthorityError("isolated restore cannot complete before its backup")
    restored_revision = require_sha(
        backup["restored_revision"], "backup_restore_proof.restored_revision"
    )
    if restored_revision != observed_running_revision:
        raise AuthorityError(
            "backup_restore_proof.restored_revision must equal observed_running_revision"
        )
    checks = backup["validation_checks"]
    if not isinstance(checks, list) or not checks:
        raise AuthorityError("backup_restore_proof.validation_checks must be a non-empty array")
    for index, check in enumerate(checks):
        require_string(check, f"backup_restore_proof.validation_checks[{index}]")

    author = verify_identity(evidence["evidence_author"], AUTHOR_KEYS, "evidence_author")
    reviewer = verify_identity(
        evidence["independent_reviewer"], REVIEWER_KEYS, "independent_reviewer"
    )
    if type(reviewer["team_id"]) is not int or reviewer["team_id"] <= 0:
        raise AuthorityError("independent_reviewer.team_id must be a positive integer")
    if author["github_login"].casefold() == reviewer["github_login"].casefold():
        raise AuthorityError("evidence author and independent reviewer must be distinct")
    if author["identity_provider_subject"] == reviewer["identity_provider_subject"]:
        raise AuthorityError("evidence author and independent reviewer subjects must be distinct")

    charter = require_object(evidence["charter"], "charter")
    require_exact_keys(charter, CHARTER_KEYS, "charter")
    if charter["charter_id"] != EXPECTED_CHARTER_ID:
        raise AuthorityError(f"charter.charter_id must equal {EXPECTED_CHARTER_ID}")
    if charter["trust_domain_id"] != EXPECTED_TRUST_DOMAIN_ID:
        raise AuthorityError(
            f"charter.trust_domain_id must equal {EXPECTED_TRUST_DOMAIN_ID}"
        )

    observed_at = require_rfc3339(evidence["observed_at"], "observed_at")
    prepared_at = require_rfc3339(evidence["prepared_at"], "prepared_at")
    reviewed_at = require_rfc3339(evidence["reviewed_at"], "reviewed_at")
    if not (window_end <= observed_at <= prepared_at <= reviewed_at):
        raise AuthorityError("evidence timestamps must follow window_end <= observed <= prepared <= reviewed")


def canonical_false(record: dict[str, object]) -> dict[str, object]:
    result = json.loads(json.dumps(record))
    result["deployment_authorized"] = False
    evidence = require_object(
        result["production_cardinality_evidence"], "production_cardinality_evidence"
    )
    evidence["verified"] = False
    return result


def authorization_with_evidence_hash(
    record: dict[str, object], digest: str
) -> dict[str, object]:
    result = canonical_false(record)
    evidence = require_object(
        result["production_cardinality_evidence"], "production_cardinality_evidence"
    )
    evidence["sha256"] = digest
    return result


def verify_authorized_commit(expected_sha: str) -> dict[str, object]:
    verify_engineering_gate(expected_sha)
    authorization = json_at_sha(expected_sha, AUTHORIZATION_PATH, "production authorization")
    verify_authorization_schema(authorization, authorized=True)
    authorized_parent = commit_parent(expected_sha, "authorized commit")
    parent_authorization = json_at_sha(
        authorized_parent, AUTHORIZATION_PATH, "parent production authorization"
    )
    verify_authorization_schema(parent_authorization, authorized=False)
    if parent_authorization != canonical_false(authorization):
        raise AuthorityError("authorized commit is not an exact one-shot false-to-true transition")
    if changed_paths(authorized_parent, expected_sha) != [AUTHORIZATION_PATH]:
        raise AuthorityError(
            "authorization commit may change only the production authorization record"
        )

    evidence_ref = require_object(
        authorization["production_cardinality_evidence"],
        "production_cardinality_evidence",
    )
    evidence_path = str(evidence_ref["path"])
    evidence_bytes = git_bytes_at_sha(authorized_parent, evidence_path)
    digest = hashlib.sha256(evidence_bytes).hexdigest()
    if digest != evidence_ref["sha256"]:
        raise AuthorityError(
            "production cardinality evidence sha256 does not match immutable content"
        )
    evidence = json_at_sha(
        authorized_parent, evidence_path, "production cardinality evidence"
    )
    require_canonical_json_bytes(
        evidence_bytes, evidence, "production cardinality evidence"
    )

    candidate_sha = commit_parent(authorized_parent, "evidence-preparation commit")
    if set(changed_paths(candidate_sha, authorized_parent)) != {
        evidence_path,
        AUTHORIZATION_PATH,
    }:
        raise AuthorityError(
            "evidence-preparation commit must change exactly the evidence JSON and still-false authorization hash"
        )
    verify_evidence_schema(evidence, candidate_sha)

    candidate_authorization = json_at_sha(
        candidate_sha, AUTHORIZATION_PATH, "candidate production authorization"
    )
    verify_authorization_schema(candidate_authorization, authorized=False)
    candidate_evidence_digest = hashlib.sha256(
        git_bytes_at_sha(candidate_sha, evidence_path)
    ).hexdigest()
    if candidate_authorization != authorization_with_evidence_hash(
        parent_authorization, candidate_evidence_digest
    ):
        raise AuthorityError(
            "evidence-preparation commit may change only the false authorization evidence hash"
        )
    return authorization


def fetch_origin_dev() -> str:
    git("fetch", "--no-tags", "origin", "+refs/heads/dev:refs/remotes/origin/dev")
    return require_sha(git("rev-parse", "refs/remotes/origin/dev"), "origin/dev SHA")


def require_clean_tracked_worktree() -> None:
    if git("status", "--porcelain", "--untracked-files=no"):
        raise AuthorityError("production checkout has tracked working-tree changes")


def verify_initial(expected_sha: str, expected_ref: str, require_local_branch: bool) -> None:
    if expected_ref != MAIN_REF:
        raise AuthorityError(f"production promotion requires {MAIN_REF}, got {expected_ref!r}")
    if git("rev-parse", "HEAD") != expected_sha:
        raise AuthorityError("checked-out HEAD does not equal the authorized promotion SHA")
    require_clean_tracked_worktree()
    if require_local_branch and git("symbolic-ref", "--quiet", "--short", "HEAD") != "dev":
        raise AuthorityError("manual production promotion must run from local branch dev")
    if fetch_origin_dev() != expected_sha:
        raise AuthorityError("origin/dev does not equal the authorized promotion SHA")
    authorization = verify_authorized_commit(expected_sha)
    if authorization["desired_state_authority_cutover"] is not True:
        raise AuthorityError(
            "production activation is blocked: desired_state_authority_cutover is immutable false in schema v2; "
            "activation requires a separate accepted higher-authority ADR/cutover and an explicit schema/code change"
        )


def reset_authorization(expected_sha: str) -> None:
    authorization = verify_authorized_commit(expected_sha)
    Path(AUTHORIZATION_PATH).write_text(
        json.dumps(canonical_false(authorization), indent=2) + "\n", encoding="utf-8"
    )


def verify_pre_push(expected_sha: str) -> None:
    if fetch_origin_dev() != expected_sha:
        raise AuthorityError("origin/dev advanced after authorization; refusing production push")
    require_clean_tracked_worktree()
    new_sha = require_sha(git("rev-parse", "HEAD"), "promotion commit SHA")
    parent = commit_parent(new_sha, "promotion commit")
    if parent != expected_sha:
        raise AuthorityError("promotion commit parent is not the exact authorized SHA")
    changed = set(changed_paths(expected_sha, new_sha))
    allowed = {AUTHORIZATION_PATH, PROD_OVERLAY_PATH}
    if AUTHORIZATION_PATH not in changed or not changed.issubset(allowed):
        raise AuthorityError(
            "promotion commit may change only the prod overlay and must reset authorization"
        )
    reset = json_at_sha(new_sha, AUTHORIZATION_PATH, "reset production authorization")
    verify_authorization_schema(reset, authorized=False)
    authorized = json_at_sha(expected_sha, AUTHORIZATION_PATH, "authorized production authorization")
    if reset != canonical_false(authorized):
        raise AuthorityError(
            "promotion commit did not reset authorization to canonical false"
        )


def verify_remote(expected_sha: str) -> None:
    if fetch_origin_dev() != expected_sha:
        raise AuthorityError("origin/dev is not the exact expected production revision")


def reviewer_context(expected_sha: str) -> dict[str, object]:
    authorization = verify_authorized_commit(expected_sha)
    evidence_ref = require_object(
        authorization["production_cardinality_evidence"],
        "production_cardinality_evidence",
    )
    evidence = json_at_sha(
        expected_sha, str(evidence_ref["path"]), "production cardinality evidence"
    )
    author = require_object(evidence["evidence_author"], "evidence_author")
    reviewer = require_object(evidence["independent_reviewer"], "independent_reviewer")
    return {
        "team_id": reviewer["team_id"],
        "evidence_author_login": author["github_login"],
        "independent_reviewer_login": reviewer["github_login"],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    modes = parser.add_subparsers(dest="mode", required=True)
    initial = modes.add_parser("initial")
    initial.add_argument("--expected-sha", required=True)
    initial.add_argument("--expected-ref", required=True)
    initial.add_argument("--require-local-branch", action="store_true")
    for mode in ("reset", "pre-push", "remote", "reviewer-context"):
        command = modes.add_parser(mode)
        command.add_argument("--expected-sha", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        expected_sha = require_sha(args.expected_sha)
        if args.mode == "initial":
            verify_initial(expected_sha, args.expected_ref, args.require_local_branch)
        elif args.mode == "reset":
            reset_authorization(expected_sha)
        elif args.mode == "pre-push":
            verify_pre_push(expected_sha)
        elif args.mode == "reviewer-context":
            print(json.dumps(reviewer_context(expected_sha), separators=(",", ":")))
        else:
            verify_remote(expected_sha)
    except AuthorityError as exc:
        print(f"production-promotion-authority: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
