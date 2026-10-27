#!/usr/bin/env python3
"""Digests raw audio into the 8 Hz feature matrix the model trains on.

docs/AUDIO_FEATURES.md is the design doc — read that first. This is the
concrete implementation of its pooling strategy: librosa extracts features
at their native frame rate (~43 Hz at a 512-sample hop, 22050 Hz sample
rate), and this script pools them down into 125 ms windows — 8 windows per
second, chosen to match `koreo-core::motion::KEY_RATE` exactly, so an
audio frame and a motion keyframe share one clock.

Per 125 ms window, 16 floats:

  [0]     RMS (loudness)              — mean over the window
  [1]     spectral bandwidth          — mean over the window
  [2]     onset strength              — max over the window (a strength
                                         value, not a hard 0/1 gate, so a
                                         fast transient inside the window
                                         is never averaged away)
  [3]     spectral centroid           — mean over the window, then scaled
                                         into 0..1 by dividing by Nyquist
                                         (sr/2) and clipping — the "scaler"
                                         is this normalization, nothing
                                         learned or fit per song
  [4:16]  chroma (12 pitch classes)   — mean over the window (pooling
                                         choice not specified in the
                                         original spec; mean is the
                                         standard treatment for chroma
                                         energy and is what's implemented
                                         here — flag if a different
                                         pooling was intended)

No conditioning columns (style/difficulty/dancer_count) — per the decision
in NOTATION_SIMPLIFICATION.md, the model's only input is this matrix.
Those fields stay in generate_dataset.py purely as corpus metadata.

A song only counts as part of the corpus once it has been run through
this script — "digested" into this notation. Raw audio files are not a
valid dataset entry on their own.

Requires: pip install librosa numpy — neither is a dependency of the Rust
workspace; this is a standalone offline preprocessing step, run once per
audio source, output cached to disk (see `main` below).

Real-time / microphone use (flagged in AUDIO_FEATURES.md, not built): the
same per-window pooling works on a rolling buffer instead of a full file —
`digest_chunk` below is a placeholder for that path, not implemented yet.

Run: python3 data/digest/digest_audio.py <audio_file> [out.npy]
"""
import sys

import numpy as np
import librosa

TARGET_HZ = 8
WINDOW_S = 1.0 / TARGET_HZ  # 125 ms — matches koreo-core::motion::KEY_RATE
N_FEATURES = 16  # rms(1) + bandwidth(1) + onset(1) + centroid(1) + chroma(12)


def digest(path: str, sr: int = 22050, hop_length: int = 512) -> np.ndarray:
    """Returns a (n_windows, 16) float32 matrix, one row per 125 ms."""
    y, sr = librosa.load(path, sr=sr, mono=True)

    rms = librosa.feature.rms(y=y, hop_length=hop_length)[0]
    bandwidth = librosa.feature.spectral_bandwidth(y=y, sr=sr, hop_length=hop_length)[0]
    centroid = librosa.feature.spectral_centroid(y=y, sr=sr, hop_length=hop_length)[0]
    onset_env = librosa.onset.onset_strength(y=y, sr=sr, hop_length=hop_length)
    chroma = librosa.feature.chroma_stft(y=y, sr=sr, hop_length=hop_length)  # (12, T)

    centroid_norm = np.clip(centroid / (sr / 2), 0.0, 1.0)

    native_times = librosa.frames_to_time(np.arange(len(rms)), sr=sr, hop_length=hop_length)
    duration = float(native_times[-1]) if len(native_times) else 0.0
    n_windows = int(np.ceil(duration / WINDOW_S)) + 1

    out = np.zeros((n_windows, N_FEATURES), dtype=np.float32)
    for w in range(n_windows):
        t0, t1 = w * WINDOW_S, (w + 1) * WINDOW_S
        mask = (native_times >= t0) & (native_times < t1)
        if not mask.any():
            # No native frame landed in this window (can happen at the very
            # end of a file). Zero is the honest "nothing measured here",
            # not a guess at what the value should have been.
            continue
        out[w, 0] = rms[mask].mean()
        out[w, 1] = bandwidth[mask].mean()
        out[w, 2] = onset_env[mask].max()
        out[w, 3] = centroid_norm[mask].mean()
        out[w, 4:16] = chroma[:, mask].mean(axis=1)
    return out


def digest_chunk(*args, **kwargs):
    """Placeholder for the real-time/microphone path — not implemented.

    The pooling logic above is per-window and stateless, so it should
    transfer to a rolling buffer directly; what's missing is the streaming
    plumbing (buffer management, latency budget, when a 125 ms window is
    considered "closed"), which hasn't been designed yet.
    """
    raise NotImplementedError("real-time digestion is a flagged open item, not built")


def main():
    if len(sys.argv) < 2:
        print(f"usage: {sys.argv[0]} <audio_file> [out.npy]", file=sys.stderr)
        raise SystemExit(1)
    audio_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) > 2 else audio_path + ".8hz.npy"
    matrix = digest(audio_path)
    np.save(out_path, matrix)
    print(f"{audio_path}: {matrix.shape[0]} windows ({matrix.shape[0] / TARGET_HZ:.1f}s) -> {out_path}")


if __name__ == "__main__":
    main()
