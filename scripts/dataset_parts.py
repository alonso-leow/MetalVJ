#!/usr/bin/env python3
"""Cut each song down to the parts worth authoring for the Colourist.

    python3 scripts/dataset_parts.py            # writes data/dataset/colourist/parts.json

Most of a song repeats. A bar that sounds like many other bars of the same
song is the "repetitive part"; a bar that sounds like few others is an
anomaly — an intro, a build, a drop, a break, a bridge, an outro: the places a
DJ cuts between songs and the places the show should change model. The dataset
is biased on purpose toward those places, so a model change is learnt at a
transition, not in the middle of a loop. Every part keeps a margin around its
anomaly: a pre-roll that holds the approach (the build, when there is one) and
a post-roll that holds the repetitive section the anomaly lands in, so the
Colourist also sees what "after the change" looks like.

Inputs, all read, none written:
- `data/audio/<stem>.32hz.npy`  the canonical digest (columns 0..16 = music:
  rms, bandwidth, onset, centroid, 12 chroma), 32 ticks per second;
- `data/audio/<stem>.tempo.json` the digester's beats (bpm segments are noisy,
  so one global period is taken from the median beat gap);
- `data/router/gate.json`       keywords ("genre") and the allowed models.

How many parts a song gets follows its keywords' rarity, so the dataset is
regularised by keyword rather than by song: a keyword shared by many songs
(cha cha, bachata, merengue, slow, edm…) gives 1–2 parts per song; a rare one
(samba, disco, son…) gives 2–5. All ticks in the output are canonical 32 Hz.
"""

import json
import math
import sys
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
AUDIO = ROOT / "data" / "audio"
GATE = ROOT / "data" / "router" / "gate.json"
OUT = ROOT / "data" / "dataset" / "colourist" / "parts.json"

HZ = 32
TARGET_PER_KEYWORD = 8.0  # parts a keyword should end up with, spread over its songs
DEFERRED_MODELS = {"monstruo"}  # waits on data/voice-labels
KEYWORD_FIX = {"merengeue": "merengue"}
SKIP_KEYWORDS = {"note: duplicated"}
# Models added after the gate was filled in stand in for an existing one:
# wherever a row allows the original it allows these, unless the row names
# them itself. Mirrors `metal_router::gate::EQUIVALENT`.
EQUIVALENT = {"terrario": ["parallax-cueva", "parallax-cielo", "parallax-campos", "parallax-ciudad"]}


def gate_allow(allow: dict) -> dict:
    out = dict(allow)
    for orig, eqs in EQUIVALENT.items():
        if allow.get(orig) is True:
            for e in eqs:
                out.setdefault(e, True)
    return out


def keywords_of(genre: str) -> list[str]:
    out = []
    for raw in genre.replace(";", ",").split(","):
        k = raw.strip().lower()
        k = KEYWORD_FIX.get(k, k)
        if k and k not in SKIP_KEYWORDS:
            out.append(k)
    return out


def zscore(x: np.ndarray) -> np.ndarray:
    sd = x.std(axis=0)
    sd[sd < 1e-9] = 1.0
    return (x - x.mean(axis=0)) / sd


def beat_grid(tempo: dict, n_ticks: int) -> tuple[float, float]:
    """(period in ticks, phase tick of a beat) — one global grid per song."""
    beats = np.array(tempo.get("beats", []), dtype=float)
    if len(beats) < 8:
        return 0.5 * HZ, 0.0  # 120 bpm, phase 0
    gaps = np.diff(beats)
    gaps = gaps[(gaps > 0.25) & (gaps < 1.2)]
    period = float(np.median(gaps)) if len(gaps) else 0.5
    # A grid phase that most beats agree with (circular mean of beat mod period).
    ang = 2 * math.pi * (beats % period) / period
    phase = (math.atan2(np.sin(ang).mean(), np.cos(ang).mean()) % (2 * math.pi)) / (2 * math.pi) * period
    return period * HZ, phase * HZ


def bar_features(m: np.ndarray, bar_starts: np.ndarray, bar_len: float) -> dict:
    rows = []
    rms, onset = [], []
    for s in bar_starts:
        a, b = int(s), int(min(len(m), s + bar_len))
        seg = m[a:b]
        # 16-step onset pattern: where the hits fall inside the bar.
        steps = np.array_split(seg[:, 2], 16)
        pattern = np.array([x.max() if len(x) else 0.0 for x in steps])
        pattern = pattern / (pattern.sum() + 1e-9)
        chroma = seg[:, 4:16].mean(axis=0)
        chroma = chroma / (np.linalg.norm(chroma) + 1e-9)
        timbre = np.array([seg[:, 0].mean(), seg[:, 1].mean(), seg[:, 3].mean(), seg[:, 2].mean()])
        rows.append((timbre, pattern, chroma))
        rms.append(seg[:, 0].mean())
        onset.append(seg[:, 2].mean())
    timbre = zscore(np.array([r[0] for r in rows]))
    pattern = zscore(np.array([r[1] for r in rows]))
    chroma = np.array([r[2] for r in rows])
    return {"timbre": timbre, "pattern": pattern, "chroma": chroma,
            "rms": np.array(rms), "onset": np.array(onset)}


def ssm(f: dict) -> np.ndarray:
    def cos(x):
        n = np.linalg.norm(x, axis=1, keepdims=True) + 1e-9
        y = x / n
        return y @ y.T
    return 0.45 * cos(f["timbre"]) + 0.25 * cos(f["pattern"]) + 0.30 * (f["chroma"] @ f["chroma"].T)


def novelty(s: np.ndarray, w: int = 4) -> np.ndarray:
    """Foote's checkerboard novelty over the bar SSM: boundary strength at bar i."""
    n = len(s)
    k = np.ones((2 * w, 2 * w))
    k[:w, w:] = -1
    k[w:, :w] = -1
    g = np.exp(-0.5 * ((np.arange(2 * w) - w + 0.5) / (w * 0.6)) ** 2)
    k *= np.outer(g, g)
    pad = np.pad(s, w, mode="edge")
    out = np.zeros(n)
    for i in range(n):
        out[i] = (pad[i:i + 2 * w, i:i + 2 * w] * k).sum()
    out -= out.min()
    return out / (out.max() + 1e-9)


def repetitiveness(s: np.ndarray, min_dist: int = 4) -> np.ndarray:
    """Mean of each bar's three best matches at ≥ min_dist bars away."""
    n = len(s)
    out = np.zeros(n)
    for i in range(n):
        cand = [s[i, j] for j in range(n) if abs(i - j) >= min_dist]
        out[i] = float(np.mean(sorted(cand)[-3:])) if cand else 0.0
    return out


def rank01(x: np.ndarray) -> np.ndarray:
    order = x.argsort().argsort()
    return order / max(1, len(x) - 1)


def classify(i: int, nbars: int, f: dict) -> tuple[str, dict]:
    rms, ons = f["rms"], f["onset"]
    pre = slice(max(0, i - 4), i)
    post = slice(i, min(nbars, i + 4))
    r_pre = rms[pre].mean() if i > 0 else 0.0
    r_post = rms[post].mean()
    o_pre = ons[pre].mean() if i > 0 else 0.0
    o_post = ons[post].mean()
    # Rising approach: rms slope over the 8 bars before the boundary.
    look = rms[max(0, i - 8):i]
    slope = float(np.polyfit(np.arange(len(look)), look, 1)[0]) if len(look) >= 3 else 0.0
    rel = (r_post - r_pre) / (rms.std() + 1e-9)
    chroma_shift = 1.0 - float(f["chroma"][pre].mean(axis=0) @ f["chroma"][post].mean(axis=0)
                               / (np.linalg.norm(f["chroma"][pre].mean(axis=0)) *
                                  np.linalg.norm(f["chroma"][post].mean(axis=0)) + 1e-9)) if i > 0 else 0.0
    facts = {"rms_before": round(float(r_pre), 4), "rms_after": round(float(r_post), 4),
             "onset_before": round(float(o_pre), 3), "onset_after": round(float(o_post), 3),
             "rms_step_sd": round(float(rel), 2), "build_slope": round(slope * 1000, 3),
             "chroma_shift": round(chroma_shift, 3)}
    if i <= 2:
        return "intro", facts
    if i >= nbars - 6:
        return "outro", facts
    if rel > 0.8:
        return ("build-drop" if slope > 0 else "drop"), facts
    if rel < -0.8:
        return ("bridge" if o_post < o_pre * 0.6 else "break"), facts
    if chroma_shift > 0.03:
        return "harmonic-change", facts
    return "section-change", facts


def parts_budget(kws: list[str], freq: Counter) -> int:
    if not kws:
        return 1
    want = max(TARGET_PER_KEYWORD / freq[k] for k in kws)
    return int(max(1, min(5, round(want))))


def analyse(stem: str, entry: dict, budget: int) -> dict:
    m = np.load(AUDIO / f"{stem}.32hz.npy")[:, :16].astype(float)
    tempo = json.loads((AUDIO / f"{stem}.tempo.json").read_text())
    period, phase = beat_grid(tempo, len(m))
    bar_len = 4 * period
    first = phase % bar_len
    bar_starts = np.arange(first, len(m) - bar_len * 0.5, bar_len)
    if first > bar_len * 0.25:
        bar_starts = np.concatenate([[0.0], bar_starts])
    nb = len(bar_starts)
    f = bar_features(m, bar_starts, bar_len)
    s = ssm(f)
    nov = novelty(s)
    rep = repetitiveness(s)
    anom = 1.0 - rank01(rep)
    # Energy step around each bar, in song standard deviations.
    rms = f["rms"]
    step = np.array([abs(rms[i:i + 4].mean() - rms[max(0, i - 4):i].mean()) if i > 0 else 0.0
                     for i in range(nb)]) / (rms.std() + 1e-9)
    win_anom = np.array([anom[max(0, i - 2):i + 3].mean() for i in range(nb)])
    score = nov + 0.7 * win_anom + 0.35 * np.minimum(step, 3.0)
    # The intro is always a DJ cut point: score it by how unlike the body it is.
    score[0] = max(score[0], 0.6 + 0.7 * win_anom[0])
    # The checkerboard's edge padding makes the last bars look novel just for
    # being last (and a fade-out is anomalous by construction). An outro is a
    # real mix-out point, but not the most informative one: weigh it down.
    score[max(1, nb - 6):] *= 0.6

    # Candidate boundaries: local maxima of the score (±3 bars).
    cands = [i for i in range(nb)
             if score[i] >= score[max(0, i - 3):i + 4].max() - 1e-12]
    cands.sort(key=lambda i: -score[i])

    chosen = []
    floor = np.percentile(score, 60)
    for i in cands:
        if len(chosen) >= budget:
            break
        if score[i] < floor and chosen:
            continue
        kind, facts = classify(i, nb, f)
        pre = 2 if kind == "intro" else (8 if kind == "build-drop" else 4)
        post = 8 if budget <= 3 else 6
        # The anomaly's own extent: neighbouring bars that stay unusual.
        a = i
        while a - 1 >= 0 and i - a < 3 and anom[a - 1] > 0.7:
            a -= 1
        b = i
        while b + 1 < nb and b - i < 3 and anom[b + 1] > 0.7:
            b += 1
        sa = max(0, a - pre) if kind != "intro" else 0
        sb = min(nb - 1, b + post)
        if any(not (sb + 1 < c["_bars"][0] or sa > c["_bars"][1] + 1) for c in chosen):
            continue
        start = int(bar_starts[sa])
        end = int(min(len(m) - 1, bar_starts[sb] + bar_len - 1))
        chosen.append({
            "_bars": (sa, sb),
            "kind": kind,
            "boundary_tick": int(bar_starts[i]),
            "start_tick": start,
            "end_tick": end,
            "anomaly_ticks": [int(bar_starts[a]), int(min(len(m) - 1, bar_starts[b] + bar_len - 1))],
            "seconds": [round(start / HZ, 2), round(end / HZ, 2)],
            "score": round(float(score[i]), 3),
            "novelty": round(float(nov[i]), 3),
            "repetitiveness_at_boundary": round(float(rep[i]), 3),
            "repetitiveness_song_median": round(float(np.median(rep)), 3),
            "facts": facts,
        })
    chosen.sort(key=lambda c: c["start_tick"])
    for c in chosen:
        del c["_bars"]
    return {
        "ticks": len(m),
        "seconds": round(len(m) / HZ, 1),
        "bpm": round(60.0 * HZ / period, 1),
        "beat_ticks": round(period, 3),
        "bar_ticks": round(bar_len, 3),
        "grid_phase_tick": round(first, 2),
        "tempo_confidence": tempo.get("tempo_confidence"),
        "bars": nb,
        "repetitive_share": round(float((rep > np.median(rep)).mean()), 2),
        "parts": chosen,
    }


def assign_models(songs: dict, gate: dict) -> None:
    """Spread models so every allowed model is seen in as many songs as possible.

    Greedy over parts, rarest-first: each part takes the allowed model that so
    far has the fewest songs, never repeating a model inside one song."""
    use = Counter()
    order = sorted(songs, key=lambda st: sum(gate["songs"][st]["allow"].values()))
    for stem in order:
        allowed = [m for m, ok in gate_allow(gate["songs"][stem]["allow"]).items() if ok and m not in DEFERRED_MODELS]
        taken = set()
        for p in songs[stem]["parts"]:
            pool = [m for m in allowed if m not in taken] or allowed
            if not pool:
                p["model"] = None
                continue
            pick = min(pool, key=lambda m: (use[m], m))
            p["model"] = pick
            taken.add(pick)
            use[pick] += 1


def main() -> None:
    gate = json.loads(GATE.read_text())
    entries = {st: e for st, e in gate["songs"].items() if any(e["allow"].values())}
    freq = Counter(k for e in entries.values() for k in keywords_of(e["genre"]))
    songs = {}
    for stem, e in sorted(entries.items()):
        if not (AUDIO / f"{stem}.32hz.npy").exists():
            print(f"skip (no digest): {stem}", file=sys.stderr)
            continue
        kws = keywords_of(e["genre"])
        budget = parts_budget(kws, freq)
        r = analyse(stem, e, budget)
        r["keywords"] = kws
        r["budget"] = budget
        r["monstruo_allowed"] = bool(e["allow"].get("oracle"))
        songs[stem] = r
    assign_models(songs, gate)
    for stem, r in songs.items():
        for k, p in enumerate(r["parts"], 1):
            p["id"] = f"{stem.split(' ')[0]}-p{k}"

    per_kw = Counter()
    for r in songs.values():
        for k in r["keywords"]:
            per_kw[k] += len(r["parts"])
    per_model = defaultdict(set)
    for stem, r in songs.items():
        for p in r["parts"]:
            if p.get("model"):
                per_model[p["model"]].add(stem)
    out = {
        "version": 1,
        "rate": HZ,
        "about": "Parts per song for the Colourist dataset; see scripts/dataset_parts.py and "
                 "data/dataset/colourist/README.md. Ticks are canonical 32 Hz.",
        "keyword_songs": dict(freq.most_common()),
        "keyword_parts": dict(per_kw.most_common()),
        "model_songs": {m: len(s) for m, s in sorted(per_model.items(), key=lambda x: -len(x[1]))},
        "kinds": dict(Counter(p["kind"] for r in songs.values() for p in r["parts"]).most_common()),
        "total_parts": sum(len(r["parts"]) for r in songs.values()),
        "songs": songs,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
    print(f"{out['total_parts']} parts over {len(songs)} songs -> {OUT.relative_to(ROOT)}")
    print("kinds", out["kinds"])
    print("keyword parts", out["keyword_parts"])
    print("model songs", out["model_songs"])


if __name__ == "__main__":
    main()
