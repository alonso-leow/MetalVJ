# The Colourist dataset: parts, not songs

Authored examples the `ai.colourist` learns from (music row → float offsets),
cut down to the **parts** of each song where the show should change. Started
2026-10-06 on `feat/colourist-dataset`.

## Why parts

Most of a song repeats. Training on whole songs teaches the Colourist the
repetitive loop many times over and the transitions almost never. A DJ cuts at
intros, drops, breaks and bridges, and the router changes model there. So the
dataset is **biased on purpose** toward those places. Each part keeps a margin
around its anomaly: a pre-roll that holds the approach (the whole build when
there is one) and a post-roll that lands in the repetitive section after it, so
the Colourist also sees what "after the change" looks like.

## How the parts were found (`scripts/dataset_parts.py`)

Inputs: the canonical 32 Hz digest (`data/audio/<stem>.32hz.npy`, music
columns 0–15), the digester's beats (`<stem>.tempo.json`), and the gate
(`data/router/gate.json`: keywords and allowed models).

1. **One beat grid per song.** The tracker's BPM segments jump around (median
   tempo confidence 0.48), so the period is the median beat gap and the phase
   is the circular mean of the beats. A bar is 4 beats. The tracker rarely
   commits to a downbeat, so bar numbers are grid bars, not confirmed "1"s.
2. **Bar features:** timbre (RMS, bandwidth, centroid, mean onset, z-scored per
   song), the 16-step onset pattern inside the bar, and normalised chroma.
3. **Self-similarity** between bars (0.45 timbre + 0.25 rhythm + 0.30 chroma).
   - *Repetitiveness* of a bar is the mean of its 3 best matches at least 4
     bars away. Low means anomalous.
   - *Novelty* is Foote's checkerboard kernel (4 bars) on that matrix, which
     gives boundary strength.
4. **Score** = novelty + 0.7 × anomaly over the ±2 bars around the boundary
   + 0.35 × RMS step (capped at 3 s.d.). The intro is always a candidate. The
   last 6 bars are weighed ×0.6, because the kernel's edge padding and the
   fade-out made outros win 49 of 172 parts in the first pass.
5. **Kind** comes from the numbers around the boundary:
   - +0.8 s.d. RMS step: `drop`, or `build-drop` when RMS was rising over the
     8 bars before.
   - −0.8 s.d. RMS step: `break`, or `bridge` when the onsets also thin out.
   - otherwise: `harmonic-change` (chroma cosine shift) or `section-change`.
   - by position: `intro` and `outro`.
6. **Windows:** pre-roll 4 bars (8 for a build, 2 for an intro), post-roll
   6–8 bars, non-overlapping.
7. **Budget by keyword rarity**, so keywords are regularised, not songs.
   Parts per song = clamp(round(max over its keywords of 8 / songs with that
   keyword), 1, 5):
   - common keywords (slow, edm and pop with 12 songs each; hip hop 10;
     happy 11) → 1 part;
   - cha cha / bachata / merengue / latin (5–6 songs) → 1–2;
   - swing / psychedelic / r&b (3) → 3;
   - relax / cumbia / afro / playful (2) → 4;
   - one-song keywords (samba, disco, son, child, dark, fast, energetic,
     old school, latin pop) → 5.
   `merengeue` is read as `merengue`. Song 62 (a duplicate, nothing allowed)
   is skipped.
8. **Models:** each part takes a model the gate allows for that song, never
   twice in one song, preferring the model with the fewest songs so far.
   `monstruo` is **deferred**: the gate allows it wherever it allows `oracle`
   (20 songs), but `data/voice-labels` is not ready.

## What came out (`parts.json`)

- **171 parts over 73 songs**: 4,758 s of the 5.42 h corpus (24 %).
- Part length: median 27 s (p10 20 s, p90 35 s).
- Kinds: 54 break, 36 build-drop, 23 drop, 19 intro, 18 section-change,
  11 outro, 9 harmonic-change, 1 bridge.
- RMS step at the boundary, median: build-drop +1.9 s.d., drop +1.8, intro
  +2.0, break −1.8, outro −3.0, section/harmonic change ≈ 0. The kinds are
  separated by the numbers, not just by name.
- Boundary bars are less repetitive than the song (median 0.76 vs 0.79), as
  intended.
- Every keyword ends up with 4–34 parts (it was 1–12 songs). Every allowed
  model is in ≥ 4 songs, except `forge` (2: the gate allows it on 2 songs
  only).
- By keyword: EDM concentrates on break (9) and build-drop (7); cha cha is
  mostly breaks (5 of 10); bachata's anomalies are intros (3), breaks (3) and
  harmonic changes (2), with a single drop; slow songs spread over break (8),
  section-change (6) and drop (5), so their changes are less about energy.
- Tempo: median 123 BPM, range 78–185. A few are likely half/double-time
  reads (e.g. "ARE WE STILL FRIENDS?" at 185). This only shifts the bar grid,
  not the boundary.

## Files

- `parts.json`: every song's parts (ticks are canonical 32 Hz), facts,
  model.
- `specs/<part>.json`: what an author wrote: sparse per-float keys, each with
  its reason (`why`), plus the part's story.
- `examples/<part>.json`: built by `scripts/dataset_example.py build`. An
  `Example` (`model, song, rate: 32, ticks` = whole rows) plus `part`,
  `span`, `kind`, `keywords`, `title`, `story`, `keys`.
- `proof/<part>/`: 3–6 frames rendered by `metal-render --bin example-proof`.
  Every part's author looked at them before it was accepted.

Training: `POST /author/grade/train { model }` pools these parts with the
whole-song examples in `data/animations/`, keeping **only the ticks inside each
part's span** (`metal-dash::author::dataset::takes_for`).

Viewing: in the sequence editor, the **dataset** picker (top bar) opens a
part as a clip of its song. Mark-in/out is the span, the view zooms to it, and
⏵ plays it with the song's audio.

How to author a part: `the dataset notes`.
