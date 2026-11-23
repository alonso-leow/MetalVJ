#!/usr/bin/env python3
"""Cut the stage songs into parts, with the Colourist's own cutter.

    python3 scripts/stage_parts.py [budget=2] [max_songs=40]

Reuses `scripts/dataset_parts.py` (`analyse`, `beat_grid`, `classify`: the same
novelty / repetitiveness / anomaly scoring and the same kinds) on the songs of
`data/dataset/stage/songs.json` that have a digest in
`~/.cache/metalvj/stage-digest/` (`<stem>.32hz.npy` and `<stem>.tempo.json`).
Songs are taken in order of their operator scene events (most first), at most
`max_songs`. Each song gets `budget` parts (no keyword rarity here: stage songs have
no gate).

Writes `data/dataset/stage/parts.json`, the colourist's schema without models:
`songs[stem] = {ticks, seconds, bpm, beat_ticks, grid_phase_tick, bars, parts[...],
scene_events, timelines}`, parts `{id, kind, start_tick, end_tick, boundary_tick,
anomaly_ticks, seconds, score, facts, ...}`. Ids are `<n>-p<k>` (n = 1-based song
order), so they are unique and safe as file names.
"""

import importlib.util
import json
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIGEST = Path.home() / ".cache" / "metalvj" / "stage-digest"
SONGS = ROOT / "data" / "dataset" / "stage" / "songs.json"
OUT = ROOT / "data" / "dataset" / "stage" / "parts.json"


def load_dataset_parts():
    spec = importlib.util.spec_from_file_location("dataset_parts", ROOT / "scripts" / "dataset_parts.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    mod.AUDIO = DIGEST
    return mod


def main() -> None:
    budget = int(sys.argv[1]) if len(sys.argv) > 1 else 2
    max_songs = int(sys.argv[2]) if len(sys.argv) > 2 else 40
    dp = load_dataset_parts()
    doc = json.loads(SONGS.read_text())
    have = []
    for rec in doc["songs"].values():
        stem = rec["stem"]
        if rec["audio"] and (DIGEST / f"{stem}.32hz.npy").exists() and (DIGEST / f"{stem}.tempo.json").exists():
            have.append(rec)
    have.sort(key=lambda r: (-len(r["scenes"]), r["stem"].lower()))
    chosen = sorted(have[:max_songs], key=lambda r: r["stem"].lower())
    songs = {}
    for n, rec in enumerate(chosen, 1):
        stem = rec["stem"]
        r = dp.analyse(stem, None, budget)
        for k, p in enumerate(r["parts"], 1):
            p["id"] = f"{n:02d}-p{k}"
        r["scene_events"] = len(rec["scenes"])
        r["timelines"] = [t["file"] for t in rec["timelines"]]
        r["budget"] = budget
        songs[stem] = r
    out = {
        "version": 1,
        "rate": dp.HZ,
        "about": "Parts per stage song (fixtures dataset); see scripts/stage_parts.py and "
                 "data/dataset/stage/README.md. Ticks are canonical 32 Hz.",
        "songs_with_digest": len(have),
        "songs_missing_digest": len(doc["songs"]) - len(have),
        "kinds": dict(Counter(p["kind"] for r in songs.values() for p in r["parts"]).most_common()),
        "total_parts": sum(len(r["parts"]) for r in songs.values()),
        "songs": songs,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{out['total_parts']} parts over {len(songs)} songs -> {OUT}")
    print("kinds", out["kinds"])


if __name__ == "__main__":
    main()
