#!/usr/bin/env python3
"""Beat-by-beat numbers of one dataset part, for authoring its keyframes.

    python3 scripts/dataset_part_report.py 67-p2

Reads `data/dataset/colourist/parts.json` and the song's canonical digest
(`data/audio/<stem>.32hz.npy`, 32 ticks/s) and tempo map, and prints:

1. the part: span, kind, the anomaly inside it, the analysis facts;
2. one line per beat of the span (on the song's global beat grid): the tick,
   seconds, bar.beat, RMS mean, onset max and the tick of that max, centroid,
   bandwidth, the dominant pitch class, and flags —
   `PEAK` (onset in the song's top 5 %), `STEP+`/`STEP-` (RMS of this beat vs
   the 4 beats before, ≥ 1 song s.d.), `KEY` (chroma turned, cosine < 0.85),
   `ANOM` (inside the anomaly);
3. the strongest onsets in the span, ranked, with their exact ticks.

Every keyframe tick should come from this report: an onset peak, a downbeat, an
RMS step, a chroma turn. Nothing here is written.
"""

import json
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
PARTS = ROOT / "data" / "dataset" / "colourist" / "parts.json"
AUDIO = ROOT / "data" / "audio"
HZ = 32
PITCH = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]


def main(part_id: str) -> None:
    d = json.loads(PARTS.read_text())
    for stem, song in d["songs"].items():
        for p in song["parts"]:
            if p["id"] == part_id:
                return report(stem, song, p)
    raise SystemExit(f"no part {part_id!r} in {PARTS}")


def report(stem: str, song: dict, p: dict) -> None:
    m = np.load(AUDIO / f"{stem}.32hz.npy")[:, :16].astype(float)
    rms, bw, ons, cen, chroma = m[:, 0], m[:, 1], m[:, 2], m[:, 3], m[:, 4:16]
    beat = song["beat_ticks"]
    phase = song["grid_phase_tick"]
    a, b = p["start_tick"], p["end_tick"]
    an = p["anomaly_ticks"]
    peak_thr = np.percentile(ons, 95)
    rms_sd = rms.std()

    print(f"{p['id']}  {stem}")
    print(f"  keywords {song['keywords']}   model {p.get('model')}   kind {p['kind']}")
    print(f"  bpm {song['bpm']}  beat {beat:.2f} ticks  bar {song['bar_ticks']:.2f} ticks  "
          f"tempo confidence {song['tempo_confidence']}")
    print(f"  span {a}..{b} ticks = {a / HZ:.2f}s..{b / HZ:.2f}s ({(b - a) / HZ:.1f}s)   "
          f"boundary {p['boundary_tick']} ({p['boundary_tick'] / HZ:.2f}s)   anomaly {an[0]}..{an[1]}")
    print(f"  facts {p['facts']}")
    print(f"  song: rms median {np.median(rms):.3f} p95 {np.percentile(rms, 95):.3f} | "
          f"onset median {np.median(ons):.2f} p95 {peak_thr:.2f} | "
          f"centroid median {np.median(cen):.3f}")
    print()
    print("  (bar.bt is the song's global beat grid, not a confirmed downbeat)")
    print("  tick   sec     bar.bt  rms    onsetmax@tick   centroid bandwidth pitch  flags")
    first = phase + np.ceil((a - phase) / beat) * beat
    beats = np.arange(first, b, beat)
    prev_ch = None
    rms_hist = []
    for n, t0 in enumerate(beats):
        s, e = int(round(t0)), int(round(min(b + 1, t0 + beat)))
        if e <= s:
            continue
        seg = slice(s, e)
        r = rms[seg].mean()
        k = s + int(ons[seg].argmax())
        ch = chroma[seg].mean(axis=0)
        chn = ch / (np.linalg.norm(ch) + 1e-9)
        flags = []
        if ons[k] >= peak_thr:
            flags.append("PEAK")
        if len(rms_hist) >= 4:
            step = (r - np.mean(rms_hist[-4:])) / rms_sd
            if step >= 1:
                flags.append("STEP+")
            elif step <= -1:
                flags.append("STEP-")
        if prev_ch is not None and float(chn @ prev_ch) < 0.85:
            flags.append("KEY")
        if an[0] <= s <= an[1]:
            flags.append("ANOM")
        idx = int(round((t0 - phase) / beat))
        bar_no, bt = idx // 4 + 1, idx % 4 + 1
        print(f"  {s:5d} {s / HZ:7.2f}  {bar_no:3d}.{bt}  {r:.3f}  {ons[k]:6.2f}@{k:<6d}  "
              f"{cen[seg].mean():.3f}    {bw[seg].mean():6.0f}   {PITCH[int(ch.argmax())]:<5} {' '.join(flags)}")
        prev_ch = chn
        rms_hist.append(r)
    print()
    seg = ons[a:b + 1]
    top = np.argsort(seg)[::-1]
    picked = []
    for i in top:
        if all(abs(i - j) > HZ // 4 for j in picked):
            picked.append(i)
        if len(picked) == 12:
            break
    print("  strongest onsets (tick, sec, strength):")
    for i in sorted(picked, key=lambda i: -seg[i]):
        print(f"    {a + i:6d}  {(a + i) / HZ:7.2f}s  {seg[i]:.2f}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    main(sys.argv[1])
