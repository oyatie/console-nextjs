#!/usr/bin/env python3
"""Behavior locks for the first-party Rust BUCK graph generator."""

import importlib.util
import inspect
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


GENERATOR_PATH = Path(__file__).with_name("gen_first_party.py")
SPEC = importlib.util.spec_from_file_location("gen_first_party", GENERATOR_PATH)
assert SPEC is not None and SPEC.loader is not None
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)


class FirstPartyBuckGeneratorTests(unittest.TestCase):
    def test_ui_package_members_are_skipped_because_leptos_is_not_vendored(self) -> None:
        self.assertTrue(
            GENERATOR.skip_workspace_member({"package": {"name": "console-payroll-ui"}})
        )
        self.assertTrue(
            GENERATOR.skip_workspace_member({"package": {"name": "console-platform-ui"}})
        )
        self.assertFalse(
            GENERATOR.skip_workspace_member({"package": {"name": "console-payroll-rest"}})
        )
        self.assertFalse(
            GENERATOR.skip_workspace_member({"package": {"name": "console-platform-auth"}})
        )
        self.assertFalse(GENERATOR.skip_workspace_member({"package": {"name": "ui"}}))
        self.assertFalse(GENERATOR.skip_workspace_member({}))
        self.assertFalse(GENERATOR.skip_workspace_member({"package": {}}))

    def test_skipped_ui_dependency_is_omitted_not_rewritten_as_third_party(self) -> None:
        first_party = {"console-app": "//backend/app:console-app"}
        skipped = frozenset({"console-payroll-ui"})
        deps, named = GENERATOR.map_deps(
            {"console-payroll-ui": {"path": "crates/payroll/ui"}},
            first_party,
            skipped,
        )
        self.assertEqual([], deps)
        self.assertEqual({}, named)
        self.assertNotIn("//third-party/rust:console-payroll-ui", deps)

    def test_app_without_ui_dep_does_not_gain_a_ui_edge(self) -> None:
        app_dir = Path(GENERATOR.REPO) / "backend" / "app"
        manifest = GENERATOR.load(app_dir)
        first_party = {}
        for directory in GENERATOR.find_members():
            name = GENERATOR.load(directory)["package"]["name"]
            rel = str(Path(directory).relative_to(GENERATOR.REPO))
            first_party[name] = "//{}:{}".format(rel, name)
        deps, named = GENERATOR.map_deps(
            manifest.get("dependencies"),
            first_party,
            GENERATOR.skipped_ui_package_names(),
        )
        self.assertFalse(
            any("-ui" in target for target in deps),
            "App must not invent a Ui edge; got {}".format(deps),
        )
        self.assertFalse(
            any("-ui" in target for target in named.values()),
            "App must not invent a named Ui edge; got {}".format(named),
        )

    def test_repo_source_layout_uses_mapped_sources_and_explicit_crate_root(self) -> None:
        block = "\n".join(
            GENERATOR._block(
                "rust_library",
                "example",
                'glob(["src/**/*.rs"])',
                "example",
                [],
                {},
                {"CARGO_MANIFEST_DIR": "backend/crates/example"},
                package="backend/crates/example",
                crate_root="backend/crates/example/src/lib.rs",
                external={
                    "//docs/specs:cedar-pbac-map": (
                        "docs/specs/cedar-pbac-coexistence-map.json"
                    ),
                },
            )
        )

        self.assertIn(
            'mapped_srcs = repo_mapped_srcs("backend/crates/example", '
            'glob(["src/**/*.rs"]), external = {',
            block,
        )
        self.assertIn(
            '"//docs/specs:cedar-pbac-map": '
            '"docs/specs/cedar-pbac-coexistence-map.json"',
            block,
        )
        self.assertIn(
            'crate_root = "backend/crates/example/src/lib.rs"',
            block,
        )
        self.assertNotIn("\n    srcs =", block)

    def test_generation_is_clean_for_all_first_party_buck_faces(self) -> None:
        subprocess.run(
            [sys.executable, str(GENERATOR_PATH)],
            cwd=GENERATOR.REPO,
            check=True,
            capture_output=True,
            text=True,
        )
        buck_files = [
            str(Path(directory).relative_to(GENERATOR.REPO) / "BUCK")
            for directory in GENERATOR.find_members()
        ]
        result = subprocess.run(
            ["git", "diff", "--quiet", "--", *buck_files],
            cwd=GENERATOR.REPO,
            check=False,
        )
        self.assertEqual(0, result.returncode, "generated BUCK faces are stale")

    def test_compile_time_resource_contracts_are_declared(self) -> None:
        resources = GENERATOR.RESOURCE_CONFIG

        self.assertEqual(
            resources["console-platform-authz"]["external"][
                "//docs/specs:cedar-pbac-map"
            ],
            "docs/specs/cedar-pbac-coexistence-map.json",
        )
        self.assertEqual(
            resources["console-reporting-adapter-postgres"]["external"][
                "//docs/reference:daily-progress"
            ],
            "docs/reference/일일업무진행현황_0605.xlsx",
        )
        self.assertEqual(
            resources["console-app"]["external"]["//backend/openapi:openapi.yaml"],
            "backend/openapi/openapi.yaml",
        )

    def test_sqlx_tests_map_the_authoritative_migration_tree(self) -> None:
        external = GENERATOR.integration_external_resources(
            "console-leave-adapter-postgres",
            "tests/leave_migration_expand_contract.rs",
            '#[sqlx::test(migrations = "../../platform/db/migrations")]',
        )

        self.assertEqual(
            external["//backend/crates/platform/db/migrations:tree"],
            "backend/crates/platform/db/migrations",
        )

    def test_openapi_drift_maps_real_rest_source_trees(self) -> None:
        config = GENERATOR.integration_resource_config(
            "console-app",
            "tests/openapi_drift.rs",
        )

        self.assertIn("src/**/*.rs", config["srcs"])
        self.assertEqual(
            config["external"][
                "//backend/crates/dispatch/rest:crate-source-tree"
            ],
            "backend/crates/dispatch/rest/src",
        )
        self.assertEqual(
            config["external"]["//backend/openapi:openapi.yaml"],
            "backend/openapi/openapi.yaml",
        )
        self.assertEqual(
            config["external"]["//backend/crates/equipment/rest:crate-source-tree"],
            "backend/crates/equipment/rest/src",
        )

    def test_openapi_drift_maps_every_compile_time_resource(self) -> None:
        test_path = Path(GENERATOR.REPO) / "backend/app/tests/openapi_drift.rs"
        source = test_path.read_text(encoding="utf-8")
        include_paths = re.findall(r'include_str!\(\s*"([^"]+)"', source)
        config = GENERATOR.integration_resource_config(
            "console-app",
            "tests/openapi_drift.rs",
        )
        mapped_roots = [
            Path(GENERATOR.REPO) / destination
            for destination in config["external"].values()
        ]
        app_source_root = Path(GENERATOR.REPO) / "backend/app/src"
        app_package_root = Path(GENERATOR.REPO) / "backend/app"

        def itest_src_maps(resource: Path, src_pattern: str) -> bool:
            # Package-relative itest srcs resolve against backend/app/.
            if "*" not in src_pattern:
                return resource == (app_package_root / src_pattern).resolve()
            prefix = src_pattern.split("*", 1)[0].rstrip("/")
            root = (app_package_root / prefix).resolve()
            return resource == root or (root.is_dir() and resource.is_relative_to(root))

        unmapped = []
        for include_path in include_paths:
            resource = (test_path.parent / include_path).resolve()
            self.assertTrue(resource.exists(), f"missing include_str resource: {resource}")
            if resource.is_relative_to(app_source_root):
                continue
            if any(
                resource == mapped_root
                or (mapped_root.is_dir() and resource.is_relative_to(mapped_root))
                for mapped_root in mapped_roots
            ):
                continue
            if any(itest_src_maps(resource, src) for src in config.get("srcs", [])):
                continue
            unmapped.append(str(resource.relative_to(GENERATOR.REPO)))

        self.assertEqual([], unmapped, "openapi_drift has unmapped include_str resources")

    def test_openapi_fragment_globs_detect_tree_and_include_str(self) -> None:
        governance = Path(GENERATOR.REPO) / "backend/crates/governance/rest"
        src = governance / "src"
        pats = GENERATOR.openapi_fragment_globs(str(governance), str(src))
        self.assertEqual(
            ["openapi/**/*.yaml", "openapi/**/*.json"],
            pats,
            "governance rest must map YAML + manifest.json under openapi/",
        )

        # include_str marker alone (no openapi/ dir) still emits YAML so examined-zero
        # cannot silently pass — buck/rustc fail closed when fragments are absent.
        bare = Path(GENERATOR.REPO) / "backend/crates/contracts"
        bare_src = bare / "src"
        # contracts has no openapi/ sibling and no ../openapi include_str in src
        self.assertEqual([], GENERATOR.openapi_fragment_globs(str(bare), str(bare_src)))

    def test_rest_faces_with_openapi_tree_map_fragment_globs_in_buck(self) -> None:
        """Lock: every first-party package with openapi/ must map fragments in BUCK.

        Examined-zero fails: if no openapi trees exist this assertion is wrong;
        if trees exist but BUCK omits the glob, the list is non-empty (RED).
        """
        missing = []
        examined = 0
        for directory in GENERATOR.find_members():
            openapi = Path(directory) / "openapi"
            if not openapi.is_dir():
                continue
            examined += 1
            buck = Path(directory) / "BUCK"
            text = buck.read_text(encoding="utf-8")
            if "openapi/**/*.yaml" not in text:
                missing.append(str(Path(directory).relative_to(GENERATOR.REPO)))
        self.assertGreater(
            examined,
            0,
            "expected at least one first-party crate with an openapi/ tree",
        )
        self.assertEqual(
            [],
            missing,
            "BUCK faces missing openapi/**/*.yaml mapped_srcs (regenerate gen_first_party)",
        )

    def test_openapi_dotfile_srcs_and_lib_srcs_expr_include_hidden(self) -> None:
        """Lock: Buck2 glob omits leading-dot basenames; generator must union them.

        Would have failed on tip 3c803bdc4 where identity BUCK used only
        glob(["...","openapi/**/*.yaml","openapi/**/*.json"]) with no explicit
        .well-known__* listsrcs union (CI Backend include_str missing those files).
        """
        identity = Path(GENERATOR.REPO) / "backend/crates/identity/rest"
        expected = [
            "openapi/paths/.well-known__apple-app-site-association.get.yaml",
            "openapi/paths/.well-known__assetlinks.json.get.yaml",
        ]
        found = GENERATOR.openapi_dotfile_srcs(str(identity))
        self.assertEqual(expected, found)

        # Temp tree: any leading-dot component must appear in emitted srcs expr.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            hidden = root / "openapi" / "paths" / ".hidden.yaml"
            hidden.parent.mkdir(parents=True)
            hidden.write_text("x: 1\n", encoding="utf-8")
            (root / "openapi" / "paths" / "visible.yaml").write_text("y: 2\n", encoding="utf-8")
            dots = GENERATOR.openapi_dotfile_srcs(str(root))
            self.assertEqual(["openapi/paths/.hidden.yaml"], dots)
            expr = GENERATOR.lib_srcs_expr(
                ["src/**/*.rs", "openapi/**/*.yaml"],
                explicit=dots,
            )
            self.assertIn('glob(["src/**/*.rs", "openapi/**/*.yaml"])', expr)
            self.assertIn('"openapi/paths/.hidden.yaml"', expr)
            self.assertIn(" + ", expr)

    def test_identity_rest_buck_maps_well_known_openapi_dotfiles(self) -> None:
        """Examined-zero lock: identity BUCK mapped_srcs must name both .well-known files.

        Plain openapi/**/*.yaml glob is insufficient on Buck2 (dot basenames excluded).
        """
        buck = Path(GENERATOR.REPO) / "backend/crates/identity/rest" / "BUCK"
        text = buck.read_text(encoding="utf-8")
        for path in (
            "openapi/paths/.well-known__apple-app-site-association.get.yaml",
            "openapi/paths/.well-known__assetlinks.json.get.yaml",
        ):
            self.assertIn(
                path,
                text,
                "identity rest BUCK must list {} after gen_first_party regen".format(path),
            )
        self.assertIn(
            " + ",
            text,
            "identity rest BUCK must union listsrcs for openapi dotfiles",
        )

    def test_workbench_integration_test_maps_its_path_module(self) -> None:
        config = GENERATOR.integration_resource_config(
            "console-app",
            "tests/workbench_api.rs",
        )

        self.assertIn("src/workbench.rs", config["srcs"])

    def test_cross_package_path_modules_are_explicit_mapped_inputs(self) -> None:
        expected = {
            ("console-dispatch-worker", "tests/timer_delivery.rs"): {
                "//backend/test_support:dispatch-worker-fixtures":
                    "backend/test_support/dispatch_worker_fixtures.rs",
            },
            ("console-workorder-rest", "tests/mobile_device_registration.rs"): {
                "//backend/test_support:mobile-evidence-fixtures":
                    "backend/test_support/mobile_evidence_fixtures.rs",
            },
            ("console-workorder-rest", "tests/mobile_evidence.rs"): {
                "//backend/test_support:mobile-evidence-fixtures":
                    "backend/test_support/mobile_evidence_fixtures.rs",
            },
            ("console-workorder-rest", "tests/mobile_sync.rs"): {
                "//backend/test_support:mobile-evidence-fixtures":
                    "backend/test_support/mobile_evidence_fixtures.rs",
            },
        }

        for (crate, test_file), external in expected.items():
            config = GENERATOR.integration_resource_config(crate, test_file)
            self.assertEqual(external, config["external"], f"{crate}:{test_file}")

    def test_manifest_env_is_hermetic_and_repo_relative(self) -> None:
        env = GENERATOR.base_env("backend/crates/example", uses_sqlx=True)

        self.assertEqual(env["CARGO_MANIFEST_DIR"], "backend/crates/example")
        self.assertEqual(env["SQLX_OFFLINE"], "true")
        self.assertEqual(
            env["SQLX_OFFLINE_DIR"],
            "$(location //backend:sqlx-offline)",
        )

    def test_production_parser_unit_target_stays_hermetic(self) -> None:
        self.assertFalse(
            GENERATOR.requires_postgres("console-production-rest", "test.unit")
        )

    def test_console_app_inline_postgres_variant_is_feature_gated(self) -> None:
        variant = GENERATOR.INLINE_TEST_VARIANTS["console-app"][0]
        app_dir = Path(GENERATOR.REPO) / "backend" / "app"
        manifest = GENERATOR.load(app_dir)
        app_source = (app_dir / "src").glob("**/*.rs")
        source_text = "\n".join(path.read_text(encoding="utf-8") for path in app_source)

        self.assertEqual("itest-inline-postgres", variant["name"])
        self.assertEqual("test-postgres", variant["feature"])
        self.assertEqual("postgres", variant["resource"])
        self.assertEqual([], manifest["features"]["test-postgres"])
        self.assertNotIn("default", manifest["features"])
        ordinary_tests = re.findall(r"^\s*#\[(?:test|tokio::test)\]", source_text, re.MULTILINE)
        ordinary_gates = re.findall(
            r'^\s*#\[cfg\(not\(feature = "test-postgres"\)\)\]\n\s*#\[(?:test|tokio::test)\]',
            source_text,
            re.MULTILINE,
        )
        sqlx_tests = re.findall(r"^\s*#\[sqlx::test", source_text, re.MULTILINE)
        sqlx_gates = re.findall(
            r'^\s*#\[cfg\(feature = "test-postgres"\)\]\n\s*#\[sqlx::test',
            source_text,
            re.MULTILINE,
        )
        self.assertEqual(164, len(ordinary_tests))
        self.assertEqual(len(ordinary_tests), len(ordinary_gates))
        self.assertEqual(23, len(sqlx_tests))
        self.assertEqual(len(sqlx_tests), len(sqlx_gates))
        self.assertEqual(
            ("dev-auth",),
            GENERATOR.integration_test_features(
                "console-app", "tests/dev_auth_persona_guard_feature.rs"
            ),
        )

    def test_dev_auth_feature_variants_propagate_through_app_and_auth_rest(self) -> None:
        auth_rest_variant = GENERATOR.INLINE_TEST_VARIANTS["console-platform-auth-rest"][0]
        self.assertEqual("itest-dev-auth-postgres", auth_rest_variant["name"])
        self.assertEqual("dev-auth", auth_rest_variant["feature"])
        self.assertEqual("postgres", auth_rest_variant["resource"])
        self.assertEqual(
            ":console-app-lib-dev-auth",
            GENERATOR.integration_test_library_target(
                "console-app", "tests/dev_auth_persona_guard_feature.rs", ":console-app-lib"
            ),
        )
        self.assertEqual(
            ":console-platform-auth-rest-dev-auth",
            GENERATOR.integration_test_library_target(
                "console-platform-auth-rest", "tests/dev_auth_session.rs", ":console-platform-auth-rest"
            ),
        )
        self.assertEqual(
            ("dev-auth",),
            GENERATOR.integration_test_features(
                "console-platform-auth-rest", "tests/group_admin_tenant_context.rs"
            ),
        )
        self.assertEqual(
            ":console-platform-auth-rest-dev-auth",
            GENERATOR.integration_test_library_target(
                "console-platform-auth-rest",
                "tests/group_admin_tenant_context.rs",
                ":console-platform-auth-rest",
            ),
        )

    def test_inline_test_variants_reject_missing_manifest_features(self) -> None:
        with self.assertRaisesRegex(ValueError, "feature is absent"):
            GENERATOR.validate_inline_test_variants(
                {"console-app": {"features": {}}}
            )


class TestTaxonomy(unittest.TestCase):
    def test_every_test_has_exactly_one_type_and_resource_label(self) -> None:
        for package in (
            "backend/app",
            "backend/ci/contract-tests",
            "backend/crates/logistics/domain",
            "backend/crates/platform/authz-rest",
        ):
            for test_type in GENERATOR.TEST_TYPE_LABELS:
                for uses_postgres in (False, True):
                    labels = GENERATOR.test_labels(package, test_type, uses_postgres)
                    self.assertEqual(
                        1,
                        len(set(labels) & GENERATOR.TEST_TYPE_LABELS),
                    )
                    self.assertEqual(
                        1,
                        len(set(labels) & GENERATOR.RESOURCE_LABELS),
                    )
                    self.assertEqual("needs-postgres" in labels, uses_postgres)

    def test_ownership_labels_are_path_derived_and_deterministic(self) -> None:
        package = "backend/crates/logistics/adapter-postgres"
        expected = [
            "owner.backend.crates.logistics.adapter-postgres",
            "domain.logistics",
        ]
        self.assertEqual(expected, GENERATOR.ownership_labels(package))
        self.assertEqual(expected, GENERATOR.ownership_labels(package))
        self.assertEqual(
            ["owner.backend.app", "domain.app"],
            GENERATOR.ownership_labels("backend/app"),
        )

    def test_unknown_test_type_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            GENERATOR.test_labels("backend/app", "test.e2e", False)

class TestResourceClassification(unittest.TestCase):
    def test_benefit_and_facilities_units_are_hermetic_even_when_sources_mention_postgres(self) -> None:
        for package in ("console-benefit-rest", "console-facilities-rest"):
            self.assertFalse(GENERATOR.requires_postgres(package, "test.unit"))
            labels = GENERATOR.test_labels(
                "backend/crates/{}/rest".format(package.removeprefix("console-").removesuffix("-rest")),
                "test.unit",
                GENERATOR.requires_postgres(package, "test.unit"),
            )
            self.assertIn("resource.none", labels)
            self.assertNotIn("resource.postgres", labels)

    def test_comments_and_unrelated_library_code_cannot_require_postgres(self) -> None:
        self.assertNotIn("PgPool", inspect.getsource(GENERATOR.requires_postgres))
        self.assertFalse(GENERATOR.requires_postgres("console-facilities-rest", "test.unit"))
        with self.assertRaisesRegex(ValueError, "missing reviewed resource metadata"):
            GENERATOR.requires_postgres(
                "console-facilities-rest", "test.integration", "tests/comment_only.rs"
            )

    def test_reviewed_database_integration_target_is_postgres_bound(self) -> None:
        self.assertTrue(
            GENERATOR.requires_postgres(
                "console-benefit-adapter-postgres",
                "test.integration",
                "tests/catalog_rls_surfaces_as_runtime_role.rs",
            )
        )
        labels = GENERATOR.test_labels(
            "backend/crates/benefit/adapter-postgres", "test.integration", True
        )
        self.assertIn("resource.postgres", labels)
        self.assertIn("needs-postgres", labels)

    def test_attendance_self_service_integration_is_postgres_bound(self) -> None:
        self.assertTrue(
            GENERATOR.requires_postgres(
                "console-attendance-adapter-postgres",
                "test.integration",
                "tests/self_service.rs",
            )
        )

    def test_equipment_discoveries_have_reviewed_resources(self) -> None:
        self.assertTrue(
            GENERATOR.requires_postgres(
                "console-app", "test.integration", "tests/equipment_3r_api.rs"
            )
        )
        self.assertFalse(GENERATOR.requires_postgres("console-equipment-domain", "test.unit"))

    def test_integration_resource_lookup_requires_a_target_path(self) -> None:
        with self.assertRaises(ValueError):
            GENERATOR.requires_postgres("console-benefit-adapter-postgres", "test.integration")

    def test_unreviewed_discovered_test_fails_generation_preflight(self) -> None:
        discovered = {
            ("console-benefit-rest", "test.unit", None),
            ("console-benefit-rest", "test.integration", "tests/unreviewed.rs"),
        }
        requirements = {"console-benefit-rest": {"unit": "none"}}
        with self.assertRaisesRegex(ValueError, "missing"):
            GENERATOR.validate_resource_metadata(discovered, requirements)

    def test_metadata_is_exhaustive_for_current_generator_targets(self) -> None:
        discovered = set()
        for directory in GENERATOR.find_members():
            package = GENERATOR.load(directory)["package"]["name"]
            discovered.update(GENERATOR.discovered_test_resource_keys(directory, package))
        GENERATOR.validate_resource_metadata(discovered)

    def test_every_discovered_target_has_exactly_one_test_and_resource_label(self) -> None:
        for directory in GENERATOR.find_members():
            package_name = GENERATOR.load(directory)["package"]["name"]
            package_path = str(Path(directory).relative_to(GENERATOR.REPO))
            for _, test_type, test_file in GENERATOR.discovered_test_resource_keys(
                directory, package_name
            ):
                labels = GENERATOR.test_labels(
                    package_path,
                    test_type,
                    GENERATOR.requires_postgres(package_name, test_type, test_file),
                )
                self.assertEqual(1, len(set(labels) & GENERATOR.TEST_TYPE_LABELS))
                self.assertEqual(1, len(set(labels) & GENERATOR.RESOURCE_LABELS))


if __name__ == "__main__":
    unittest.main()
