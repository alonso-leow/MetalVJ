#!/usr/bin/env python3
"""Songs and lighting scenes from the desk timelines (.tml).

    python3 scripts/stage_timelines.py            # writes data/dataset/stage/songs.json

Every timeline is an INI-like file: `[Params]` names the tracks (`TimeLine_<n>`),
and each `[Event_<k>]` sits on one track (`TimeLineIndex = n`) with a `StartTime`
(`h:mm:ss.f`), a `Path` and, for audio, a `Length`. The audio track ("TimeLine
Audio") names the song files (Windows paths, `X:/...`); the lighting tracks
("TimeLine Escena de iluminación #n") name the operator's scenes (`.sce`), or `OFF`.
The scenes are evidence of what a human operator did with a song: which scene came
in on which second.

Inputs, read only:
- `/run/media/alonso-leow/Alonso/timelines/**/*.tml` (UTF-8 BOM, UTF-16 or cp1252);
- the external drive, walked once for audio files, matched by basename
  (case-insensitive). A Windows path that names a folder which exists on the drive
  wins over a same-named file elsewhere; `AUDIO-SOURCE` is preferred next.

Output, `data/dataset/stage/songs.json`: one entry per song (keyed by the audio file
stem), the timelines that use it with their audio event offsets, and every scene
event with its time on the timeline clock and in the song (`song_s` = timeline time
minus the audio event's start). A song whose file is not on the drive keeps
`"audio": null`.
"""

import json
import os
import re
import sys
import unicodedata
from collections import defaultdict, deque
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent
DRIVE = Path("/run/media/alonso-leow/Alonso")
TIMELINES = DRIVE / "timelines"
OUT = ROOT / "data" / "dataset" / "stage" / "songs.json"
INDEX_CACHE = Path.home() / ".cache" / "metalvj" / "stage-audio-index.json"

AUDIO_EXT = {".mp3", ".wav", ".flac", ".ogg", ".m4a", ".aif", ".aiff", ".wma"}
SKIP_DIRS = {"$RECYCLE.BIN", "System Volume Information"}
NO_FILE = {"", "OFF", "STOP"}


def read_text(path: Path) -> str:
    raw = path.read_bytes()
    if raw.startswith(b"\xef\xbb\xbf"):
        return raw[3:].decode("utf-8", errors="replace")
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        return raw.decode("utf-16", errors="replace")
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return raw.decode("cp1252", errors="replace")


def parse_sections(text: str) -> list[tuple[str, dict[str, str]]]:
    out: list[tuple[str, dict[str, str]]] = []
    cur: dict[str, str] | None = None
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith(";"):
            continue
        m = re.fullmatch(r"\[(.+)\]", line)
        if m:
            cur = {}
            out.append((m.group(1), cur))
            continue
        if cur is not None and "=" in line:
            k, v = line.split("=", 1)
            cur[k.strip()] = v.strip()
    return out


def seconds(text: str | None) -> float | None:
    """`h:mm:ss.f` (or `m:ss.f`) to seconds."""
    if not text:
        return None
    try:
        parts = [float(p) for p in text.split(":")]
    except ValueError:
        return None
    total = 0.0
    for p in parts:
        total = total * 60 + p
    return round(total, 3)


def win_path(text: str) -> PurePosixPath:
    return PurePosixPath(text.replace("\\", "/"))


def scene_name(path: str) -> str:
    base = win_path(path).name
    return re.sub(r"\.scex?$", "", base, flags=re.I)


def scan_timelines() -> list[dict]:
    """One record per timeline: its audio events and its lighting scene events."""
    records = []
    for f in sorted(TIMELINES.rglob("*.tml")):
        secs = parse_sections(read_text(f))
        params = next((kv for name, kv in secs if name == "Params"), {})
        audio_tracks, light_tracks = set(), {}
        for key, value in params.items():
            m = re.fullmatch(r"TimeLine_(\d+)", key)
            if not m:
                continue
            low = value.lower()
            if "audio" in low:
                audio_tracks.add(m.group(1))
            elif "iluminaci" in low or "escena" in low:
                light_tracks[m.group(1)] = value
        audio, scenes, off = [], [], 0
        for name, kv in secs:
            if not name.startswith("Event_"):
                continue
            track = kv.get("TimeLineIndex", "")
            path = kv.get("Path", "")
            start = seconds(kv.get("StartTime"))
            if start is None:
                continue
            # Some timelines put scenes on the audio track (their Params order is
            # not the usual one), so the file kind decides, not only the track.
            if path in NO_FILE:
                off += 1
                continue
            ext = PurePosixPath(path.replace("\\", "/")).suffix.lower()
            if ext in AUDIO_EXT and track in audio_tracks:
                audio.append({"path": path, "start_s": start, "length_s": seconds(kv.get("Length"))})
            elif ext in (".sce", ".scex") or track in light_tracks:
                scenes.append({
                    "scene": scene_name(path),
                    "scene_path": path.replace("\\", "/"),
                    "start_s": start,
                    "track": light_tracks.get(track, f"TimeLine {track}"),
                })
        records.append({
            "file": str(f.relative_to(DRIVE)),
            "audio": audio,
            "scenes": scenes,
            "scene_off_events": off,
        })
    return records


def index_audio(wanted: set[str]) -> dict[str, list[str]]:
    """basename.lower() -> audio files on the drive with that name, for `wanted` only.

    The external drive is slow to walk, so the search stops as soon as every wanted
    name has a hit. The timelines folder (AUDIO-SOURCE lives there) is walked first,
    then the rest of the drive, one top-level folder after another.
    """
    if INDEX_CACHE.exists():
        cached = json.loads(INDEX_CACHE.read_text())
        if wanted <= set(cached.get("_wanted", [])):
            return cached["index"]
    index: dict[str, list[str]] = defaultdict(list)
    pending = {w.lower() for w in wanted}
    queue = deque([TIMELINES, *sorted(p for p in DRIVE.iterdir() if p.is_dir() and p != TIMELINES)])
    while queue and pending:
        d = queue.popleft()
        try:
            entries = list(os.scandir(d))
        except OSError:
            continue
        for e in entries:
            try:
                if e.is_dir(follow_symlinks=False):
                    if e.name not in SKIP_DIRS and not e.name.startswith("."):
                        queue.append(Path(e.path))
                elif e.name.lower() in pending:
                    index[e.name.lower()].append(e.path)
                    pending.discard(e.name.lower())
                elif Path(e.name).suffix.lower() in AUDIO_EXT:
                    index[e.name.lower()].append(e.path)
            except OSError:
                continue
    out = {k: sorted(v) for k, v in index.items()}
    INDEX_CACHE.parent.mkdir(parents=True, exist_ok=True)
    INDEX_CACHE.write_text(json.dumps({"_wanted": sorted(wanted), "index": out}, ensure_ascii=False))
    return out


def fold(name: str) -> str:
    """Letters and digits only, lowercase, no accents, no extension."""
    s = unicodedata.normalize("NFKD", name).encode("ascii", "ignore").decode().lower()
    s = re.sub(r"\.(mp3|wav|m4a|flac|ogg|aiff?|wma)$", "", s)
    return re.sub(r"[^a-z0-9]", "", s)


def near(ref: str, index: dict[str, list[str]]) -> list[str]:
    """Drive files whose folded name contains the reference's (or the reverse), for review.

    Never used as a match: a different recording can carry a similar title.
    """
    want = fold(win_path(ref).name)
    if len(want) < 8:
        return []
    return sorted(c for k, cs in index.items() if len(fold(k)) >= 8 and (want in fold(k) or fold(k) in want)
                  for c in cs)


def resolve(ref: str, index: dict[str, list[str]]) -> tuple[str | None, str]:
    """The real file for a Windows path, and how it was matched."""
    wp = win_path(ref)
    cands = index.get(wp.name.lower(), [])
    if not cands:
        return None, "missing"
    if len(cands) == 1:
        return cands[0], "basename"
    # Several files share the name: the longest run of the path's folders that the
    # candidate's own folders end with wins; AUDIO-SOURCE breaks ties.
    want = [p.lower() for p in wp.parent.parts if p not in ("/", "X:", "")]

    def score(c: str) -> tuple[int, int]:
        have = [p.lower() for p in Path(c).parent.parts]
        run = 0
        for a, b in zip(reversed(want), reversed(have)):
            if a != b:
                break
            run += 1
        return (run, "audio-source" in c.lower())

    best = max(cands, key=lambda c: (score(c), c))
    return best, "folder" if score(best)[0] else "basename-ambiguous"


def build() -> dict:
    records = scan_timelines()
    wanted = {win_path(ev["path"].replace("\\", "/")).name for r in records for ev in r["audio"]}
    index = index_audio(wanted)
    songs: dict[str, dict] = {}
    audio_refs = 0
    for rec in records:
        # A song plays from its StartTime to its Length, or to the next audio event on
        # the same timeline when the Length is not written.
        starts = sorted(ev["start_s"] for ev in rec["audio"])
        for ev in rec["audio"]:
            audio_refs += 1
            if ev["length_s"]:
                ev["end_s"] = ev["start_s"] + ev["length_s"]
            else:
                later = [x for x in starts if x > ev["start_s"]]
                ev["end_s"] = later[0] if later else float("inf")
            ref = ev["path"].replace("\\", "/")
            real, how = resolve(ref, index)
            stem = Path(real or ref.split("/")[-1]).stem if real else win_path(ref).stem
            key = real or f"missing:{win_path(ref).name.lower()}"
            s = songs.setdefault(key, {
                "stem": stem,
                "audio": real,
                "match": how,
                "near_on_drive": [] if real else near(ref, index),
                "referenced": [],
                "timelines": [],
                "scenes": [],
            })
            if ref not in s["referenced"]:
                s["referenced"].append(ref)
            tl = next((t for t in s["timelines"] if t["file"] == rec["file"]), None)
            if tl is None:
                tl = {"file": rec["file"], "audio_starts_s": [], "audio_lengths_s": []}
                s["timelines"].append(tl)
            tl["audio_starts_s"].append(ev["start_s"])
            tl["audio_lengths_s"].append(ev["length_s"])
            # Scenes of this timeline are a song's scenes while its audio plays.
            for sc in rec["scenes"]:
                if ev["start_s"] <= sc["start_s"] < ev["end_s"] and not any(
                    x["timeline"] == rec["file"] and x["start_s"] == sc["start_s"] and x["scene"] == sc["scene"]
                    for x in s["scenes"]
                ):
                    s["scenes"].append({
                        "timeline": rec["file"],
                        "track": sc["track"],
                        "scene": sc["scene"],
                        "start_s": sc["start_s"],
                        "song_s": round(sc["start_s"] - ev["start_s"], 3),
                    })
    for s in songs.values():
        s["scenes"].sort(key=lambda x: (x["timeline"], x["song_s"]))
        for tl in s["timelines"]:
            tl["audio_starts_s"] = sorted(set(tl["audio_starts_s"]))
    # Stems can collide (the same name from two folders, or a missing name reused).
    seen: dict[str, int] = defaultdict(int)
    for s in sorted(songs.values(), key=lambda x: x["stem"].lower()):
        seen[s["stem"].lower()] += 1
        if seen[s["stem"].lower()] > 1:
            s["stem"] = f"{s['stem']} ({seen[s['stem'].lower()]})"
    ordered = dict(sorted(songs.items(), key=lambda kv: kv[1]["stem"].lower()))
    found = [s for s in ordered.values() if s["audio"]]
    return {
        "version": 1,
        "about": "Songs and operator lighting scenes from the desk timelines. "
                 "Written by scripts/stage_timelines.py; see data/dataset/stage/README.md.",
        "timelines": len(records),
        "timelines_with_audio": sum(1 for r in records if r["audio"]),
        "timelines_with_scenes": sum(1 for r in records if r["scenes"]),
        "scene_events": sum(len(r["scenes"]) for r in records),
        "audio_events": audio_refs,
        "songs_total": len(ordered),
        "songs_found": len(found),
        "songs_missing": len(ordered) - len(found),
        "matches": {m: sum(1 for s in ordered.values() if s["match"] == m) for m in
                    sorted({s["match"] for s in ordered.values()})},
        "songs": ordered,
    }


def main() -> None:
    doc = build()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n")
    print(f"{doc['timelines']} timelines ({doc['timelines_with_audio']} with audio, "
          f"{doc['timelines_with_scenes']} with scenes); {doc['scene_events']} scene events")
    print(f"{doc['songs_total']} songs: {doc['songs_found']} found, {doc['songs_missing']} missing; "
          f"matches {doc['matches']}")
    print(f"-> {OUT.relative_to(ROOT)}", file=sys.stderr)


if __name__ == "__main__":
    main()
