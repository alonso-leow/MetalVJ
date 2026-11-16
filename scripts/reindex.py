#!/usr/bin/env python3
"""Prints the `[studio.*]` blocks `config.toml` should hold, from disk.

S.AH point 4: `[studio.*]` is a *view* of files sync already carries, per
`plan-opus.md` § O.26 — "it lists them, it does not define their content;
the file is the authority and the row is regenerated from disk
(scripts/reindex.py), never by hand." This script is the "from disk" half:
it walks `data/{graphs,timelines,...}` and `data/palettes.json` and prints
what a person (or the sync process, when it sees a diff on one of those
paths) should paste into `config.toml`'s `[studio]` table.

It never writes `config.toml` itself — that stays a deliberate, reviewable
edit. The point of running this from disk instead of trusting whatever an
assistant typed is exactly that: "an assistant cannot slip in a name that
does not exist" only holds if the list is generated, not hand-typed.

    ./scripts/reindex.py [data-root]        # defaults to ./data
"""
import json
import sys
from pathlib import Path


def list_json_stems(dir_path: Path) -> list[str]:
    if not dir_path.is_dir():
        return []
    return sorted(p.stem for p in dir_path.glob("*.json"))


def main() -> None:
    data_root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("data")

    print("[studio]")
    # These five are single files, not directories — the config value is a
    # path, present only when the file actually exists on disk.
    for key, rel in [
        ("collections", "collections.json"),
        ("sequences", "sequences.json"),
        ("roles", "agent/roles.json"),
    ]:
        if (data_root / rel).is_file():
            print(f'{key} = "{rel}"')
    for key, rel in [("graphs", "graphs/"), ("timelines", "timelines/")]:
        if (data_root / rel.rstrip("/")).is_dir():
            print(f'{key} = "{rel}"')

    palettes_path = data_root / "palettes.json"
    if palettes_path.is_file():
        try:
            palettes = json.loads(palettes_path.read_text())
        except (json.JSONDecodeError, OSError) as e:
            print(f"# palettes.json did not parse: {e}", file=sys.stderr)
            palettes = []
        print()
        print("[studio.palettes]  # regenerated from data/palettes.json — do not hand-edit")
        for entry in palettes:
            name = entry.get("name", "?")
            n_slots = len(entry.get("slots", []))
            print(f'# "{name}": {n_slots} slot(s) — c_0..c_{max(n_slots - 1, 0)}')

    graphs = list_json_stems(data_root / "graphs")
    timelines = list_json_stems(data_root / "timelines")
    if graphs or timelines:
        print()
        print(f"# found on disk: {len(graphs)} graph(s), {len(timelines)} timeline(s)")
        for g in graphs:
            print(f"#   graphs/{g}.json")
        for t in timelines:
            print(f"#   timelines/{t}.json")


if __name__ == "__main__":
    main()
