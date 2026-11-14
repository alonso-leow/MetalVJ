#!/usr/bin/env python3
"""Regenerates the factory loop templates in data/loops/templates/.

    python3 scripts/gen_loop_templates.py

Loop templates T6. Every model gets three templates built from rhythmic
archetypes applied to its 2-3 most musical floats (picked by hand among the
floats with the model's highest `drive` weight, never a camera), plus a
palette track where it has a colour slot of its own. Every one-amount effect
gets two Amount templates; multi-parameter effects (fx.move, particles,
image-pop, forge, warp) get Float tracks over their own params (T11). Palette sets are pre-tested 4-colour OkLCh groups.

Model float names and drive weights are read from data/schema.json and each
choice is checked here; `crates/metal-perform/tests/templates.rs` checks the
result again against the live catalog (rule 7: no hand-written counts there).
The output dir is wiped of *.json first so a removed template disappears,
except hand-authored `<model>-a-<slug>.json` loops, which are kept.
"""

import json
import math
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "loops", "templates")

# ---------------------------------------------------------------------------
# palettes: [l, c, h] OkLCh. Each group reads dark -> light or is ordered so
# colour 0 is the "home" colour, 1-2 the movement, 3 the accent/flash.
# ---------------------------------------------------------------------------
PALETTES = {
    "skies": ("Skies", [
        [[0.28, 0.07, 265], [0.48, 0.12, 255], [0.72, 0.09, 230], [0.92, 0.04, 210]],  # clear day
        [[0.30, 0.10, 300], [0.52, 0.14, 340], [0.74, 0.13, 40], [0.90, 0.09, 80]],    # dusk
        [[0.18, 0.05, 270], [0.32, 0.09, 280], [0.55, 0.08, 250], [0.85, 0.05, 200]],  # night
        [[0.62, 0.10, 20], [0.78, 0.12, 55], [0.86, 0.07, 90], [0.80, 0.06, 220]],     # dawn
    ]),
    "backgrounds": ("Backgrounds", [
        [[0.14, 0.01, 260], [0.20, 0.02, 260], [0.27, 0.03, 255], [0.34, 0.03, 250]],  # slate
        [[0.15, 0.02, 40], [0.21, 0.03, 45], [0.28, 0.04, 50], [0.36, 0.04, 60]],      # umber
        [[0.13, 0.03, 160], [0.19, 0.04, 165], [0.26, 0.05, 170], [0.33, 0.05, 175]],  # deep teal
        [[0.12, 0.04, 300], [0.18, 0.05, 300], [0.25, 0.06, 305], [0.32, 0.06, 310]],  # plum
    ]),
    "psychedelia": ("Psychedelia", [
        [[0.65, 0.25, 330], [0.85, 0.18, 110], [0.60, 0.20, 250], [0.80, 0.16, 180]],  # tetrad
        [[0.70, 0.22, 20], [0.75, 0.20, 140], [0.55, 0.24, 290], [0.88, 0.16, 95]],
        [[0.60, 0.26, 0], [0.70, 0.20, 60], [0.80, 0.18, 120], [0.65, 0.20, 200]],     # wheel run
        [[0.55, 0.22, 280], [0.72, 0.22, 340], [0.82, 0.17, 30], [0.90, 0.14, 100]],
    ]),
    "rays": ("Rays", [
        [[0.95, 0.03, 90], [0.88, 0.14, 85], [0.78, 0.17, 60], [0.65, 0.20, 35]],      # sunbeam
        [[0.95, 0.03, 220], [0.85, 0.11, 210], [0.72, 0.15, 230], [0.60, 0.19, 265]],  # ice laser
        [[0.95, 0.04, 150], [0.87, 0.20, 145], [0.75, 0.22, 140], [0.62, 0.18, 165]],  # green laser
        [[0.94, 0.04, 330], [0.78, 0.20, 340], [0.66, 0.25, 350], [0.58, 0.22, 10]],   # magenta laser
    ]),
    "fire": ("Fire", [
        [[0.25, 0.10, 30], [0.50, 0.19, 35], [0.72, 0.18, 55], [0.92, 0.12, 95]],      # flame
        [[0.20, 0.08, 25], [0.42, 0.17, 28], [0.60, 0.20, 40], [0.80, 0.16, 70]],      # embers
        [[0.30, 0.12, 15], [0.55, 0.21, 25], [0.78, 0.16, 75], [0.95, 0.05, 100]],     # white heat
    ]),
    "neon": ("Neon", [
        [[0.70, 0.27, 340], [0.85, 0.16, 195], [0.75, 0.20, 300], [0.90, 0.19, 120]],
        [[0.65, 0.29, 5], [0.80, 0.18, 220], [0.90, 0.20, 105], [0.70, 0.24, 320]],
        [[0.88, 0.22, 140], [0.68, 0.27, 330], [0.80, 0.14, 235], [0.92, 0.18, 100]],
    ]),
    # Keyed to drive.rs's MetalVJ palette: hue 27 is the accent red, and the
    # fourth stop is a grey so a gradient has somewhere to go.
    "metalvj-red": ("MetalVJ red", [
        [[0.25, 0.10, 27], [0.45, 0.18, 27], [0.62, 0.20, 27], [0.80, 0.02, 27]],
        [[0.18, 0.06, 27], [0.35, 0.14, 27], [0.55, 0.21, 27], [0.70, 0.03, 260]],
        [[0.30, 0.02, 27], [0.50, 0.12, 27], [0.65, 0.19, 27], [0.92, 0.01, 27]],
    ]),
}

# ---------------------------------------------------------------------------
# archetypes: 16 unit values (or None = hold) and a curve.
# ---------------------------------------------------------------------------


def scale(vals, lo, hi):
    return [None if v is None else round(lo + v * (hi - lo), 3) for v in vals]


def quarter():  # four-on-the-floor hit decaying across each beat
    return [1, 0.55, 0.3, 0.15] * 4, "ramp"


def offbeat():  # the "and" of every beat
    return [0, None, 1, None] * 4, "step"


def build():  # climbs across the bar and drops on the "1"
    return [i / 15 for i in range(16)], "step"


def strobe():  # sixteenth strobe
    return [1, 0] * 8, "step"


def swell():  # half-bar sine swell, twice a bar
    return [0.5 - 0.5 * math.cos(2 * math.pi * i / 8) for i in range(16)], "ramp"


def breathe():  # one slow breath per bar
    return [0.5 - 0.5 * math.cos(2 * math.pi * i / 16) for i in range(16)], "ramp"


def backbeat():  # beats 2 and 4
    v = [0.0] * 16
    v[4] = v[12] = 1
    v[5] = v[13] = 0.4
    return v, "step"


def three_three_two():  # 3-3-2 bell pattern, twice
    v = [None] * 16
    for i in range(16):
        v[i] = 1 if i in (0, 3, 6, 8, 11, 14) else (0 if i in (1, 4, 7, 9, 12, 15) else None)
    return v, "step"


def fill():  # sixteenth roll on the last beat, home elsewhere
    v = [0, None, None, None, 0.2, None, None, None, 0, None, None, None, 1, 0.3, 1, 0.3]
    return v, "step"


ARCH = {f.__name__: f for f in (quarter, offbeat, build, strobe, swell, breathe, backbeat, three_three_two, fill)}


def track_float(param, arch, lo=0.15, hi=0.85):
    if isinstance(arch, (int, float)):  # a constant, held all bar: lo..hi unused
        return {"target": {"kind": "float", "param": param}, "curve": "step",
                "steps": [round(float(arch), 3)] + [None] * 15}
    if isinstance(arch, list):  # literal 16 unit values (None = hold) + ramp
        return {"target": {"kind": "float", "param": param}, "curve": "ramp", "steps": scale(arch, lo, hi)}
    vals, curve = ARCH[arch]()
    return {"target": {"kind": "float", "param": param}, "curve": curve, "steps": scale(vals, lo, hi)}


def track_amount(arch, lo, hi):
    vals, curve = ARCH[arch]()
    return {"target": {"kind": "amount"}, "curve": curve, "steps": scale(vals, lo, hi)}


def pick(c, g=None):
    p = {"colour": c}
    if g is not None:
        p["group"] = g
    return p


# Palette step patterns (picks never ramp).
PAL = {
    "beats": [pick(0), None, None, None, pick(1), None, None, None, pick(2), None, None, None, pick(3), None, None, None],
    "flash": [pick(0)] + [None] * 11 + [pick(3), pick(0), pick(3), pick(0)],
    "offbeat": [pick(0), None, pick(2), None] * 4,
    "groups": [pick(0, 0), None, None, None, pick(0, 1), None, None, None, pick(0, 2), None, None, None, pick(0, 3), None, None, None],
}


def track_palette(slot, palette, pattern):
    return {"target": {"kind": "palette", "slot": slot, "palette": palette}, "steps": PAL[pattern]}


# ---------------------------------------------------------------------------
# models: floats (first = lead), own colour slot, palette set.
# ---------------------------------------------------------------------------
MODELS = {
    "geoterrain": (["twist", "radius", "thickness"], "spiral", "psychedelia"),
    "domain": (["distortion", "rgb_split", "scanline"], None, None),
    "escape": (["tension", "pulsation", "escape"], "escape", "neon"),
    "entrance": (["speed_advance", "scale", "variation_palette"], "elements", "skies"),
    "cyberspace": (["neon", "speed_parallax", "glass"], "neon", "neon"),
    "solvent": (["hit", "agitation", "size"], "liquid_a", "psychedelia"),
    "chamber": (["height", "propagation", "gravity"], "mesh", "metalvj-red"),
    "membrane": (["wind_force", "luminescence", "growth"], "tip", "skies"),
    "oracle": (["pupil", "eyelid", "speed_scan"], "iris", "psychedelia"),
    "turing": (["fold", "wave_high", "shine"], "patron", "metalvj-red"),
    "fractals": (["drift", "fold"], "escape", "psychedelia"),
    "kaleido": (["torsion", "turn", "sharpness"], "crystal", "psychedelia"),
    "vaporgrid": (["speed", "sun", "perturbation"], "grid", "neon"),
    "matrix": (["pulse", "shine", "sweep"], "mesh", "neon"),
    "ascii": (["amplitude", "speed", "thickness"], "text", "neon"),
    "puzzle": (["overlay", "slide", "separation"], "pieces", "psychedelia"),
    "someone": (["teleport", "elasticity", "blink"], "body", "neon"),
    "rings": (["impulse", "shine", "alternate"], "rings", "rays"),
    "scratch": (["shine", "speed", "edge"], "scratchable", "fire"),
    "laser": (["shine", "speed", "dispersion"], "laser", "rays"),
    "sandstorm": (["wind", "shine", "turbulence"], "sand", "fire"),
    "strobe": (["duty", "glare", "sweep"], "light", "rays"),
    "lightning": (["glare", "reach", "shine_center"], "ray", "rays"),
    "moire": (["turn", "shift", "contrast"], "lines", "psychedelia"),
    "stained-glass": (["flash", "burst", "lit"], "glass", "psychedelia"),
    "smoke": (["rise", "density", "light"], "smoke", "skies"),
    "terrarium": (["flow", "vibration", "wind"], "sand", "fire"),
    "river": (["swell", "pulse", "foam"], "mesh", "skies"),
    "aurora": (["energy", "impact", "speed"], "palette", "skies"),
    "beam": (["pulse", "intensity", "amplitude"], "beam", "rays"),
    "scene3d": (["grid_scale", "metal", "bg_mix"], None, None),
    "dla": (["spark", "shine", "light"], "coral", "fire"),
    "forge": (["onset", "rms", "bandwidth"], None, None),
    "monster": (["pulse", "jaw_open", "trip"], "neon", "psychedelia"),
    "parallax-cave": (["light", "halo", "speed"], "lights", "neon"),
    "parallax-sky": (["speed", "light", "swing"], "lights", "skies"),
    "parallax-fields": (["speed", "light", "swing"], "lights", "skies"),
    "parallax-city": (["light", "blink", "speed"], "lights", "neon"),
}

# Hand-authored loops, one file per monstruo species
# (`scripts/monstruo_loops/<kind>.json`, a list of template objects), written
# out verbatim after the factory set. One file per species so the species'
# shader and its loops are authored together without touching this script.
MONSTRUO_LOOPS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "monstruo_loops")

# Three templates per model: (id suffix, name, [archetype per float], palette pattern).
MODEL_TEMPLATES = [
    ("heartbeat", "Heartbeat", ["quarter", "swell", None], "beats"),
    ("climb", "Climb", ["build", "fill", "breathe"], "flash"),
    ("skip", "Skip", ["offbeat", "three_three_two", None], "offbeat"),
]

# Never looped: cameras (CORRECTIONS §43).
CAMERA_NAMES = {"cam_x", "cam_y", "cam_z", "orbit", "distance", "fov", "rot_x", "rot_y", "rot_z"}

# ---------------------------------------------------------------------------
# effects: (name, archetype, lo, hi) x2 per `fx.*` kind (text effects are
# `text.<name>`). Zoom's range is -1..1, so 0.5 is neutral; kaleido is 0..16.
# ---------------------------------------------------------------------------
EFFECTS = {
    "shake": [("Kick", "quarter", 0.0, 0.6), ("Rattle", "fill", 0.0, 0.7)],
    "light_leak": [("Bloom", "swell", 0.0, 0.7), ("Dawn", "build", 0.0, 0.8)],
    "ghost": [("Echo", "offbeat", 0.1, 0.7), ("Haze", "breathe", 0.1, 0.6)],
    "kaleido": [("Fold", "quarter", 0.125, 0.5), ("Unfold", "build", 0.0, 0.5)],
    "crt": [("Flicker", "strobe", 0.2, 0.6), ("Scan", "swell", 0.1, 0.7)],
    "glitch": [("Stutter", "fill", 0.0, 0.9), ("Snap", "backbeat", 0.0, 0.8)],
    "posterize": [("Steps", "quarter", 0.1, 0.7), ("Crush", "build", 0.0, 0.8)],
    "zoom": [("Punch", "quarter", 0.5, 0.75), ("Dive", "build", 0.5, 0.95)],
    "fire": [("Flare", "quarter", 0.1, 0.8), ("Blaze", "build", 0.0, 0.9)],
    "lorenz": [("Drift", "breathe", 0.1, 0.7), ("Swirl", "swell", 0.1, 0.8)],
    "vignette": [("Breathe", "quarter", 0.2, 0.7), ("Iris", "build", 0.1, 0.9)],
    "blur": [("Wash", "swell", 0.0, 0.6), ("Focus", "backbeat", 0.0, 0.5)],
    "sharpen": [("Edge", "quarter", 0.1, 0.8), ("Crisp", "offbeat", 0.1, 0.7)],
    "grain": [("Dust", "strobe", 0.15, 0.4), ("Storm", "build", 0.1, 0.9)],
    "speed": [("Lurch", "backbeat", 0.4, 0.8), ("Rush", "build", 0.3, 0.9)],
    "text.typewriter": [("Type", "build", 0.0, 1.0), ("Tick", "quarter", 0.2, 0.9)],
    "text.wave": [("Ripple", "swell", 0.1, 0.8), ("Swing", "offbeat", 0.1, 0.7)],
    "text.jitter": [("Shiver", "strobe", 0.0, 0.5), ("Snap", "backbeat", 0.0, 0.8)],
    "text.color_cycle": [("Spin", "build", 0.0, 1.0), ("Blink", "quarter", 0.1, 0.8)],
    "text.scale_pulse": [("Throb", "quarter", 0.1, 0.9), ("Swell", "swell", 0.1, 0.7)],
    "texture": [("Texture", "quarter", 0.0, 60.0), ("Texturize", "swell", 0.0, 80.0)],
    "clarity": [("Clarity", "quarter", 0.0, 60.0), ("Define", "swell", 0.0, 80.0)],
    "dehaze": [("Dehaze", "quarter", 0.0, 60.0), ("Clear", "swell", 0.0, 80.0)],
}

# T11: multi-parameter effects — Float tracks over the effect's own params
# (metal_graph::params::effect_params): (name, [(param, archetype, lo, hi)]).
# fx.move's floats are -1..1, so 0.5 is still.
EFFECT_MULTI = {
    "grade": [
        ("Warmth", [("grade_temperature", "quarter", -60.0, 60.0), ("grade_contrast", "offbeat", -20.0, 40.0)]),
        ("Tone", [("grade_exposure", "swell", -30.0, 30.0), ("grade_saturation", "build", -20.0, 50.0)]),
    ],
    "denoise": [
        ("Clean", [("fx_denoise_luminance", "quarter", 0.0, 60.0), ("fx_denoise_detail", "swell", 30.0, 70.0)]),
    ],
    "optics": [
        ("Lens", [("fx_optics_distortion", "breathe", -40.0, 40.0), ("fx_optics_vignetting", "swell", -40.0, 40.0)]),
    ],
    "geometry": [
        ("Shift", [("fx_geometry_vertical", "quarter", -20.0, 20.0), ("fx_geometry_horizontal", "offbeat", -20.0, 20.0)]),
    ],
    "fm": [
        ("Wobble", [("fx_fm_cant", "swell", 0.0, 0.6), ("fx_fm_indice", "quarter", 0.1, 0.6)]),
    ],
    "ascii": [
        ("Terminal", [("fx_ascii_cant", "swell", 0.0, 0.9), ("fx_ascii_celda", "quarter", 0.07, 0.36)]),
    ],
    "brush": [
        ("Paint", [("fx_brush_cant", "swell", 0.0, 0.9), ("fx_brush_cobertura", "quarter", 0.1, 0.6)]),
    ],
    "wavewarp": [
        ("Swell", [("fx_wavewarp_cant", "swell", 0.0, 0.7), ("fx_wavewarp_alto", "quarter", 0.02, 0.16)]),
    ],
    "hardstutter": [
        ("Gate", [("fx_hardstutter_cant", "swell", 0.0, 0.9), ("fx_hardstutter_duty", "quarter", 0.2, 0.7)]),
    ],
    "move": [
        ("Figure eight", [("x", "breathe", 0.3, 0.7), ("y", "swell", 0.4, 0.6)]),
        ("Bounce", [("y", "quarter", 0.5, 0.62), ("x", "offbeat", 0.45, 0.55)]),
    ],
    "particles": [
        ("Burst", [("count", "quarter", 0.15, 0.9), ("glow", "quarter", 0.1, 0.8), ("size", "swell", 0.3, 0.7)]),
        ("Storm", [("drift", "build", 0.05, 0.8), ("jitter", "fill", 0.1, 0.9), ("direction", "breathe", 0.0, 0.5)]),
    ],
    "image-pop": [
        ("Pop beat", [("rate", "quarter", 0.05, 0.6), ("scale", "backbeat", 0.2, 0.5)]),
        ("Wander", [("position_x", "breathe", 0.2, 0.8), ("position_y", "swell", 0.3, 0.7), ("scale", "offbeat", 0.15, 0.35)]),
    ],
    # fx.warp (T12): `mode` picks the pattern, floor(mode*6): 0 radial,
    # 1 waves_h, 2 waves_v, 3 ripples, 4 swirl, 5 interference — held as a
    # constant at the band's centre. The T13 pack adds the rest.
    "warp": [
        ("Breathe", [("mode", 0.08, 0, 1), ("amount", "breathe", 0.0, 0.5)]),
        ("Ripple", [("mode", 0.58, 0, 1), ("amount", "quarter", 0.1, 0.6), ("phase", "build", 0.0, 1.0)]),
    ],
    "forge": [
        ("Pulse", [("onset", "quarter", 0.0, 1.0), ("rms", "swell", 0.2, 0.8)]),
        ("Climb", [("rms", "build", 0.0, 1.0), ("centroid", "breathe", 0.2, 0.8), ("bandwidth", "fill", 0.0, 0.9)]),
    ],
}

# ---------------------------------------------------------------------------
# T13: the warp pack. fx.warp `mode` band centres (floor(mode*6)).
# ---------------------------------------------------------------------------
RADIAL, WAVES_H, WAVES_V, RIPPLES, SWIRL, INTERFERENCE = (round((k + 0.5) / 6, 3) for k in range(6))


def ease_in(t):  # slow start, fast finish
    return t * t


def ease_out(t):
    return 1 - (1 - t) * (1 - t)


def const(v):
    return [v] + [None] * 15


def roll():  # phase walks one cycle per bar, a sixteenth per step
    return [i / 16 for i in range(16)]


def crest(at, base=0.25, peak=1.0, width=4):
    """Amount swelling into one crest at step `at` and settling after it."""
    v = []
    for i in range(16):
        d = min((i - at) % 16, (at - i) % 16)
        v.append(round(base + (peak - base) * max(0.0, 1 - d / width) ** 2, 3))
    return v


def snap_up(at, span):
    """Eases 0 -> 1 over `span` steps ending just before `at`, snaps to 0 on `at`."""
    v = [0.0] * 16
    for k in range(span):
        v[(at - span + k) % 16] = round(ease_in((k + 1) / span), 3)
    return v


def snap_down(at, span):
    """Snaps to 1 on `at`, then eases 1 -> 0 over the next `span` steps."""
    v = [0.0] * 16
    for k in range(span):
        v[(at + k) % 16] = round(1 - ease_out(k / span), 3)
    return v


def flash_once(at, peak=0.85, tail=0.2):
    v = [0.0] * 16
    v[at] = peak
    v[(at + 1) % 16] = tail
    return v


def ftrack(param, steps, curve="step"):
    return {"target": {"kind": "float", "param": param}, "curve": curve, "steps": steps}


def scaled(vals, lo, hi):
    return [None if x is None else round(lo + x * (hi - lo), 3) for x in vals]


# (pair id, name, warp tracks, the step the strobe lands on)
WARP_PACK = [
    ("zoom-in-snap", "Zoom in, snap", [
        ftrack("mode", const(RADIAL)),
        ftrack("amount", scaled(snap_up(12, 12), 0.0, 0.7)),
    ], 12),
    ("zoom-out-snap", "Zoom out, snap", [
        ftrack("mode", const(RADIAL)),
        ftrack("amount", scaled(snap_down(0, 16), 0.0, 0.7)),
    ], 0),
    ("rolling-waves", "Rolling waves", [
        ftrack("mode", const(WAVES_H)),
        ftrack("frequency", const(0.35)),
        ftrack("phase", roll()),
        ftrack("amount", scaled(crest(8), 0.0, 0.8), "ramp"),
    ], 8),
    ("cross-waves", "Cross waves", [
        ftrack("mode", const(INTERFERENCE)),
        ftrack("frequency", const(0.5)),
        ftrack("phase", roll()),
        ftrack("amount", scaled(crest(12), 0.0, 0.7), "ramp"),
    ], 12),
    ("ripple-pulse", "Ripple pulse", [
        ftrack("mode", const(RIPPLES)),
        ftrack("frequency", const(0.6)),
        ftrack("phase", roll()),
        ftrack("amount", scaled(crest(0, base=0.15, width=6), 0.0, 0.9), "ramp"),
    ], 0),
    ("swirl-breath", "Swirl breath", [
        ftrack("mode", const(SWIRL)),
        ftrack("amount", scaled(crest(8, base=0.0, width=8), 0.0, 0.6), "ramp"),
        ftrack("mosh", const(0.25)),
    ], 8),
    ("vertical-mosh", "Vertical mosh", [
        ftrack("mode", const(WAVES_V)),
        ftrack("frequency", const(0.25)),
        ftrack("phase", roll()),
        ftrack("amount", scaled(crest(4), 0.0, 0.6), "ramp"),
        ftrack("mosh", scaled(crest(4, base=0.2, width=6), 0.0, 0.7), "ramp"),
    ], 4),
]

# ---------------------------------------------------------------------------
# T14: reggaeton / dembow — "boom-ch-boom-chic", hits on steps 4, 5, 12, 13
# (1-based) = indices 3, 4, 11, 12. The first track of each template is the
# one that hits (the test reads it); the second hit of each pair is softer.
# ---------------------------------------------------------------------------
DEMBOW = (3, 4, 11, 12)


def dembow(lo, hi, soft=0.7):
    v = [lo] * 16
    for k, i in enumerate(DEMBOW):
        v[i] = hi if k % 2 == 0 else round(lo + (hi - lo) * soft, 3)
    return v


DEMBOW_TEMPLATES = [
    ("dembow-flash", "Dembow flash", {"effect": "flash"},
     [ftrack("opacity", dembow(0.0, 0.75)), ftrack("diffuse", const(0.8))]),
    ("dembow-warp", "Dembow punch", {"effect": "warp"},
     [ftrack("amount", dembow(0.0, 0.45)), ftrack("mode", const(RADIAL))]),
    ("dembow-estrobo", "Dembow", {"model": "strobe"},
     [ftrack("glare", dembow(0.1, 0.95)), ftrack("duty", dembow(0.3, 0.8))]),
]


def slug(s):
    return s.lower().replace("_", "-").replace(".", "-")


def shared_names(model):
    fl = [b for b in model["blocks"] if b["type"] == "floats"]
    return {f["name"] for b in fl[-2:] for f in b["fields"]}


def own_floats(model):
    """The model's own floats: every floats block but the trailing signal +
    effects blocks, minus the camera-track reserved floats."""
    fl = [b for b in model["blocks"] if b["type"] == "floats"]
    return {f["name"]: f for b in fl[:-2] for f in b["fields"] if f["name"] not in CAMERA_NAMES}


def main():
    schema = json.load(open(os.path.join(ROOT, "data", "schema.json")))
    models = {m["id"]: m for m in schema["models"] if not m.get("footage")}
    global SHARED
    SHARED = shared_names(next(iter(models.values())))
    missing = set(models) - set(MODELS)
    if missing:
        sys.exit(f"no template plan for models: {sorted(missing)}")

    os.makedirs(os.path.join(OUT, "palettes"), exist_ok=True)
    for d in (OUT, os.path.join(OUT, "palettes")):
        for f in os.listdir(d):
            # `<model>-a-<slug>.json` are hand-authored loops: never wiped.
            if f.endswith(".json") and "-a-" not in f:
                os.remove(os.path.join(d, f))

    def write(path, obj):
        # Steps and colour groups on one line each: a bar reads as a bar.
        text = json.dumps(obj, indent=1)
        text = re.sub(r"\[\s*([^\[\]]*?)\s*\]", lambda m: "[" + re.sub(r"\s*\n\s*", " ", m.group(1)) + "]", text)
        text = re.sub(r"\{\s*(\"(?:colour|oklch)\"[^{}]*?)\s*\}", lambda m: "{" + re.sub(r"\s*\n\s*", " ", m.group(1)) + "}", text)
        text = re.sub(r"\[\s*(\{[^\[\]]*?\})\s*\]", lambda m: "[" + re.sub(r"\s*\n\s*", " ", m.group(1)) + "]", text)
        with open(path, "w") as fh:
            fh.write(text + "\n")

    for pid, (name, groups) in PALETTES.items():
        write(os.path.join(OUT, "palettes", f"{pid}.json"),
              {"id": pid, "name": name, "groups": [[{"oklch": c} for c in g] for g in groups]})

    n = 0
    for mid, (floats, slot, palette) in MODELS.items():
        m = models[mid]
        own = own_floats(m)
        top = max(f["drive"] for f in own.values())
        for p in floats:
            # A name shared with a signal/effects float (fractals' and moire's
            # own `zoom`) is ambiguous for a Float track: never pick one.
            if p in SHARED or p not in own:
                sys.exit(f"{mid}: {p} is not an own float")
            if own[p]["drive"] < top:
                sys.exit(f"{mid}: {p} has drive {own[p]['drive']} < the model's top {top}")
        slots = {b["name"] for b in m["blocks"] if b["type"] == "color_slot"}
        if slot and slot not in slots:
            sys.exit(f"{mid}: no colour slot {slot}")
        for suffix, tname, archs, pal in MODEL_TEMPLATES:
            tracks = [track_float(p, a) for p, a in zip(floats, archs) if a]
            if slot:
                tracks.append(track_palette(slot, palette, pal))
            # The backdrop walks the dark backgrounds one group per beat.
            if suffix == "skip" and "background" in slots and slot != "background":
                tracks.append(track_palette("background", "backgrounds", "groups"))
            write(os.path.join(OUT, f"{mid}-{suffix}.json"),
                  {"id": f"{mid}-{suffix}", "name": tname, "target": {"model": mid}, "tracks": tracks})
            n += 1

    for eff, temps in EFFECTS.items():
        for tname, arch, lo, hi in temps:
            tid = f"fx-{slug(eff)}-{slug(tname)}"
            write(os.path.join(OUT, f"{tid}.json"),
                  {"id": tid, "name": tname, "target": {"effect": eff}, "tracks": [track_amount(arch, lo, hi)]})
            n += 1
    for eff, temps in EFFECT_MULTI.items():
        for tname, tracks in temps:
            tid = f"fx-{slug(eff)}-{slug(tname).replace(' ', '-')}"
            write(os.path.join(OUT, f"{tid}.json"),
                  {"id": tid, "name": tname, "target": {"effect": eff},
                   "tracks": [track_float(p, a, lo, hi) for p, a, lo, hi in tracks]})
            n += 1
    # T13: each warp loop and its strobe, paired by name: `warp-<pair>` +
    # `strobe-<pair>`. The strobe fires once per bar on the warp's snap or
    # crest step (one clock, so the same step index is the same instant).
    for pair, tname, tracks, hit in WARP_PACK:
        write(os.path.join(OUT, f"warp-{pair}.json"),
              {"id": f"warp-{pair}", "name": tname, "target": {"effect": "warp"}, "tracks": tracks})
        write(os.path.join(OUT, f"strobe-{pair}.json"),
              {"id": f"strobe-{pair}", "name": f"Strobe: {tname.lower()}", "target": {"effect": "flash"},
               "tracks": [ftrack("opacity", flash_once(hit)), ftrack("diffuse", const(0.7))]})
        n += 2
    # fx.flash's own templates (its default — `Library::default_for` prefers
    # `fx-<effect>-*` over the paired/themed packs).
    for tname, at, n_hits in (("Downbeat", 0, 1), ("Backbeat", 4, 2)):
        op = [0.0] * 16
        for k in range(n_hits):
            op[(at + 8 * k) % 16] = 0.7
            op[(at + 8 * k + 1) % 16] = 0.15
        tid = f"fx-flash-{slug(tname)}"
        write(os.path.join(OUT, f"{tid}.json"),
              {"id": tid, "name": tname, "target": {"effect": "flash"},
               "tracks": [ftrack("opacity", op), ftrack("diffuse", const(0.6))]})
        n += 1
    if os.path.isdir(MONSTRUO_LOOPS):
        for fname in sorted(os.listdir(MONSTRUO_LOOPS)):
            if not fname.endswith(".json"):
                continue
            for t in json.load(open(os.path.join(MONSTRUO_LOOPS, fname))):
                if not t["id"].startswith("monstruo-"):
                    sys.exit(f"{fname}: {t['id']} must start with monstruo-")
                write(os.path.join(OUT, f"{t['id']}.json"), t)
                n += 1
    for tid, tname, target, tracks in DEMBOW_TEMPLATES:
        write(os.path.join(OUT, f"{tid}.json"), {"id": tid, "name": tname, "target": target, "tracks": tracks})
        n += 1
    print(f"{n} templates, {len(PALETTES)} palette sets -> {OUT}")


if __name__ == "__main__":
    main()
