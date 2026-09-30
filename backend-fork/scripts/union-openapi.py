#!/usr/bin/env python3
"""Structurally union two OpenAPI YAML files (merge-conflict resolution).

Usage: union-openapi.py BASE_WITH_PRIORITY OTHER OUTPUT

Copies OTHER's paths and ALL component sections that BASE lacks. On a
component-key clash with DIFFERENT content, OTHER's entry is renamed with a
`V2` suffix (V3, ... if taken) and every $ref to it inside OTHER's copied
paths/components is rewritten - both sides' routes keep working.

The app serves the committed file via include_str! - after any contract
merge, run this, then `npm run gen:api` and the full check suite.
"""
import json
import sys

import yaml

base = yaml.safe_load(open(sys.argv[1]))
other = yaml.safe_load(open(sys.argv[2]))

renames = {}
bc = base.setdefault('components', {})
oc = other.get('components', {})
for section, entries in oc.items():
    dst = bc.setdefault(section, {})
    for key, val in entries.items():
        if key in dst and dst[key] != val:
            new_key = None
            for i in range(2, 10):
                cand = f"{key}V{i}"
                if cand not in dst and cand not in entries:
                    new_key = cand
                    break
            assert new_key, f"no free rename for {section}/{key}"
            renames[f"#/components/{section}/{key}"] = (
                f"#/components/{section}/{new_key}", section, key, new_key)

def rewrite(node):
    if isinstance(node, dict):
        ref = node.get('$ref')
        if isinstance(ref, str) and ref in renames:
            node['$ref'] = renames[ref][0]
        for v in node.values():
            rewrite(v)
    elif isinstance(node, list):
        for v in node:
            rewrite(v)

other_paths = {k: v for k, v in other.get('paths', {}).items() if k not in base['paths']}
rewrite(other_paths)
rewrite(oc)

for k, v in other_paths.items():
    base['paths'][k] = v
for section, entries in oc.items():
    dst = bc.setdefault(section, {})
    for key, val in entries.items():
        if key in dst and dst[key] != val:
            _, _, _, new_key = renames[f"#/components/{section}/{key}"]
            dst[new_key] = val
        else:
            dst.setdefault(key, val)

open(sys.argv[3], 'w').write(yaml.safe_dump(base, allow_unicode=True, sort_keys=False, width=100))
print(f"unioned -> {sys.argv[3]}")
if renames:
    print("clash renames:", json.dumps({k: v[0] for k, v in renames.items()}, indent=2))
