// The preview painter's Web Worker: the same wasm module as the page, driven
// by messages. `boot` hands it the transferred OffscreenCanvas; `draw` asks for
// one model's picture (answers `frame` with the encoded reply); `quality` sets
// the render scale. Draws run one at a time: a call overlapping another would be
// refused by the module.
import init, { worker_boot, worker_draw, worker_quality } from "./pkg/metal_dash.js";

let ready = init();
let chain = Promise.resolve();

self.onmessage = (event) => {
  const m = event.data;
  chain = chain.then(async () => {
    await ready;
    if (m.kind === "boot") {
      let ok = false;
      try {
        ok = await worker_boot(m.canvas);
      } catch (e) {
        console.error("metalvj worker: boot failed", e);
      }
      self.postMessage({ kind: "ready", ok });
    } else if (m.kind === "draw") {
      const bytes = await worker_draw(m.header, m.floats);
      self.postMessage({ kind: "frame", bytes }, [bytes.buffer]);
    } else if (m.kind === "quality") {
      worker_quality(m.value);
    }
  }).catch((e) => console.error("metalvj worker:", e));
};
