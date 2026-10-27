# Frame rate report: why the window stalls around 24–50 fps

Date: 2026-10-12. Machine: Intel UHD 630 (i915, drives `eDP-1`) plus GTX 1050 Ti (nvidia,
drives `HDMI-A-1`), 8 threads, Wayland (niri). Build: current tree plus the warning cleanup
on top of it.

Status: **research only, no fix applied.** The proposal is at the end.

## 1. Short answer

The GPU is not the limit. The main cause is the UI thread **waiting on locks** that worker
threads hold for long stretches, mainly the author state lock held during the whole grid
render. The per-frame budget at 60 fps is 16.7 ms. The UI thread spends about 10 ms of
that waiting on locks, about 6 ms painting, and about 4 ms blocked in `present`. Those add up
to 20–22 ms, which is about 45–50 fps, and it matches the frame rates you see.

The 60 fps cap is only an upper limit. It does not cause the drop.

## 2. What was measured

All numbers come from `GET /perf` (`crates/metal-dash/src/perf.rs`), `nvidia-smi`, `nvidia-smi
pmon`, `top -H` and `ps`, on the current build with nothing open except the default shell.

| Measurement | Result |
|---|---|
| fps at idle (no preview), 10 s | 30 → 13 fps, falling as the app goes idle |
| frame p50 / paint / ui (idle) | p50 5–10 ms, paint 5.5–7.1 ms, ui 2.2–2.6 ms |
| process CPU (idle) | about 67% of one core |
| GPU, app stopped | 0–6% |
| GPU, app running (`nvidia-smi`) | spikes to 74–82%, between spikes 0–23% |
| GPU per process (`pmon`) | `metal-dash` 6–40% SM; `niri` and `Xorg` 0–5% |
| `grid.jpg` texture (before the cadence change) | 47 calls/s × 15 ms (70% busy) |
| `grid.jpg` texture (after, 8 Hz) | 4.3 calls/s × 21.7 ms (9% busy) |
| reads on the reads worker | `GET /state` 2/s × 13.6 ms; `GET /author/models/state` 2/s × 11.1 ms; `GET /author/dataset` 2/s × 9.6 ms |
| per-frame lock time, UI thread | `author.state` 5.9 ms/frame, `door.shared` 3.9 ms/frame |
| per-frame `frame.present` | 3.7 ms/frame |
| per-frame `frame.run_ui` | 2.4 ms/frame |
| fps on eDP-1 under grid load (earlier build) | 52–54 |
| fps on HDMI-A-1 under grid load (earlier build) | 51.7 |
| your reports | 24 fps (earlier), 45 fps (now) |

The per-frame lock numbers are the time the UI thread spent *acquiring* a mutex. They come
from `perf::locked()`, which wraps the lock calls in `app.rs` and `call/door.rs`.

## 3. Causes, ranked by evidence

### 3.1 Author state lock held during the GPU grid render (verified in code, measured)

`Author::grid_texture` (`crates/metal-dash/src/app.rs:65`) takes
`perf::locked(&self.state, "author.state")` and keeps the guard while
`call::author::render::grid_texture` draws every model. That is about 20 ms per call. The UI
thread needs the same mutex for its own reads of author state, so a frame that arrives during a
render waits up to the rest of that render.

The grid runs at 8 Hz when no preview is open and at 1 Hz with a preview open
(`shell/panels/nodes.rs`). Each call holds the lock for about 21 ms, so the lock is taken for
about 9% of wall time. Every collision costs the UI thread a full frame.

The `door.shared` lock was already moved out of the grid path (the comment in `call/door.rs`
says so). `author.state` was not.

### 3.2 Author reads hold the same lock for their whole JSON build (verified in code, measured)

`Author::dispatch` (`app.rs:129`) holds `author.state` for the whole route, including
serialising the reply. `GET /state`, `GET /author/models/state` and `GET /author/dataset` take
9–14 ms each, and the shell repeats them about twice a second (the 250 ms TTL in
`shell/reads.rs`). Each read blocks the UI thread for its full duration on a collision.

`json_memo` and the stale-while-revalidate cache reduce the frequency, but they do not shorten
the time the lock is held.

### 3.3 `frame.present` blocks (measured; cause not confirmed)

`window.rs:828` times `queue.present()` at about 3.7 ms per frame. Blocking there means either
the surface is waiting for vblank (FIFO), or the driver is waiting for a copy to the display.
The code asks for `Immediate` first, then `Mailbox`, then `FifoRelaxed`, then `AutoVsync`
(`pick_present_mode`, `window.rs:307`). **The mode that was actually picked is not logged**, so
this is not yet confirmed. Whether `Immediate` or `Mailbox` is available on this NVIDIA + Wayland
surface has not been checked.

### 3.4 Hybrid graphics: the app renders on NVIDIA, the laptop panel is on Intel (hypothesis)

The adapter is chosen with `HighPerformance` (`metal-render/src/gpu.rs:41`), which is the
GTX 1050 Ti. `eDP-1` is driven by `card2` (i915). `HDMI-A-1` is driven by `card1` (nvidia).

If the window sits on `eDP-1`, every presented frame has to be copied from the NVIDIA GPU to
the Intel scanout. That adds bandwidth and latency. The measurements did not show a big gap:
52 fps on eDP versus 52 fps on HDMI with the grid running. So this is **not the main cause**,
but it can still add some of the `present` time. It needs a direct test with a fixed window
placement.

### 3.5 Per-frame paint work (measured, moderate)

`paint` takes 5.5–7 ms per frame at idle with nothing animating. It covers egui tessellation,
texture upload for the previews, and the device renders. This is significant, but on its own
it would allow about 100 fps.

### 3.6 Background CPU load (measured, minor)

- `tone-infer` (ONNX on CPU, `metal-listen/src/tone/live.rs`) runs whenever a tone model is
  loaded. It showed about 15% CPU in `top -H`, even at idle.
- The cpal audio digest runs continuously.
- Timecode threads (`timecode.rs`) and `metal-sync` peer polling run on their own schedules.

With 8 hardware threads, this is not saturating the CPU. It can still add jitter to the UI
thread when the scheduler is busy.

### 3.7 Frame pacing (checked, not a limit)

- `frame_cap` default is 60 (`data/settings.json`: `"fps_cap":60`).
- `pace()` (`window.rs:965`) waits `IDLE_REFRESH` (100 ms) when idle, and waits at the frame
  gap while a preview is live. The fps reading drops at idle because the app *does not* draw
  unless something asks for it. That is the intended behaviour, not a cap.
- The monitors are both 60 Hz, so a 60 fps cap is the top of the range.

### 3.8 What was ruled out

- **GPU saturation:** `metal-dash` uses 6–40% of the GPU on average, with short bursts.
- **Software renderer:** the adapter is NVIDIA over Vulkan, not llvmpipe.
- **Output monitor:** the same fps on both outputs under load.
- **`fps_cap` setting:** it is 60, not lower.

## 4. Proposal

Ordered by expected gain per effort. Each step has a measurement that says whether it worked.

### P1. Release `author.state` before the GPU render (fixes 3.1)

Under the lock, copy what the render needs: the preview rows, the time, the model names and
the tile size. Release the lock, then call `grid_canvas` and `copy_texture`. If the renderer
itself needs `&mut`, give the render its own `Renderer` (or a small pool) so it does not
share the state lock.

- Gain: the UI thread stops waiting on grid renders. Expect `author.state` lock time to fall
  from about 5.9 ms/frame to under 1 ms.
- Measure: `GET /perf`, `spans.author.state.ms_per_frame`, before and after, with the grid
  running and no preview.

### P2. Build author reads outside the lock (fixes 3.2)

For `dispatch`, take the lock only to copy the inputs. Build the JSON reply with the lock
released. For the big reads (`/author/models/state`, `/author/dataset`, `/state`), keep the
JSON in a cache that is rebuilt on change, not on every request.

- Gain: each 9–14 ms read stops blocking the UI thread.
- Measure: lock time for `author.state` during the reads, and `GET /perf` read timings.

### P3. Log the present mode and pick deliberately (addresses 3.3)

Print the picked present mode and the surface's supported modes at startup. Prefer `Mailbox`
when `Immediate` is not truly available. Confirm the surface is not blocking on vblank for
the whole frame.

- Gain: if `present` falls from 3.7 ms to under 1 ms, that is about 3–4 ms per frame back.
- Measure: `frame.present` span.

### P4. Test the hybrid path, then decide (addresses 3.4)

Run the same scene with the window on `eDP-1` and on `HDMI-A-1`, and compare `frame.present`
and fps. If `eDP-1` is clearly worse, consider choosing the adapter that drives the window's
output, or choosing the Intel adapter for the window (wgpu's `WGPU_ADAPTER_NAME`, if this wgpu version honours it).

- Measure: fps and `frame.present` on both outputs, same scene.

### P5. Keep background CPU off the UI path (addresses 3.6)

Run `tone-infer` only when audio is actually playing or a song is loaded. Lower its priority
with `nice` / `sched_setscheduler` if it must run. Check the cpal callback and the timecode
threads for busy loops.

- Measure: `top -H` per thread, idle and with the grid.

### P6. Paint cost (addresses 3.5)

Profile the per-frame paint at idle, looking for texture re-uploads and tessellation that
could be cached. Do this after P1–P3, because lock waits are the bigger cost.

## 5. Acceptance

- With the grid running and no preview: fps holds at the cap (60) while there is motion, and
  `author.state` lock time stays under 1 ms per frame.
- With a preview open: the preview runs at the cap and its frame time stays under 16 ms.
- `frame.present` under 1 ms per frame.

## 6. Reproduction

```sh
cargo build --release -p metal-dash
./target/release/metal-dash > /tmp/run.log 2>&1 &
curl -s http://127.0.0.1:7900/perf | python3 -m json.tool | less   # fps, frame_ms, spans, calls
nvidia-smi pmon -s u -c 10                                           # GPU per process
top -H -b -n1 -p "$(pgrep -x metal-dash)"                            # CPU per thread
```

Window moves: `niri msg windows` for the id, then
`niri msg action move-window-to-monitor --id <id> HDMI-A-1`.
