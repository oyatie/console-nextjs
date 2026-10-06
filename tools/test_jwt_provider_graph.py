import importlib.util
import contextlib
import io
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "jwt_provider_graph", Path(__file__).with_name("check-jwt-provider-graph.py"))
graph = importlib.util.module_from_spec(spec)
spec.loader.exec_module(graph)

OWNERS = "\n".join(name + " v0.1.0 (/workspace/" + name + ")|" for name in sorted(graph.OWNERS))
AWS = OWNERS + "\njsonwebtoken v10.4.0|aws_lc_rs,pem,use_pem"


class ProviderGraphTests(unittest.TestCase):
    def test_actual_native_rows_and_duplicates(self):
        self.assertEqual(graph.inspect_graph(AWS + "\n\njsonwebtoken v10.4.0|aws_lc_rs,pem,use_pem (*)\n\n"), [])
        # RS256 is deliberately supported; only the vulnerable crate is denied.
        self.assertEqual(graph.inspect_graph(AWS + "\nrs256-wire-tests v0.1.0|"), [])
        old = OWNERS + "\njsonwebtoken v10.4.0|rsa,rust_crypto,use_pem\nrsa v0.9.10|pem"
        self.assertIn("vulnerable rsa crate is selected", graph.inspect_graph(old))
        self.assertIn("JWT must select AWS-LC and PEM without RustCrypto/RSA", graph.inspect_graph(old))

    def test_missing_dual_and_conflicting_providers_fail_closed(self):
        for bad in (OWNERS, OWNERS + "\njsonwebtoken v10.4.0|use_pem",
                    AWS.replace("aws_lc_rs", "aws_lc_rs,rust_crypto"),
                    AWS + "\njsonwebtoken v10.5.0|aws_lc_rs,use_pem",
                    AWS + "\njsonwebtoken v10.4.0|rust_crypto,use_pem",
                    AWS + "\nrsa v0.10.0|", AWS.replace("console-app", "unknown-app")):
            with self.subTest(graph=bad):
                self.assertTrue(graph.inspect_graph(bad))

    def test_empty_truncated_or_malformed_output_fails_closed(self):
        for bad in ("", "warning: nothing to print", AWS + "\ntruncated",
                    AWS + "\npackage v1.0.0|feature,", AWS + "\n|aws_lc_rs",
                    AWS + "\npackage vbogus|", AWS + "\npackage v1.0.0|x|y"):
            with self.subTest(graph=bad):
                self.assertTrue(graph.inspect_graph(bad))

    def test_command_evidence_and_credential_environment(self):
        with patch.dict(os.environ, {"LIVE_INTEGRATION_TOKEN": "must-not-inherit"}):
            code, stdout, stderr, failure = graph.bounded_command([
                sys.executable, "-c",
                "import os,sys; print(os.environ.get('LIVE_INTEGRATION_TOKEN', 'absent')); sys.stderr.write('diagnostic')"])
        self.assertEqual((code, stdout, stderr, failure), (0, b"absent\n", b"diagnostic", None))

    def test_timeout_output_budget_and_read_error_reap_owned_child(self):
        native_read = os.read
        children = []
        def fail_output_read(fd, size):
            if children and fd in (children[-1].stdout.fileno(), children[-1].stderr.fileno()):
                raise OSError("read failed")
            return native_read(fd, size)
        commands = [
            (patch.object(graph, "TIMEOUT_SECONDS", 0.05), "import time; time.sleep(10)", "deadline"),
            (patch.object(graph, "MAX_OUTPUT_BYTES", 128), "import time; print('x' * 1024, flush=True); time.sleep(10)", "budget"),
            (patch.object(graph.os, "read", side_effect=fail_output_read), "import time; print('ready', flush=True); time.sleep(10)", "read failed"),
        ]
        native_popen = subprocess.Popen
        def record_child(*args, **kwargs):
            process = native_popen(*args, **kwargs)
            children.append(process)
            return process
        for override, source, diagnostic in commands:
            with self.subTest(diagnostic=diagnostic), override, patch.object(graph.subprocess, "Popen", side_effect=record_child):
                children.clear()
                code, _, _, failure = graph.bounded_command([sys.executable, "-c", source])
                self.assertIn(diagnostic, failure)
                self.assertIsNotNone(children[-1].poll())
                self.assertEqual(code, children[-1].returncode)
                self.assertTrue(children[-1].stdout.closed)
                self.assertTrue(children[-1].stderr.closed)

    def test_already_exited_group_permission_race_still_reaps_and_fails(self):
        native_popen, native_read = subprocess.Popen, os.read
        children = []
        def record_child(*args, **kwargs):
            process = native_popen(*args, **kwargs)
            children.append(process)
            return process
        def fail_after_exit(fd, size):
            if children and fd in (children[-1].stdout.fileno(), children[-1].stderr.fileno()):
                children[-1].wait(timeout=2)
                raise OSError("read failed after exit")
            return native_read(fd, size)
        with patch.object(graph.subprocess, "Popen", side_effect=record_child), \
             patch.object(graph.os, "read", side_effect=fail_after_exit), \
             patch.object(graph.os, "killpg", side_effect=PermissionError("exited group")):
            with self.assertRaises(PermissionError):
                graph.bounded_command([sys.executable, "-c", "print('ready')"])
        self.assertEqual(children[-1].returncode, 0)
        self.assertTrue(children[-1].stdout.closed)
        self.assertTrue(children[-1].stderr.closed)

    def test_terminal_leader_does_not_hide_denied_live_descendant_fencing(self):
        native_popen, native_read, native_killpg = subprocess.Popen, os.read, os.killpg
        children = []
        def record_child(*args, **kwargs):
            process = native_popen(*args, **kwargs)
            children.append(process)
            return process
        def fail_after_leader_exit(fd, size):
            if children and fd == children[-1].stdout.fileno():
                data = native_read(fd, size)
                if data:
                    children[-1].wait(timeout=2)
                    raise OSError("leader exited, descendant remains")
                return data
            return native_read(fd, size)
        source = "import os,time\nif os.fork() == 0:\n time.sleep(10)\nelse:\n print('ready', flush=True)"
        try:
            with patch.object(graph.subprocess, "Popen", side_effect=record_child), \
                 patch.object(graph.os, "read", side_effect=fail_after_leader_exit), \
                 patch.object(graph.os, "killpg", side_effect=PermissionError("group fencing denied")):
                with self.assertRaises(PermissionError):
                    graph.bounded_command([sys.executable, "-c", source])
            self.assertEqual(children[-1].returncode, 0)
            native_killpg(children[-1].pid, 0)  # The descendant group is still alive.
        finally:
            if children:
                native_killpg(children[-1].pid, signal.SIGKILL)

    def test_matrix_errors_and_source_drift_fail_closed_with_all_rows(self):
        for outcome, hashes in ((OSError("cargo unavailable"), [{}, {}]),
                                ((1, b"", b"cargo failed", None), [{}, {}]),
                                ((0, AWS.encode(), b"", None), [{}, {"Cargo.lock": "changed"}])):
            with self.subTest(outcome=outcome), patch.object(sys, "argv", ["guard"]), \
                 patch.object(graph, "graph_inputs", side_effect=hashes), \
                 patch.object(graph, "bounded_command") as command, contextlib.redirect_stdout(io.StringIO()) as output:
                if isinstance(outcome, Exception):
                    command.side_effect = outcome
                else:
                    command.return_value = outcome
                self.assertEqual(graph.main(), 1)
                receipt = json.loads(output.getvalue())
                self.assertEqual(receipt["status"], "failed")
                self.assertEqual(len(receipt["rows"]), 6)
                self.assertEqual(command.call_count, 6)


if __name__ == "__main__":
    unittest.main()
