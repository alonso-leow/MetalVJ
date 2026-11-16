#!/usr/bin/env python3
"""Turn a sparse authoring spec into a Colourist example file, and describe models.

    python3 scripts/dataset_example.py schema <model>        # the model's row, named
    python3 scripts/dataset_example.py build <spec.json>     # -> examples/<part>.json

An example (`metal-dash::author::models::example::Example`) stores the model's
**whole row** at every keyframe. Authoring that way would force every float to
be keyed at every keyframe, which is exactly what the plan forbids ("a float
left out of a keyframe keeps its interpolated path"). So the spec is written
per float, sparsely, and this script resolves it:

- each float has its own track: the ticks where the spec names it;
- at a keyframe that does not name a float, the float takes the value of its
  own track there — smoothstep between its two neighbouring keys (the same
  ease `dense::interpolate` uses), held before its first and after its last;
- a float never named anywhere stays at the model's schema default;
- colour slots are named `slot:<name>` (`"slot:background": [12 numbers]`; rings
  has both a float and a slot called `ball`), the camera by
  `"camera": [...]`, and are resolved the same way.

Spec (all ticks canonical 32 Hz, absolute song ticks):

    {
      "part": "67-p2", "model": "rings", "song": "<stem>", "span": [864, 1920],
      "kind": "build-drop", "keywords": ["samba", ...],
      "base": {"layers": 7, "slot:rings": [ ...12 ]},     # optional: row at span start
      "keys": [
        {"tick": 864,  "set": {"speed": 0.6}, "why": "first downbeat of the build"},
        {"tick": 1360, "set": {"speed": 1.8, "shake": 0.4}, "why": "drop, onset 18.2"}
      ]
    }

A float that appears twice in a model's schema (fractals, vaporgrid, moire) is
addressed as `name#2` for its second occurrence.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCHEMA = ROOT / "data" / "schema.json"
EXAMPLES = ROOT / "data" / "dataset" / "colourist" / "examples"
HZ = 32


def model_schema(model: str) -> dict:
    s = json.loads(SCHEMA.read_text())
    for m in s["models"]:
        if m["id"] == model:
            return m
    raise SystemExit(f"no model {model!r} in data/schema.json")


def layout(m: dict) -> list[dict]:
    """One entry per addressable item, in row order: {key, offset, width, default, min, max, drive}."""
    out, at, seen = [], 0, {}
    for b in m["blocks"]:
        if b["type"] == "floats":
            for f in b["fields"]:
                seen[f["name"]] = seen.get(f["name"], 0) + 1
                key = f["name"] if seen[f["name"]] == 1 else f"{f['name']}#{seen[f['name']]}"
                out.append({"key": key, "offset": at, "width": 1, "default": [f["default"]],
                            "min": f["min"], "max": f["max"], "drive": f["drive"]})
                at += 1
        elif b["type"] == "color_slot":
            out.append({"key": f"slot:{b['name']}", "offset": at, "width": 12, "default": list(b["default"]),
                        "min": 0.0, "max": 1.0, "drive": None})
            at += 12
        elif b["type"] == "camera":
            out.append({"key": "camera", "offset": at, "width": len(b["default"]),
                        "default": list(b["default"]), "min": None, "max": None, "drive": None})
            at += len(b["default"])
    return out


def ease(t: float) -> float:
    return t * t * (3.0 - 2.0 * t)


def track_value(track: list[tuple[int, list[float]]], tick: int) -> list[float]:
    if tick <= track[0][0]:
        return track[0][1]
    for (ta, va), (tb, vb) in zip(track, track[1:]):
        if ta <= tick <= tb:
            f = ease((tick - ta) / (tb - ta)) if tb > ta else 1.0
            return [a + (b - a) * f for a, b in zip(va, vb)]
    return track[-1][1]


def build(spec_path: str) -> None:
    spec = json.loads(Path(spec_path).read_text())
    m = model_schema(spec["model"])
    items = {it["key"]: it for it in layout(m)}
    width = sum(it["width"] for it in items.values())
    keys = sorted(spec["keys"], key=lambda k: k["tick"])
    if not keys:
        raise SystemExit("spec has no keys")
    span = spec.get("span") or [keys[0]["tick"], keys[-1]["tick"]]

    def as_list(item, v):
        v = v if isinstance(v, list) else [v]
        if len(v) != item["width"]:
            raise SystemExit(f"{item['key']}: {len(v)} values, takes {item['width']}")
        if item["width"] == 1 and item["min"] is not None and not (item["min"] - 1e-6 <= v[0] <= item["max"] + 1e-6):
            raise SystemExit(f"{item['key']} = {v[0]} is outside [{item['min']}, {item['max']}]")
        return [float(x) for x in v]

    tracks: dict[str, list[tuple[int, list[float]]]] = {}
    for name, v in (spec.get("base") or {}).items():
        if name not in items:
            raise SystemExit(f"base: {spec['model']} has no {name!r}")
        tracks.setdefault(name, []).append((span[0], as_list(items[name], v)))
    for k in keys:
        for name, v in k.get("set", {}).items():
            if name not in items:
                raise SystemExit(f"tick {k['tick']}: {spec['model']} has no {name!r}")
            t = tracks.setdefault(name, [])
            t[:] = [p for p in t if p[0] != k["tick"]]
            t.append((k["tick"], as_list(items[name], v)))
    for t in tracks.values():
        t.sort(key=lambda p: p[0])

    ticks = []
    for k in keys:
        row = [0.0] * width
        for it in items.values():
            v = track_value(tracks[it["key"]], k["tick"]) if it["key"] in tracks else it["default"]
            row[it["offset"]:it["offset"] + it["width"]] = v
        ticks.append({"tick": int(k["tick"]), "floats": [round(x, 5) for x in row]})

    # What the Colourist can reproduce: `grade::offset_of` clamps every float to
    # default ± SWING·drive·range (SWING = 0.6). Outside that band the take is
    # clipped in training, so say so instead of letting it pass silently.
    for it in items.values():
        if it["width"] != 1 or it["key"] not in tracks:
            continue
        reach = 0.6 * (it["drive"] or 0.0) * (it["max"] - it["min"])
        lo, hi = it["default"][0] - reach, it["default"][0] + reach
        for tick, v in tracks[it["key"]]:
            if not (lo - 1e-6 <= v[0] <= hi + 1e-6):
                print(f"warning: {it['key']} = {v[0]:.4g} at tick {tick} is outside what the Colourist "
                      f"can reach ([{max(lo, it['min']):.4g}, {min(hi, it['max']):.4g}]); training clips it",
                      file=sys.stderr)

    if len({t["tick"] for t in ticks}) != len(ticks):
        raise SystemExit("two keys share a tick")
    out = {
        "model": spec["model"],
        "song": spec["song"],
        "rate": float(HZ),
        "ticks": ticks,
        # Everything below is the dataset's own: `Example` ignores it.
        "part": spec["part"],
        "span": span,
        "kind": spec.get("kind"),
        "keywords": spec.get("keywords", []),
        "title": spec.get("title"),
        "story": spec.get("story"),
        "keys": [{"tick": k["tick"], "set": k.get("set", {}), "why": k.get("why")} for k in keys],
    }
    EXAMPLES.mkdir(parents=True, exist_ok=True)
    dest = EXAMPLES / f"{spec['part']}.json"
    dest.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{dest.relative_to(ROOT)}: {len(ticks)} keyframes, width {width}, "
          f"span {span[0]}..{span[1]} ({span[0] / HZ:.1f}s..{span[1] / HZ:.1f}s)")


def show_schema(model: str) -> None:
    m = model_schema(model)
    print(f"{model} — {m.get('label')} — technique: {m.get('technique')}")
    for it in layout(m):
        if it["width"] == 1:
            reach = 0.6 * it["drive"] * (it["max"] - it["min"])
            lo, hi = max(it["min"], it["default"][0] - reach), min(it["max"], it["default"][0] + reach)
            print(f"  [{it['offset']:3}] {it['key']:<16} default {it['default'][0]:<9.4g} "
                  f"range [{it['min']:.4g}, {it['max']:.4g}]  drive {it['drive']:.2f}  "
                  f"learnable [{lo:.4g}, {hi:.4g}]")
        else:
            print(f"  [{it['offset']:3}] {it['key']:<16} ({it['width']} values) default "
                  f"{[round(x, 3) for x in it['default']]}")
    print(f"  width {sum(it['width'] for it in layout(m))}")


if __name__ == "__main__":
    if len(sys.argv) != 3 or sys.argv[1] not in ("schema", "build"):
        raise SystemExit(__doc__)
    (show_schema if sys.argv[1] == "schema" else build)(sys.argv[2])
