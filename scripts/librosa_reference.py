#!/usr/bin/env python3
"""Dumps librosa's exact parameters and a full reference bundle for one clip.

Two jobs:

1. Print the defaults that `digest_audio.py` inherits without naming, so the
   Rust port can match them instead of guessing. Every one of these is a place
   the two implementations can silently disagree.

2. Write a reference bundle: the decoded samples, every intermediate feature
   at librosa's native frame rate, the filterbanks, and the final pooled
   (T,16) matrix. Feeding the *same samples* to Rust isolates the DSP from the
   decoder, so a mismatch means the maths differs rather than the mp3 reader.
"""
import inspect
import json
import sys

import numpy as np
import librosa

OUT = sys.argv[2] if len(sys.argv) > 2 else "reference"
CLIP_S = float(sys.argv[3]) if len(sys.argv) > 3 else 30.0

SR = 22050
N_FFT = 2048
HOP = 512
TARGET_HZ = 8
WINDOW_S = 1.0 / TARGET_HZ


def defaults(fn):
    return {
        k: (v.default if v.default is not inspect.Parameter.empty else "<required>")
        for k, v in inspect.signature(fn).parameters.items()
    }


def show(name, fn):
    d = defaults(fn)
    print(f"--- {name} ---")
    for k, v in d.items():
        if k in ("y", "S", "sr", "self"):
            continue
        print(f"    {k} = {v!r}")


print(f"librosa {librosa.__version__}, numpy {np.__version__}")
show("stft", librosa.stft)
show("feature.rms", librosa.feature.rms)
show("feature.spectral_centroid", librosa.feature.spectral_centroid)
show("feature.spectral_bandwidth", librosa.feature.spectral_bandwidth)
show("feature.chroma_stft", librosa.feature.chroma_stft)
show("feature.melspectrogram", librosa.feature.melspectrogram)
show("onset.onset_strength", librosa.onset.onset_strength)
show("power_to_db", librosa.power_to_db)
show("filters.mel", librosa.filters.mel)
show("filters.chroma", librosa.filters.chroma)

# --- the reference bundle -------------------------------------------------
path = sys.argv[1]
y, sr = librosa.load(path, sr=SR, mono=True, duration=CLIP_S)
print(f"\nloaded {path}: {len(y)} samples at {sr} Hz ({len(y)/sr:.2f}s)")

rms = librosa.feature.rms(y=y, hop_length=HOP)[0]
bandwidth = librosa.feature.spectral_bandwidth(y=y, sr=sr, hop_length=HOP)[0]
centroid = librosa.feature.spectral_centroid(y=y, sr=sr, hop_length=HOP)[0]
onset_env = librosa.onset.onset_strength(y=y, sr=sr, hop_length=HOP)
chroma = librosa.feature.chroma_stft(y=y, sr=sr, hop_length=HOP)

# The filterbanks, so the Rust side can be checked against them directly
# rather than against their downstream effect.
mel_fb = librosa.filters.mel(sr=sr, n_fft=N_FFT, n_mels=128)
chroma_fb = librosa.filters.chroma(sr=sr, n_fft=N_FFT, n_chroma=12)

# The magnitude spectrogram too — if this matches, every spectral feature
# downstream that still disagrees is a pooling or filterbank bug, not an FFT
# or windowing one.
spec = np.abs(librosa.stft(y=y, n_fft=N_FFT, hop_length=HOP))

centroid_norm = np.clip(centroid / (sr / 2), 0.0, 1.0)
native_times = librosa.frames_to_time(np.arange(len(rms)), sr=sr, hop_length=HOP)
duration = float(native_times[-1]) if len(native_times) else 0.0
n_windows = int(np.ceil(duration / WINDOW_S)) + 1

matrix = np.zeros((n_windows, 16), dtype=np.float32)
for w in range(n_windows):
    t0, t1 = w * WINDOW_S, (w + 1) * WINDOW_S
    mask = (native_times >= t0) & (native_times < t1)
    if not mask.any():
        continue
    matrix[w, 0] = rms[mask].mean()
    matrix[w, 1] = bandwidth[mask].mean()
    matrix[w, 2] = onset_env[mask].max()
    matrix[w, 3] = centroid_norm[mask].mean()
    matrix[w, 4:16] = chroma[:, mask].mean(axis=1)

np.save(f"{OUT}_y.npy", y.astype(np.float32))
np.save(f"{OUT}_rms.npy", rms.astype(np.float32))
np.save(f"{OUT}_bandwidth.npy", bandwidth.astype(np.float32))
np.save(f"{OUT}_centroid.npy", centroid.astype(np.float32))
np.save(f"{OUT}_onset.npy", onset_env.astype(np.float32))
np.save(f"{OUT}_chroma.npy", chroma.astype(np.float32))
np.save(f"{OUT}_melfb.npy", mel_fb.astype(np.float32))
np.save(f"{OUT}_chromafb.npy", chroma_fb.astype(np.float32))
np.save(f"{OUT}_spec.npy", spec.astype(np.float32))
np.save(f"{OUT}_matrix.npy", matrix)

meta = {
    "path": path,
    "sr": SR,
    "n_fft": N_FFT,
    "hop": HOP,
    "samples": int(len(y)),
    "native_frames": int(len(rms)),
    "windows": int(n_windows),
    "mel_shape": list(mel_fb.shape),
    "chroma_shape": list(chroma_fb.shape),
    "spec_shape": list(spec.shape),
    "librosa": librosa.__version__,
}
with open(f"{OUT}_meta.json", "w") as fh:
    json.dump(meta, fh, indent=2)

print(json.dumps(meta, indent=2))
print(f"\nmatrix {matrix.shape}, per-column mean:")
for i, m in enumerate(matrix.mean(axis=0)):
    print(f"    [{i:2d}] {m:.6f}")
