#!/usr/bin/env python3
"""Adds wave-2 models to data/router/gate.json, copying each song's `allow`
from the model's analogue (the effect design notes §2, §3, §5). A new
model is never allowed where its analogue is not. Idempotent; appends only.

    scripts/gate_add_models.py fluid warpcut      # ids to add (default: all registered ones)
"""
import json, sys, pathlib

ANALOGUE = {
    "fineline": ["vaporgrid", "cyberspace"], "warping": ["kaleido", "fractals", "moire"],
    "landscape": ["aurora", "river", "geoterrain"], "waves": ["rings", "moire"],
    "object": ["scene3d", "membrane"], "fluid": ["solvent", "smoke"],
    "clouds": ["smoke", "aurora"], "ember": ["lightning", "smoke"],
    "particles": ["entrance", "sandstorm"], "catenoid": ["entrance", "cyberspace"],
    "corals": ["dla", "terrarium"], "bulb": ["fractals", "kaleido"],
    "traffic": ["turing", "puzzle"],
    "colorpanels": ["stained-glass"], "dithering": ["matrix"], "dotgrid": ["puzzle"],
    "dotorbit": ["rings"], "flutedglass": ["stained-glass"], "gemsmoke": ["smoke"],
    "godrays": ["beam"], "graingradient": ["aurora"], "halftonecmyk": ["scratch"],
    "halftonedots": ["scratch"], "heatmap": ["aurora"], "imagedithering": ["matrix"],
    "liquidmetal": ["solvent"], "meshgradient": ["aurora"], "metaballs": ["solvent"],
    "neuronoise": ["domain"], "sheettexture": ["scratch"], "perlinnoise": ["smoke"],
    "pulsingborder": ["laser"], "simplexnoise": ["smoke"], "smokering": ["smoke"],
    "spiral": ["geoterrain"], "swirl": ["geoterrain"], "voronoi": ["cyberspace"],
    "warpcut": ["kaleido"], "water": ["river"], "ripples": ["rings"],
}

def main():
    path = pathlib.Path(__file__).resolve().parent.parent / "data/router/gate.json"
    g = json.loads(path.read_text())
    ids = sys.argv[1:] or list(ANALOGUE)
    for mid in ids:
        if mid in g["models"]:
            continue
        g["models"].append(mid)
        for s in g["songs"].values():
            # any analogue allowed -> allowed (union keeps the psychedelic mapping broad,
            # never wider than what the song already permits somewhere)
            s["allow"][mid] = any(s["allow"].get(a, False) for a in ANALOGUE[mid])
    path.write_text(json.dumps(g, indent=2, ensure_ascii=False) + "\n")

main()
