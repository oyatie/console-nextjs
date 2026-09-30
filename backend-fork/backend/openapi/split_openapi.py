#!/usr/bin/env python3
"""Split backend/openapi/openapi.yaml into per-face + shared YAML slices.

Oracle: loading the slices back through console-openapi-gen must reproduce
compose-canonical bytes of the published document (paths/schemas content
preserved; key order becomes compose's sorted order).

This script is mechanical evidence for console-b4z — run from repo root:
  python3 backend/openapi/split_openapi.py
"""

from __future__ import annotations

import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
YAML_PATH = ROOT / "backend/openapi/openapi.yaml"
SHARED_DIR = ROOT / "backend/openapi/shared"
FACES = [
    "analytics-quant",
    "attendance",
    "benefit",
    "comms",
    "compliance",
    "consulting",
    "dispatch",
    "docs",
    "equipment",
    "evaluation",
    "facilities",
    "finance-gl",
    "financial",
    "governance",
    "identity",
    "inbox",
    "inspection",
    "inventory",
    "leave",
    "logistics",
    "messenger",
    "notices",
    "notifications",
    "ontology",
    "orgchange",
    "payroll",
    "production",
    "recruiting",
    "registry",
    "reporting",
    "sales",
    "support",
    "todos",
    "workorder",
]

MANUAL_PREFIXES = [
    ("/api/v1/workflow-studio", "registry"),
    ("/api/v1/workflow-tasks", "registry"),
    ("/api/v1/workflow-runs", "registry"),
    ("/api/v1/approval-inbox", "registry"),
    ("/api/v1/integrity", "governance"),
    ("/api/v1/object-links", "ontology"),
    ("/api/v1/object-types", "ontology"),
    ("/api/v1/object-actions", "ontology"),
    ("/api/v1/link-types", "ontology"),
    ("/api/v1/objects", "ontology"),
    ("/api/objects", "ontology"),
    ("/api/v1/search", "ontology"),
    ("/api/v1/series", "ontology"),
    ("/api/v1/lifecycles", "governance"),
    ("/api/v1/mobile/collaboration", "comms"),
    ("/api/v1/collaboration", "comms"),
    ("/api/v1/office", "docs"),
    ("/api/v1/mail", "comms"),
    ("/api/messenger", "messenger"),
    ("/api/v1/auth", "identity"),
    ("/api/v1/console", "identity"),
    ("/api/platform", "identity"),
    ("/api/v1/group-admin", "identity"),
    ("/api/v1/users", "identity"),
    ("/api/v1/passkeys", "identity"),
    ("/api/v1/policy", "identity"),
    ("/api/v1/employees", "orgchange"),
    ("/api/v1/hr", "orgchange"),
    ("/api/audit", "governance"),
    ("/api/v1/audit", "governance"),
    ("/api/work-orders", "workorder"),
    ("/api/daily-work-plans", "workorder"),
    ("/api/target-change-requests", "workorder"),
    ("/api/approval-items", "workorder"),
    ("/api/v1/ws", "messenger"),
    ("/healthz", "identity"),
    ("/readyz", "identity"),
    ("/.well-known", "identity"),
    ("/api/v1/me/notifications", "notifications"),
    ("/api/v1/me/todos", "todos"),
    ("/api/v1/me/inbox", "inbox"),
    ("/api/v1/me/dispatch", "dispatch"),
    ("/api/v1/me/", "identity"),
    ("/api/v1/period-locks", "finance-gl"),
    ("/api/v1/recruiting/applicants", "recruiting"),
    ("/api/v1/equipment", "equipment"),
]


def norm(path: str) -> str:
    return re.sub(r"\{[^}]+\}", "{}", path)


def indent_of(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def block_after(lines: list[str], header: str, header_indent: int) -> tuple[list[str], int]:
    """Return body lines under `header` and the index of the header line.

    Full-line comments (including zero-indent editorial notes that leaked into
    the published document) do not terminate a section — only a real key at
    `header_indent` or less does.
    """
    for i, line in enumerate(lines):
        if line == header:
            body: list[str] = []
            for j in range(i + 1, len(lines)):
                stripped = lines[j].strip()
                if stripped == "" or stripped.startswith("#"):
                    body.append(lines[j])
                    continue
                if indent_of(lines[j]) <= header_indent:
                    break
                body.append(lines[j])
            while body and body[-1].strip() == "":
                body.pop()
            return body, i
    raise KeyError(header)


def split_named_map(body_lines: list[str], entry_indent: int) -> dict[str, str]:
    """Split a YAML map whose keys sit at `entry_indent` into name -> body text."""
    entries: dict[str, str] = {}
    i = 0
    while i < len(body_lines):
        line = body_lines[i]
        stripped = line.strip()
        if stripped == "" or stripped.startswith("#"):
            i += 1
            continue
        if indent_of(line) != entry_indent:
            i += 1
            continue
        m = re.match(r"^ {%d}([A-Za-z0-9._-]+):(.*)$" % entry_indent, line)
        if not m:
            i += 1
            continue
        name = m.group(1)
        rest = m.group(2)
        chunk: list[str] = []
        if rest.strip():
            # inline value on the same line — keep as a one-line body
            chunk.append(rest.lstrip() + ("\n" if not rest.endswith("\n") else ""))
            # Actually store without forcing; rebuild below
            body = rest.lstrip()
            if not body.endswith("\n"):
                body += "\n"
            entries[name] = body
            i += 1
            continue
        i += 1
        while i < len(body_lines):
            nxt = body_lines[i]
            if nxt.strip() == "":
                chunk.append(nxt)
                i += 1
                continue
            if indent_of(nxt) <= entry_indent:
                break
            chunk.append(nxt)
            i += 1
        while chunk and chunk[-1].strip() == "":
            chunk.pop()
        # Strip the entry_indent+2 common pad later via compose reindent; keep as authored.
        entries[name] = "\n".join(chunk) + ("\n" if chunk else "")
    return entries


def split_path_operations(path_body_lines: list[str]) -> dict[str, str]:
    """Path body lines are indented 4 spaces under `  /path:`."""
    return split_named_map(path_body_lines, 4)


def path_file_stem(path: str) -> str:
    stem = path.strip("/")
    stem = stem.replace("/", "__")
    stem = stem.replace("{", "").replace("}", "")
    stem = re.sub(r"[^A-Za-z0-9._-]+", "_", stem)
    return stem or "root"


def extract_face_route_norms() -> dict[str, set[str]]:
    api_pat = re.compile(r'"(/(?:api|healthz|readyz|\.well-known)[^"]*)"')
    route_pat = re.compile(r'\.route\(\s*"([^"]+)"')
    out: dict[str, set[str]] = {}
    for face in FACES:
        found: set[str] = set()
        src = ROOT / f"backend/crates/{face}/rest/src"
        for rs in src.rglob("*.rs"):
            text = rs.read_text(errors="ignore")
            found.update(m.group(1) for m in api_pat.finditer(text))
            found.update(m.group(1) for m in route_pat.finditer(text))
        out[face] = {norm(p) for p in found if p.startswith("/") and " " not in p}
    return out


def manual_face(path: str) -> str | None:
    best: tuple[str, str] | None = None
    for pref, face in MANUAL_PREFIXES:
        boundary = pref.rstrip("/")
        if path == boundary or path.startswith(boundary + "/"):
            if best is None or len(pref) > len(best[0]):
                best = (pref, face)
    return best[1] if best else None


def assign_paths(pub_paths: list[str], face_norms: dict[str, set[str]]) -> dict[str, list[str]]:
    owned: dict[str, list[str]] = defaultdict(list)
    for path in pub_paths:
        n = norm(path)
        owners = [f for f, ns in face_norms.items() if n in ns]
        if len(owners) == 1:
            owned[owners[0]].append(path)
            continue
        if len(owners) > 1:
            mf = manual_face(path) or owners[0]
            owned[mf].append(path)
            continue
        mf = manual_face(path)
        if not mf:
            raise SystemExit(f"unowned path: {path}")
        owned[mf].append(path)
    for face in FACES:
        owned.setdefault(face, [])
    return owned


def schema_refs(body: str) -> list[str]:
    """Mirror console_contracts::schema_refs enough for ownership."""
    refs: list[str] = []
    for m in re.finditer(r"#/components/schemas/([A-Za-z0-9._-]+)", body):
        refs.append(m.group(1))
    return refs


def rust_string(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def write_manifest(
    face: str,
    crate_name: str,
    paths: dict[str, dict[str, str]],
    schemas: dict[str, str],
    external: list[str],
) -> None:
    import json
    rest = ROOT / f"backend/crates/{face}/rest"
    payload = {
        "source": crate_name,
        "paths": {path: sorted(ops) for path, ops in sorted(paths.items())},
        "schemas": sorted(schemas),
        "external_schemas": external,
    }
    (rest / "openapi" / "manifest.json").write_text(json.dumps(payload, indent=2) + "\n")


def write_openapi_rs(
    face: str,
    crate_name: str,
    paths: dict[str, dict[str, str]],
    schemas: dict[str, str],
    external: list[str],
) -> None:
    rest = ROOT / f"backend/crates/{face}/rest"
    openapi_dir = rest / "openapi"
    src = rest / "src" / "openapi.rs"
    lines: list[str] = [
        "//! This face's slice of the published OpenAPI contract.",
        "//!",
        "//! Bodies live as YAML under `openapi/` and are pulled in via `include_str!`.",
        "//! `console_contracts` re-indents them; composition rejects duplicate keys.",
        "",
        "use console_contracts::{Fragment, NamedYaml, Operation, PathItem};",
        "",
        "/// This face's contribution to the composed OpenAPI document.",
        "pub const OPENAPI_FRAGMENT: Fragment = Fragment {",
        f"    source: {rust_string(crate_name)},",
        "    paths: PATHS,",
        "    schemas: SCHEMAS,",
        "    parameters: &[],",
        "    responses: &[],",
        "    security_schemes: &[],",
        "    external_schemas: EXTERNAL_SCHEMAS,",
        "};",
        "",
        "const EXTERNAL_SCHEMAS: &[&str] = &[",
    ]
    for name in external:
        lines.append(f"    {rust_string(name)},")
    lines.append("];")
    lines.append("")
    lines.append("const PATHS: &[PathItem] = &[")
    for path in sorted(paths):
        ops = paths[path]
        lines.append("    PathItem {")
        lines.append(f"        path: {rust_string(path)},")
        lines.append("        operations: &[")
        for method in sorted(ops):
            stem = path_file_stem(path)
            rel = f"../openapi/paths/{stem}.{method}.yaml"
            lines.append("            Operation {")
            lines.append(f"                method: {rust_string(method)},")
            lines.append(f"                body: include_str!({rust_string(rel)}),")
            lines.append("            },")
        lines.append("        ],")
        lines.append("    },")
    lines.append("];")
    lines.append("")
    lines.append("const SCHEMAS: &[NamedYaml] = &[")
    for name in sorted(schemas):
        rel = f"../openapi/schemas/{name}.yaml"
        lines.append("    NamedYaml {")
        lines.append(f"        name: {rust_string(name)},")
        lines.append(f"        body: include_str!({rust_string(rel)}),")
        lines.append("    },")
    lines.append("];")
    lines.append("")
    src.write_text("\n".join(lines))


def ensure_mod_and_dep(face: str) -> None:
    rest = ROOT / f"backend/crates/{face}/rest"
    lib = rest / "src" / "lib.rs"
    text = lib.read_text()
    if "mod openapi;" not in text:
        # Insert after the module doc / before first non-comment item.
        insert_at = 0
        lines = text.splitlines(True)
        i = 0
        while i < len(lines) and (
            lines[i].startswith("//") or lines[i].startswith("#!") or lines[i].strip() == ""
        ):
            i += 1
        insert_at = i
        lines.insert(insert_at, "mod openapi;\n")
        lines.insert(insert_at + 1, "pub use openapi::OPENAPI_FRAGMENT;\n")
        lines.insert(insert_at + 2, "\n")
        lib.write_text("".join(lines))
    elif "OPENAPI_FRAGMENT" not in text:
        text = text.replace("mod openapi;\n", "mod openapi;\npub use openapi::OPENAPI_FRAGMENT;\n")
        lib.write_text(text)

    cargo = rest / "Cargo.toml"
    ctext = cargo.read_text()
    if "console-contracts" not in ctext:
        # Insert into [dependencies]
        if "[dependencies]\n" in ctext:
            ctext = ctext.replace(
                "[dependencies]\n",
                '[dependencies]\nconsole-contracts = { path = "../../contracts" }\n',
                1,
            )
        else:
            ctext += '\n[dependencies]\nconsole-contracts = { path = "../../contracts" }\n'
        cargo.write_text(ctext)


def write_gen_registry() -> None:
    """Emit contracts/src/gen_registry.rs for console-openapi-gen."""
    import json

    lines: list[str] = [
        "//! AUTO-GENERATED by backend/openapi/split_openapi.py — do not edit.",
        "#![allow(clippy::all)]",
        "",
        "use console_contracts::{DocumentPreamble, Fragment, NamedYaml, Operation, PathItem};",
        "",
    ]
    shared = json.loads((SHARED_DIR / "manifest.json").read_text())
    ver = (SHARED_DIR / "openapi.version").read_text().strip()
    lines += [
        "pub const PREAMBLE: DocumentPreamble = DocumentPreamble {",
        f"    openapi: {json.dumps(ver)},",
        '    info: include_str!("../../../openapi/shared/info.yaml"),',
        "};",
        "",
        "pub const SHARED: Fragment = Fragment {",
        f"    source: {json.dumps(shared['source'])},",
        "    paths: &[],",
        "    schemas: SHARED_SCHEMAS,",
        "    parameters: SHARED_PARAMETERS,",
        "    responses: SHARED_RESPONSES,",
        "    security_schemes: SHARED_SECURITY_SCHEMES,",
        "    external_schemas: &[],",
        "};",
        "",
    ]

    def emit_named(const: str, names: list[str], folder: str) -> None:
        lines.append(f"const {const}: &[NamedYaml] = &[")
        for name in names:
            rel = f"../../../openapi/shared/{folder}/{name}.yaml"
            lines.append("    NamedYaml {")
            lines.append(f"        name: {json.dumps(name)},")
            lines.append(f"        body: include_str!({json.dumps(rel)}),")
            lines.append("    },")
        lines.append("];")
        lines.append("")

    emit_named("SHARED_SECURITY_SCHEMES", shared["security_schemes"], "securitySchemes")
    emit_named("SHARED_PARAMETERS", shared["parameters"], "parameters")
    emit_named("SHARED_RESPONSES", shared["responses"], "responses")
    emit_named("SHARED_SCHEMAS", shared["schemas"], "schemas")

    face_consts: list[str] = []
    for face in FACES:
        manifest = json.loads(
            (ROOT / f"backend/crates/{face}/rest/openapi/manifest.json").read_text()
        )
        const = face.upper().replace("-", "_") + "_FRAGMENT"
        face_consts.append(const)
        base = f"../../{face}/rest/openapi"
        lines += [
            f"pub const {const}: Fragment = Fragment {{",
            f"    source: {json.dumps(manifest['source'])},",
            f"    paths: {const}_PATHS,",
            f"    schemas: {const}_SCHEMAS,",
            "    parameters: &[],",
            "    responses: &[],",
            "    security_schemes: &[],",
            f"    external_schemas: {const}_EXTERNAL,",
            "};",
            "",
            f"const {const}_EXTERNAL: &[&str] = &[",
        ]
        for n in manifest["external_schemas"]:
            lines.append(f"    {json.dumps(n)},")
        lines += ["];", "", f"const {const}_PATHS: &[PathItem] = &["]
        for path, methods in manifest["paths"].items():
            stem = path_file_stem(path)
            lines += [
                "    PathItem {",
                f"        path: {json.dumps(path)},",
                "        operations: &[",
            ]
            for method in methods:
                rel = f"{base}/paths/{stem}.{method}.yaml"
                lines += [
                    "            Operation {",
                    f"                method: {json.dumps(method)},",
                    f"                body: include_str!({json.dumps(rel)}),",
                    "            },",
                ]
            lines += ["        ],", "    },"]
        lines += ["];", "", f"const {const}_SCHEMAS: &[NamedYaml] = &["]
        for name in manifest["schemas"]:
            rel = f"{base}/schemas/{name}.yaml"
            lines += [
                "    NamedYaml {",
                f"        name: {json.dumps(name)},",
                f"        body: include_str!({json.dumps(rel)}),",
                "    },",
            ]
        lines += ["];", ""]

    lines.append("pub const ALL_FRAGMENTS: &[&Fragment] = &[")
    lines.append("    &SHARED,")
    for c in face_consts:
        lines.append(f"    &{c},")
    lines.append("];")
    lines.append("")

    out = ROOT / "backend/crates/contracts/src/gen_registry.rs"
    out.write_text("\n".join(lines) + "\n")
    print(f"wrote {out}")


def main() -> int:
    raw = YAML_PATH.read_text()
    lines = raw.splitlines()

    # Preamble
    if not lines[0].startswith("openapi:"):
        raise SystemExit("expected openapi: on line 1")
    openapi_ver = lines[0].split(":", 1)[1].strip()
    info_body, _ = block_after(lines, "info:", 0)

    paths_body, _ = block_after(lines, "paths:", 0)
    # Split paths at indent 2
    path_map: dict[str, list[str]] = {}
    i = 0
    while i < len(paths_body):
        line = paths_body[i]
        if line.strip() == "":
            i += 1
            continue
        m = re.match(r"^  (/[^:]*):$", line)
        if not m:
            i += 1
            continue
        path = m.group(1)
        i += 1
        body: list[str] = []
        while i < len(paths_body):
            nxt = paths_body[i]
            if nxt.strip() == "":
                body.append(nxt)
                i += 1
                continue
            if indent_of(nxt) <= 2:
                break
            body.append(nxt)
            i += 1
        while body and body[-1].strip() == "":
            body.pop()
        path_map[path] = body

    components_body, _ = block_after(lines, "components:", 0)
    # Re-join with synthetic header lines for subsection extraction
    comp_lines = ["components:"] + components_body

    def section(name: str) -> dict[str, str]:
        header = f"  {name}:"
        try:
            body, _ = block_after(comp_lines, header, 2)
        except KeyError:
            return {}
        return split_named_map(body, 4)

    security = section("securitySchemes")
    parameters = section("parameters")
    responses = section("responses")
    schemas = section("schemas")

    face_norms = extract_face_route_norms()
    owned_paths = assign_paths(sorted(path_map), face_norms)

    # Build per-face operation maps
    face_ops: dict[str, dict[str, dict[str, str]]] = {}
    for face, paths in owned_paths.items():
        face_ops[face] = {}
        for path in paths:
            ops = split_path_operations(path_map[path])
            if not ops:
                raise SystemExit(f"path {path} has no operations")
            face_ops[face][path] = ops

    # Schema ownership via ref closure
    def closure_for(ops_by_path: dict[str, dict[str, str]]) -> set[str]:
        queue: list[str] = []
        for ops in ops_by_path.values():
            for body in ops.values():
                queue.extend(schema_refs(body))
        seen: set[str] = set()
        while queue:
            name = queue.pop()
            if name in seen:
                continue
            seen.add(name)
            body = schemas.get(name)
            if body:
                queue.extend(schema_refs(body))
        return seen

    face_closures = {face: closure_for(ops) for face, ops in face_ops.items()}
    schema_owners: dict[str, set[str]] = defaultdict(set)
    for face, names in face_closures.items():
        for name in names:
            if name in schemas:
                schema_owners[name].add(face)

    face_schemas: dict[str, dict[str, str]] = {f: {} for f in FACES}
    shared_schemas: dict[str, str] = {}
    for name, body in schemas.items():
        owners = schema_owners.get(name, set())
        if len(owners) == 1:
            face = next(iter(owners))
            face_schemas[face][name] = body
        else:
            shared_schemas[name] = body

    # Wipe and rewrite slice dirs
    if SHARED_DIR.exists():
        import shutil

        shutil.rmtree(SHARED_DIR)
    SHARED_DIR.mkdir(parents=True)
    (SHARED_DIR / "securitySchemes").mkdir()
    (SHARED_DIR / "parameters").mkdir()
    (SHARED_DIR / "responses").mkdir()
    (SHARED_DIR / "schemas").mkdir()

    (SHARED_DIR / "info.yaml").write_text("\n".join(info_body) + "\n")
    (SHARED_DIR / "openapi.version").write_text(openapi_ver + "\n")
    for name, body in security.items():
        (SHARED_DIR / "securitySchemes" / f"{name}.yaml").write_text(body if body.endswith("\n") else body + "\n")
    for name, body in parameters.items():
        (SHARED_DIR / "parameters" / f"{name}.yaml").write_text(body if body.endswith("\n") else body + "\n")
    for name, body in responses.items():
        (SHARED_DIR / "responses" / f"{name}.yaml").write_text(body if body.endswith("\n") else body + "\n")
    for name, body in shared_schemas.items():
        (SHARED_DIR / "schemas" / f"{name}.yaml").write_text(body if body.endswith("\n") else body + "\n")

    import json as _json
    (SHARED_DIR / "manifest.json").write_text(
        _json.dumps(
            {
                "source": "console-openapi-shared",
                "openapi": openapi_ver,
                "security_schemes": sorted(security),
                "parameters": sorted(parameters),
                "responses": sorted(responses),
                "schemas": sorted(shared_schemas),
            },
            indent=2,
        )
        + "\n"
    )

    for face in FACES:
        rest = ROOT / f"backend/crates/{face}/rest"
        odir = rest / "openapi"
        if odir.exists():
            import shutil

            shutil.rmtree(odir)
        (odir / "paths").mkdir(parents=True)
        (odir / "schemas").mkdir(parents=True)
        for path, ops in face_ops[face].items():
            stem = path_file_stem(path)
            for method, body in ops.items():
                # strip the 4-space method indent already excluded; body is under method at 6
                # compose reindents — write body as stored (lines under method key)
                # split_named_map kept lines with their original indent (6+)
                # Strip common method-body indent down for readability: remove 6 spaces if present
                cleaned_lines = []
                for ln in body.splitlines():
                    if ln.startswith("      "):
                        cleaned_lines.append(ln[6:])
                    else:
                        cleaned_lines.append(ln.lstrip() if ln.strip() == "" else ln)
                cleaned = "\n".join(cleaned_lines) + "\n"
                (odir / "paths" / f"{stem}.{method}.yaml").write_text(cleaned)
        for name, body in face_schemas[face].items():
            cleaned_lines = []
            for ln in body.splitlines():
                if ln.startswith("      "):
                    cleaned_lines.append(ln[6:])
                else:
                    cleaned_lines.append(ln)
            cleaned = "\n".join(cleaned_lines) + "\n"
            (odir / "schemas" / f"{name}.yaml").write_text(cleaned)

        crate = f"console-{face}-rest"
        # cargo package names use hyphens; finance-gl stays finance-gl
        external = sorted(
            (face_closures[face] & set(shared_schemas))
            | (face_closures[face] - set(face_schemas[face]) - set(shared_schemas))
        )
        # Only names that exist somewhere
        external = [n for n in external if n in schemas and n not in face_schemas[face]]
        write_openapi_rs(face, crate, face_ops[face], face_schemas[face], external)
        write_manifest(face, crate, face_ops[face], face_schemas[face], external)
        ensure_mod_and_dep(face)

    # Summary
    print(f"paths: {len(path_map)}")
    print(f"schemas total: {len(schemas)} shared: {len(shared_schemas)}")
    print(f"securitySchemes: {len(security)} parameters: {len(parameters)} responses: {len(responses)}")
    for face in FACES:
        print(
            f"  {face}: paths={len(face_ops[face])} schemas={len(face_schemas[face])} "
            f"external={len([n for n in face_closures[face] if n in schemas and n not in face_schemas[face]])}"
        )
    empty = [f for f in FACES if not face_ops[f]]
    if empty:
        raise SystemExit(f"faces with zero paths: {empty}")
    write_gen_registry()
    return 0


if __name__ == "__main__":
    sys.exit(main())
