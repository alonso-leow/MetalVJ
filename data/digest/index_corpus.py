#!/usr/bin/env python3
"""Indexes the corpus. There is nothing to generate anymore.

Replaces `generate_dataset.py`, deleted 2026-08-13. That script wrote a
combinatorial manifest: 21 rhythm styles crossed with 3 loop BPMs crossed
with 3 difficulty targets crossed with 3 takes, plus 40 drum rudiments, plus
60 songs. 667 rows describing 60 distinct pieces of audio.

The whole cross-product was fiction. The model reads one thing — the audio
matrix (MODEL.md) — so an axis that does not change the audio does not
produce a training example, it produces a **collision**: another target for
an input that already had one. Difficulty changed no audio at all. Neither
did the take index. salsa-on-1 and salsa-on-2 shared their loops outright.

And the classification was never doing anything either. A genre label is a
fact about the music that nothing in the pipeline reads: not the digester,
not the model, not the resolver. It survived four rewrites as decoration.

So the corpus is **hundreds of matrices and nothing else**. A song, digested;
a motion track, authored against it. No style, no BPM, no difficulty, no
rudiments, no synthesized loops, no take index, no formations. If a particular
kind of music needs covering, that is answered by putting that music in the
folder — not by a column.

    data/audio/<anything>.{mp3,wav,flac,m4a,ogg,opus}   what you downloaded
    data/audio/<same>.8hz.npy        (T, 16)   digest_audio.py
    data/tracks/<same>.track.json     (T, 843)  koreo-core::frame::Track

This script walks those three and reports what is paired, what is digested
and waiting to be authored, and what has not been digested at all. That is
the entire manifest: a directory listing with the work left in it.

Run:
    python3 data/digest/index_corpus.py            # report + write the index
    python3 data/digest/index_corpus.py --digest   # also digest what is missing

Writes: data/digest/corpus.csv
"""
import argparse
import csv
import pathlib

AUDIO_DIR = pathlib.Path("data/audio")
TRACK_DIR = pathlib.Path("data/tracks")
INDEX = pathlib.Path("data/digest/corpus.csv")

# Anything librosa will open. The list is permissive on purpose: what the
# corpus contains is whatever got put in the folder.
SOUND = {".mp3", ".wav", ".flac", ".m4a", ".ogg", ".opus", ".aiff"}

FIELDS = ["id", "audio", "matrix", "track", "ticks", "seconds", "status"]


def ticks(matrix_path):
    """Rows in a digested matrix, without importing numpy for a header read.

    A .npy header is ASCII inside the first 128 bytes and carries the shape,
    which is all this needs — the index should be readable on a machine that
    has neither numpy nor librosa installed, since digestion happens
    elsewhere and this is a listing.
    """
    try:
        head = matrix_path.open("rb").read(256)
    except OSError:
        return None
    marker = head.find(b"'shape': (")
    if marker < 0:
        return None
    try:
        return int(head[marker + 10 :].split(b",")[0])
    except (ValueError, IndexError):
        return None


def scan():
    for audio in sorted(AUDIO_DIR.glob("*")) if AUDIO_DIR.is_dir() else []:
        if audio.suffix.lower() not in SOUND:
            continue
        name = audio.stem
        matrix = AUDIO_DIR / f"{name}.8hz.npy"
        track = TRACK_DIR / f"{name}.track.json"
        n = ticks(matrix) if matrix.exists() else None
        if not matrix.exists():
            status = "undigested"
        elif not track.exists():
            status = "digested, unauthored"
        else:
            status = "paired"
        yield {
            "id": name,
            "audio": str(audio),
            "matrix": str(matrix) if matrix.exists() else "",
            "track": str(track) if track.exists() else "",
            "ticks": n if n is not None else "",
            # 8 Hz, so ticks over eight. The one derived number here, and it
            # is derived because minutes-of-corpus is the only size that
            # means anything now that there are no rows to count.
            "seconds": f"{n / 8:.1f}" if n else "",
            "status": status,
        }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--digest", action="store_true", help="digest anything undigested")
    args = ap.parse_args()

    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    TRACK_DIR.mkdir(parents=True, exist_ok=True)

    if args.digest:
        import digest_audio  # noqa: PLC0415 — optional, needs librosa

        import numpy as np

        for row in list(scan()):
            if row["status"] != "undigested":
                continue
            out = AUDIO_DIR / f"{row['id']}.8hz.npy"
            np.save(out, digest_audio.digest(row["audio"]))
            print(f"digested {row['id']}")

    rows = list(scan())
    INDEX.parent.mkdir(parents=True, exist_ok=True)
    with INDEX.open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=FIELDS)
        w.writeheader()
        w.writerows(rows)

    total = sum(float(r["seconds"]) for r in rows if r["seconds"])
    for status in ("paired", "digested, unauthored", "undigested"):
        n = sum(r["status"] == status for r in rows)
        print(f"{status + ':':<24}{n:>5}")
    print(f"{'pieces:':<24}{len(rows):>5}")
    print(f"{'authored minutes:':<24}{total / 60:>5.1f}")
    if not rows:
        print(f"\nnothing in {AUDIO_DIR}/ yet — the corpus is whatever you put there.")


if __name__ == "__main__":
    main()
