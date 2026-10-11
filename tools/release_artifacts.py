#!/usr/bin/env python3
"""Cargo's actual executable outputs must match the release recipe's directory."""
import json, pathlib, sys

expected = pathlib.Path(sys.argv[1]).absolute()
seen = set()
for line in sys.stdin:
    item = json.loads(line)
    if item.get('reason') != 'compiler-artifact' or not item.get('executable'):
        continue
    name = item['target']['name']
    if name not in ('yandi', 'yandi-release'):
        continue
    actual = pathlib.Path(item['executable']).absolute()
    if actual != expected / name:
        raise ValueError(f'Cargo artifact directory mismatch: {actual} != {expected / name}')
    seen.add(name)
if seen != {'yandi', 'yandi-release'}:
    raise ValueError('Cargo did not report both release executables')
