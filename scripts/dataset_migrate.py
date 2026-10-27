#!/usr/bin/env python3
"""Rebuild stale dataset examples on the current data/schema.json.

Example rows are positional, so a float added to a model after the example was
authored shifts the tail (room 10-p2 got col_bri = 0.1 and rendered black). An
example whose width differs from its model's current width is rebuilt from the
`keys` it carries (name -> value per tick; old Spanish names go through
crates/metal-schema/src/param_aliases.rs), everything unkeyed at its default.
A float that was only ever a base value (never keyed) is lost; the report
lists any key name it cannot place.

  dataset_migrate.py [--dir DIR] [--write]     dry run unless --write
"""
import json, re, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import dataset_example as de  # noqa: E402

ALIASES = dict(re.findall(r'\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*\)', (ROOT / "crates/metal-schema/src/param_aliases.rs").read_text()))


def rename(name, keys):
    if name in keys:
        return name
    pre, _, rest = name.partition(":")
    if rest and f"{pre}:{ALIASES.get(rest, rest)}" in keys:
        return f"{pre}:{ALIASES.get(rest, rest)}"
    base, hash_, n = name.partition("#")
    cand = ALIASES.get(base, base) + (hash_ + n)
    return cand if cand in keys else None


def main():
    a = sys.argv[1:]
    d = Path(a[a.index("--dir") + 1]) if "--dir" in a else de.EXAMPLES
    write = "--write" in a
    de.EXAMPLES = d
    de.ROOT = d.resolve()  # build() prints dest relative to it
    for f in sorted(d.glob("*.json")):
        ex = json.loads(f.read_text())
        m = de.model_schema(ex["model"])
        items = {i["key"] for i in de.layout(m)}
        width = sum(i["width"] for i in de.layout(m))
        if len(ex["ticks"][0]["floats"]) == width:
            continue
        lost, keys = set(), []
        for k in ex.get("keys", []):
            s = {}
            for n, v in k["set"].items():
                r = rename(n, items)
                if r is None:
                    lost.add(n)
                else:
                    s[r] = v
            keys.append({"tick": k["tick"], "set": s, "why": k.get("why")})
        print(f"{f.name}: {ex['model']} {len(ex['ticks'][0]['floats'])} -> {width}" + (f"  UNPLACED {sorted(lost)}" if lost else ""))
        if write and keys:
            spec = {k: ex[k] for k in ("model", "song", "part", "span", "kind", "keywords", "title", "story") if k in ex}
            spec["keys"] = keys
            tmp = d / ".spec.json"
            tmp.write_text(json.dumps(spec))
            try:
                de.build(str(tmp))
            finally:
                tmp.unlink()


main()
