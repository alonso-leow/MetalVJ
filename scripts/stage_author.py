#!/usr/bin/env python3
"""Author the stage floats of every stage example, by rule, from the music.

    python3 scripts/stage_author.py          # rewrites data/dataset/stage/examples/*.json

Deterministic: the same digests, parts and scenes always give the same examples. It
replaces each tick's `stage` draft (from `metal_stage::dataset::draft`) with values
decided by the part's kind, the song's beat grid, the music columns and the operator's
lighting scenes from the timelines. `approved` stays false: a person approves.

Taste (Alonso's show direction, memory `furtherwave-show-taste`), as rules:
- motifs with variations: the colour walks a short motif per bar (`MOTIF`), and the pan
  sweep flips every two bars, so the picture repeats with drift;
- sparse keys: a float is only written again when its target moves, on the beat (colour),
  on the bar (pan, slow songs' colour) or on a hit (pulses); otherwise it holds;
- colour changes on the beat, never between beats;
- slow stays slow: slow songs (< 100 BPM) keep a warm colour temperature, half pan
  amplitude and colour change only on the bar;
- teach the hook: intro/bridge/section parts open with the centre units only and bring
  the rest in over three bars;
- solos burn: a harmonic/section part with loud, low-onset bars goes full saturation;
- the escalating shake: a build-drop of an EDM song (>= 118 BPM) swings the moving heads
  on every kick, a little wider each bar, and the drop releases it;
- the operator's scenes are hints: a scene that starts inside a part biases the next two
  bars (strobe, dim, white, colour, spin, full, lift; see `scene_effect`).

Inputs, read only:
- `data/dataset/stage/examples/*.json` (the scaffold: columns, ticks, music, span, kind)
- `data/dataset/stage/parts.json` (beat grid: `beat_ticks`, `grid_phase_tick`, `bpm`)
- `data/dataset/stage/songs.json` (scene events: `song_s` seconds into the song)
- the canonical digests `~/.cache/metalvj/stage-digest/<stem>.32hz.npy`
"""

import json
import math
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
STAGE = ROOT / "data" / "dataset" / "stage"
EXAMPLES = STAGE / "examples"
PARTS = STAGE / "parts.json"
SONGS = STAGE / "songs.json"
DIGEST = Path.home() / ".cache" / "metalvj" / "stage-digest"
HZ = 32
MUSIC = 16

# Channel ranges (metal_stage::names::Channel::range); everything else is [0, 1].
RANGE = {"pan": (-270.0, 270.0), "tilt": (-135.0, 135.0), "cct": (1800.0, 10000.0)}
REST = {"cct": 5600.0, "zoom": 0.5, "focus": 0.5, "iris": 1.0, "frost": 0.0, "strobe": 0.0,
        "sat": 0.0, "hue": 0.0, "level": 0.0, "pan": 0.0, "tilt": 0.0}
# Colour motif per bar (in turns of the hue circle, added to the chroma's hue).
MOTIF = [0.0, 1 / 12, 2 / 12, 1 / 12]
MOTIF_SLOW = [0.0, 0.0, 1 / 12, 0.0]
HOOK_KINDS = {"intro", "bridge", "section-change", "harmonic-change"}


def clamp(chan: str, x: float) -> float:
    lo, hi = RANGE.get(chan, (0.0, 1.0))
    if not math.isfinite(x):
        return REST.get(chan, 0.0)
    return float(min(hi, max(lo, x)))


def pct(x: np.ndarray) -> np.ndarray:
    """Rank of each value in the song, 0..1."""
    order = np.argsort(np.argsort(x, kind="stable"), kind="stable")
    return order / max(1, len(x) - 1)


def smooth(x: np.ndarray, w: int) -> np.ndarray:
    k = np.ones(w) / w
    return np.convolve(np.pad(x, (w // 2, w - 1 - w // 2), mode="edge"), k, mode="valid")


class Song:
    """Per-tick features of one song, from its whole digest (not from the stride)."""

    def __init__(self, d: np.ndarray, beat_ticks: float, phase: float, bpm: float):
        n = len(d)
        self.n = n
        self.period = beat_ticks
        self.phase = phase
        self.bpm = bpm
        self.slow = bpm < 100
        self.edm = bpm >= 118
        rms, onset, cent = d[:, 0], d[:, 2], d[:, 3]
        self.energy = pct(smooth(rms, 16))
        self.onset_p = pct(onset)
        self.cent_p = pct(cent)
        chroma = np.stack([smooth(d[:, i], 16) for i in range(4, 16)], axis=1)
        self.hue_idx = chroma.argmax(axis=1) / 12.0
        # Hits: local maxima of the onset column in the top 15 %, classed by brightness.
        peak = (self.onset_p > 0.85) & (onset >= np.array(
            [onset[max(0, i - 4):i + 5].max() for i in range(n)]) - 1e-9)
        kind = np.where(self.cent_p >= 0.6, "stroke", np.where(self.cent_p <= 0.35, "kick", "snare"))
        self.hit = {k: np.zeros(n, bool) for k in ("kick", "snare", "stroke")}
        for i in np.flatnonzero(peak):
            self.hit[kind[i]][i] = True
        self.hit_any = {k: np.convolve(v.astype(int), np.ones(9, int), "same") > 0 for k, v in self.hit.items()}

    def beat(self, t: int) -> tuple[int, bool]:
        """(beat index on the song's grid, t is on a beat)."""
        b = round((t - self.phase) / self.period)
        return b, abs(t - (self.phase + b * self.period)) <= self.period / 4


def columns(names: list[str]) -> list[dict]:
    out = []
    for c in names:
        p = c.split(".")
        out.append({"name": c, "row": p[1], "col": p[2], "kind": p[3], "chan": p[4]})
    return out


def scene_effect(name: str) -> str:
    n = name.lower()
    if "strob" in n:
        return "strobe"
    if any(w in n for w in ("black", "cerr", "apag", "fade")):
        return "dim"
    if any(w in n for w in ("blanco", "white")):
        return "white"
    if any(w in n for w in ("multicolor", "colores", "color", "neon", "arco")):
        return "colour"
    if any(w in n for w in ("spider", "giro")):
        return "spin"
    if "general" in n:
        return "full"
    return "lift"


class Hold:
    """A channel that keeps its value until a target moves past `thr` or a change is allowed."""

    def __init__(self):
        self.v: dict[str, float] = {}

    def __call__(self, key: str, target: float, thr: float, allow: bool) -> float:
        prev = self.v.get(key)
        if prev is None or (allow and abs(target - prev) >= thr):
            prev = target
            self.v[key] = prev
        return prev


def author_example(ex: dict, song: Song, part: dict, scenes: list[tuple[int, str]]) -> dict:
    cols = columns(ex["columns"])
    s0, e0 = ex["span"]
    kind = part["kind"]
    span_len = max(1, e0 - s0)
    bar = 4 * song.period
    hold = Hold()
    colour_off = 0.0
    last_on_beat = None
    ticks = []
    for tk in ex["ticks"]:
        t = tk["tick"]
        u = (t - s0) / span_len
        b, on_beat = song.beat(t)
        bar_k = int(max(0, t - s0) // bar)
        bar_start = on_beat and b % 4 == 0
        E = float(song.energy[min(t, song.n - 1)])
        act = {"intro": 0.45, "build-drop": 0.4 + 0.6 * u, "drop": 1.0, "break": 0.3,
               "bridge": 0.4, "harmonic-change": 0.7, "section-change": 0.7,
               "outro": 0.5 * (1 - u)}.get(kind, 0.6)
        en = E * act
        hook_all = 1.0 if kind not in HOOK_KINDS else min(1.0, (bar_k + 1) / 3)
        hits = {k: bool(song.hit_any[k][min(t, song.n - 1)]) for k in song.hit_any}
        strong_kick = bool(song.hit["kick"][min(t, song.n - 1)]) or hits["kick"]
        # Scene hints active at this tick (scene starts within the last two bars).
        fx = set()
        for s_tick, name in scenes:
            if s_tick <= t < s_tick + 8 * bar:
                fx.add(scene_effect(name))
        if on_beat and "colour" in fx and last_on_beat != b:
            colour_off += 1 / 6
        last_on_beat = b if on_beat else last_on_beat
        dim = 0.3 if "dim" in fx else 1.0
        # The solo: a loud, low-onset bar of a harmonic or section part burns.
        solo = kind in ("harmonic-change", "section-change") and E > 0.7 and song.onset_p[min(t, song.n - 1)] < 0.5
        escalate = kind == "build-drop" and song.edm
        drop_hot = kind == "drop" and song.edm and bar_k < 2
        motif = MOTIF_SLOW if song.slow else MOTIF
        hue_root = float(song.hue_idx[min(t, song.n - 1)])
        pan_amp = {"intro": 0.0, "break": 50.0, "bridge": 40.0, "build-drop": 90.0 + 40.0 * u,
                   "drop": 150.0, "outro": 30.0}.get(kind, 80.0 + 40.0 * E)
        if song.slow:
            pan_amp *= 0.5
        if "spin" in fx:
            pan_amp = 150.0
        pan_sign = 1.0 if (bar_k // 2) % 2 == 0 else -1.0

        unit = {}
        for c in cols:
            key = (c["row"], c["col"], c["kind"])
            if key in unit:
                continue
            centre = c["col"] == "c"
            hook = 1.0 if centre else hook_all
            lvl_kind = {"backlight": 0.85, "moving": 0.9, "wash": 1.0}[c["kind"]]
            if c["kind"] == "backlight":
                lvl = hold(f"{key}:level", (0.06 + 0.7 * en * lvl_kind) * hook * dim, 0.12, on_beat or bar_start)
                if strong_kick:
                    lvl = min(1.0, lvl + 0.25) * dim
            elif c["kind"] == "wash":
                lvl = hold(f"{key}:level", (0.1 + 0.6 * en) * hook * dim, 0.12, bar_start)
                if hits["snare"]:
                    lvl = min(1.0, lvl + 0.3)
            else:
                lvl = hold(f"{key}:level", (0.2 + 0.7 * en) * hook * dim, 0.12, bar_start)
            if "full" in fx:
                lvl = max(lvl, 0.9)
            if "lift" in fx:
                lvl = min(1.0, lvl + 0.15)
            if solo and c["kind"] == "moving":
                lvl = min(1.0, lvl + 0.1)
            # Colour: the chroma's hue on the beat, walking the motif per bar, with the
            # colour step of a `colour` hint; mirrored offsets keep the sides distinct.
            side = {"l": 1 / 12, "c": 0.0, "r": -1 / 12}[c["col"]] if c["kind"] == "moving" else 0.0
            hue_t = (hue_root + motif[bar_k % len(motif)] + side + colour_off) % 1.0
            hue = hold(f"{key}:hue", hue_t, 1e-6, on_beat and (bar_start or not song.slow))
            sat_t = 0.0 if "white" in fx else (1.0 if solo else (0.6 if kind == "break" else 0.9))
            sat = hold(f"{key}:sat", sat_t, 1e-6, bar_start)
            cct = hold(f"{key}:cct", 3800.0 if song.slow else 5600.0, 1.0, bar_start)
            vals = {"level": lvl, "hue": hue, "sat": sat, "cct": cct}
            if c["kind"] == "moving":
                vals["strobe"] = 0.6 if ((escalate or drop_hot or "strobe" in fx) and (hits["kick"] or hits["stroke"])) else 0.0
                base_tilt = 25.0 if c["row"] == "back" else 40.0
                tilt = base_tilt
                if escalate and strong_kick:
                    tilt = (1 if (bar_k + b) % 2 == 0 else -1) * min(90.0, 15.0 + 15.0 * bar_k)
                vals["tilt"] = tilt
                vals["pan"] = hold(f"{key}:pan", (1.0 if c["col"] == "l" else -1.0) * pan_sign * pan_amp,
                                   1.0, bar_start)
                vals["zoom"], vals["focus"], vals["iris"], vals["frost"] = 0.5, 0.5, 1.0, 0.0
            unit[key] = vals
        out = []
        for c in cols:
            vals = unit[(c["row"], c["col"], c["kind"])]
            chan = c["chan"]
            x = vals.get(chan, REST.get(chan, 0.0))
            out.append(clamp(chan, x))
        ticks.append({"tick": t, "music": tk["music"], "stage": [round(x, 4) for x in out]})
    ex = dict(ex)
    ex["ticks"] = ticks
    ex["approved"] = False
    return ex


def write_example(path: Path, ex: dict) -> None:
    """Header as indented JSON, one tick per line (the file stays readable and small)."""
    head = {k: v for k, v in ex.items() if k != "ticks"}
    lines = ["{"]
    for k, v in head.items():
        lines.append(f"  {json.dumps(k)}: {json.dumps(v, ensure_ascii=False)},")
    lines.append('  "ticks": [')
    lines.append(",\n".join("    " + json.dumps(t, separators=(",", ":")) for t in ex["ticks"]))
    lines.append("  ]")
    lines.append("}")
    path.write_text("\n".join(lines) + "\n")


def main() -> None:
    parts = json.loads(PARTS.read_text())
    songs = json.loads(SONGS.read_text())["songs"]
    by_stem = {}
    for s in songs.values():
        by_stem[s["stem"]] = s
    stats = {"examples": 0, "songs": 0, "skipped": 0}
    cache: dict[str, Song] = {}
    for path in sorted(EXAMPLES.glob("*.json")):
        ex = json.loads(path.read_text())
        stem = ex["song"]
        song_rec = parts["songs"].get(stem)
        if song_rec is None:
            stats["skipped"] += 1
            continue
        if stem not in cache:
            d = np.load(DIGEST / f"{stem}.32hz.npy").astype(float)
            cache[stem] = Song(d, song_rec["beat_ticks"], song_rec["grid_phase_tick"], song_rec["bpm"])
            stats["songs"] += 1
        meta = next(p for p in song_rec["parts"] if p["id"] == ex["part"])
        sc = []
        for ev in by_stem.get(stem, {}).get("scenes", []):
            tick = int(round(ev["song_s"] * HZ))
            if ex["span"][0] <= tick <= ex["span"][1]:
                sc.append((tick, ev["scene"]))
        sc.sort()
        out = author_example(ex, cache[stem], meta, sc)
        write_example(path, out)
        stats["examples"] += 1
    print(json.dumps(stats), file=sys.stderr)


if __name__ == "__main__":
    main()
