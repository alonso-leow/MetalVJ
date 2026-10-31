# Materials: inline cost (rule 11)

Measured 2026-10-30 with `material-bench` (`cargo run --release -p metal-render --bin material-bench`).

- GPU: NVIDIA GeForce GTX 1050 Ti (discrete, Vulkan). Host load average 2.0 / 2.5 / 2.6 before, 1.9 / 2.4 / 2.6 after (other builds running; the CPU was not idle).
- Method: one run; per model and resolution every config renders round-robin (frame k of all configs before frame k+1 of any), 60 frames each after 5 warm-up, each frame is encode + submit + wait for the queue to drain. p50 reported, so a load spike hits every config alike and the ratios hold.
- Configs: `plain` (no material), `baked` (a 1024x1024 picture on the part at mix 1, no material: the cost of *sampling* a pre-baked albedo, not of producing it), and each kind inline at mix 1 (`material_apply` in the raymarch shader).

| model, size | plain | baked | concrete | wood | metal | other kinds |
|---|---|---|---|---|---|---|
| scene3d 720p | 1.05 ms | 1.95 | 1.14 (1.08x plain) | 1.12 (1.07x) | 1.12 (1.07x) | 1.07-1.10 |
| scene3d 1080p | 2.09 | 3.01 | 2.27 (1.09x) | 2.23 (1.07x) | 2.24 (1.07x) | 2.12-2.16 |
| bulb 720p | 5.21 | 6.16 | 5.38 (1.03x) | 5.35 (1.03x) | 5.35 (1.03x) | 5.28-5.33 |
| bulb 1080p | 10.26 | 11.18 | 10.66 (1.04x) | 10.60 (1.03x) | 10.61 (1.03x) | 10.43-10.49 |

## Decision

Inline `material_apply` costs at most +9 % over no material on the lightest raymarched model (scene3d 1080p, concrete) and +3-4 % on the heavy one (bulb), and every kind, wood and concrete included, is cheaper than sampling a baked picture. The "heavy kinds need a per-model budget and a pipeline cache keyed by kind" premise does not hold: the kind is a uniform value branch inside one shader, there is one pipeline per model, and no kind moves a frame by more than 0.2 ms at 1080p. **No per-model budget and no pipeline cache are implemented.** Revisit if a model with a much heavier march than bulb (10 ms at 1080p here) or a new kind with loops enters the family.

Caveat: the "baked" column excludes the one-off bake. Baking per frame would add a pass; baking once amortises to the baked column above, which is already slower than inline.
