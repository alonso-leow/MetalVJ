#!/usr/bin/env python3
"""Rename Spanish model ids to their English ids inside operator JSON data.

Only whole JSON tokens change: a string that is exactly a Spanish model id
(as a key or a value), or a graph kind of the form `gen.<id>` / `fx.<id>` /
`mat.<id>` whose id part is exact. Free text (prose, chat logs, dataset
transcripts) is never touched, because a Spanish word inside a sentence is not
a model id.

Each changed file is first copied to `<file>.bak-pre-english`. The rewrite is
textual, so the file's formatting is kept; the result is parsed and checked
against the same mapping applied to the original parse before it is written.

Usage:
    migrate-model-ids.py [--dry-run] ROOT [ROOT ...]
"""
import argparse
import json
import os
import re
import shutil
import sys

# The 53 model-id rows of the English-ids mapping (Spanish -> English).
MODEL_IDS = {
    "brasa": "ember",
    "bulbo": "bulb",
    "catenoide": "catenoid",
    "corales": "corals",
    "escena3d": "scene3d",
    "estela": "wake",
    "estrobo": "strobe",
    "fluido": "fluid",
    "humo": "smoke",
    "lazer": "laser",
    "mengano": "someone",
    "monstruo": "monster",
    "nubes": "clouds",
    "objeto": "object",
    "ondas": "waves",
    "paisaje": "landscape",
    "papel-colorpanels": "colorpanels",
    "papel-dithering": "dithering",
    "papel-dotgrid": "dotgrid",
    "papel-dotorbit": "dotorbit",
    "papel-flutedglass": "flutedglass",
    "papel-gemsmoke": "gemsmoke",
    "papel-godrays": "godrays",
    "papel-graingradient": "graingradient",
    "papel-halftonecmyk": "halftonecmyk",
    "papel-halftonedots": "halftonedots",
    "papel-heatmap": "heatmap",
    "papel-imagedithering": "imagedithering",
    "papel-liquidmetal": "liquidmetal",
    "papel-meshgradient": "meshgradient",
    "papel-metaballs": "metaballs",
    "papel-neuronoise": "neuronoise",
    "papel-papertexture": "sheettexture",
    "papel-perlinnoise": "perlinnoise",
    "papel-pulsingborder": "pulsingborder",
    "papel-simplexnoise": "simplexnoise",
    "papel-smokering": "smokering",
    "papel-spiral": "spiral",
    "papel-swirl": "swirl",
    "papel-voronoi": "voronoi",
    "papel-warp": "warpcut",
    "papel-water": "water",
    "papel-waves": "ripples",
    "parallax-campos": "parallax-fields",
    "parallax-cielo": "parallax-sky",
    "parallax-ciudad": "parallax-city",
    "parallax-cueva": "parallax-cave",
    "particulas": "particles",
    "relampago": "lightning",
    "rio": "river",
    "terrario": "terrarium",
    "trafico": "traffic",
    "vitral": "stained-glass",
}
PREFIXES = ("gen", "fx", "mat")




def map_value(v, m):
    if isinstance(v, str):
        if v in m:
            return m[v]
        head, sep, tail = v.partition(".")
        if sep and head in PREFIXES and tail in m:
            return f"{head}.{m[tail]}"
        return v
    if isinstance(v, list):
        return [map_value(x, m) for x in v]
    if isinstance(v, dict):
        return {map_value(k, m) if isinstance(k, str) else k: map_value(x, m) for k, x in v.items()}
    return v


def textual_rewrite(text, m):
    """Rewrite JSON string tokens exactly equal to a mapped id, keeping layout."""
    alt = "|".join(sorted(map(re.escape, m), key=len, reverse=True))
    # A bare JSON string token: opened by a structural char or whitespace, closed
    # the same way. Escaped quotes inside prose are preceded by a backslash and
    # therefore never match.
    pat = re.compile(
        r'(?<=[\[{,:\s])"(?:(' + "|".join(PREFIXES) + r')\.)?(' + alt + r')"(?=\s*[,:\]}\s])'
    )

    def sub(mo):
        prefix, ident = mo.group(1), mo.group(2)
        new = m[ident]
        return f'"{prefix}.{new}"' if prefix else f'"{new}"'

    return pat.sub(sub, text)


def process(path, m, dry):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    try:
        before = json.loads(text)
    except ValueError:
        return None  # not JSON (or a JSON-lines file); skipped, reported by caller
    new_text = textual_rewrite(text, m)
    if new_text == text:
        return False
    after = json.loads(new_text)
    if after != map_value(before, m):
        sys.exit(f"verification failed for {path}: rewrite does not match the mapping")
    if not dry:
        shutil.copy2(path, path + ".bak-pre-english")
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_text)
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("roots", nargs="+")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    m = MODEL_IDS
    changed, skipped = [], []
    for root in args.roots:
        for dp, dn, fn in os.walk(root):
            dn[:] = [d for d in dn if not d.startswith(".")]
            for name in fn:
                if not name.endswith(".json") or name.endswith(".bak-pre-english"):
                    continue
                p = os.path.join(dp, name)
                res = process(p, m, args.dry_run)
                if res is None:
                    skipped.append(p)
                elif res:
                    changed.append(p)
    for p in changed:
        print("changed", p)
    print(f"{len(changed)} file(s) changed; {len(skipped)} non-JSON skipped")
    for p in skipped:
        print("skipped", p)


if __name__ == "__main__":
    main()
