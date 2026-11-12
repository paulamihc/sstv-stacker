// SSTV Stacker's worker: runs the combining (WebAssembly) off the page's
// thread, so the page stays responsive. Nothing here touches the network
// except loading the .wasm file from this site.
"use strict";

let ready = null;

function load() {
  if (!ready) {
    const env = {
      progress(done, of) {
        self.postMessage({ type: "progress", done, of });
      },
    };
    ready = (async () => {
      const res = await fetch("sstv_stacker_web.wasm");
      if (!res.ok) throw new Error("couldn't load the stacker (" + res.status + ")");
      const bytes = await res.arrayBuffer();
      const { instance } = await WebAssembly.instantiate(bytes, { env });
      return instance.exports;
    })();
  }
  return ready;
}

self.onmessage = async (e) => {
  const { input, colourFix, dosTime, dosDate } = e.data;
  try {
    const x = await load();
    const data = new Uint8Array(input);
    // WebAssembly hands numbers back signed: ">>> 0" reads them as the
    // unsigned addresses and lengths they are.
    const ptr = x.stacker_alloc(data.length) >>> 0;
    new Uint8Array(x.memory.buffer, ptr, data.length).set(data);
    const out = x.stacker_run(ptr, data.length, colourFix, dosTime, dosDate) >>> 0;
    const len = x.stacker_output_len() >>> 0;
    // Copied out before freeing: the module's memory may move.
    const copy = new Uint8Array(x.memory.buffer, out, len).slice();
    x.stacker_free(out, len);
    self.postMessage({ type: "done", output: copy.buffer }, [copy.buffer]);
  } catch (err) {
    self.postMessage({ type: "error", message: String(err && err.message || err) });
  }
};
