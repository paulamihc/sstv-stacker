// Runs the page's WebAssembly outside the browser, the way the page does,
// on a folder of pictures, and writes the zip the page would give:
//   node web/check.mjs <folder> <out.zip>
// For checking the WebAssembly gives the same as the command line.
import { readFile, readdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const [folder, outZip] = process.argv.slice(2);
if (!folder || !outZip) {
  console.error("usage: node web/check.mjs <folder> <out.zip>");
  process.exit(2);
}
const PICTURE = /\.(png|jpe?g|bmp)$/i;
const SKIP = /^best copies( \d+)?$/i;

async function walk(dir, rel, out, depth) {
  for (const e of await readdir(dir, { withFileTypes: true })) {
    if (e.name.startsWith(".")) continue;
    const r = rel ? rel + "/" + e.name : e.name;
    if (e.isDirectory()) {
      if (depth < 8 && !SKIP.test(e.name)) await walk(join(dir, e.name), r, out, depth + 1);
    } else if (PICTURE.test(e.name)) {
      out.push([r, join(dir, e.name)]);
    }
  }
}

const files = [];
await walk(folder, "", files, 0);
files.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
const enc = new TextEncoder();
const parts = [];
for (const [name, path] of files) parts.push(enc.encode(name), new Uint8Array(await readFile(path)));
const total = parts.reduce((n, p) => n + 4 + p.length, 0);
const input = new Uint8Array(total);
const view = new DataView(input.buffer);
let at = 0;
for (const p of parts) {
  view.setUint32(at, p.length, true);
  input.set(p, at + 4);
  at += 4 + p.length;
}

const wasm = await readFile(fileURLToPath(new URL("dist/sstv_stacker_web.wasm", import.meta.url)));
const { instance } = await WebAssembly.instantiate(wasm, { env: { progress() {} } });
const x = instance.exports;
const ptr = x.stacker_alloc(input.length) >>> 0;
new Uint8Array(x.memory.buffer, ptr, input.length).set(input);
const outPtr = x.stacker_run(ptr, input.length, 0, 0, 0x5b6c) >>> 0;
const len = x.stacker_output_len() >>> 0;
const out = new Uint8Array(x.memory.buffer, outPtr, len).slice();
const ov = new DataView(out.buffer);
let o = 0;
const take = () => {
  const n = ov.getUint32(o, true);
  const b = out.subarray(o + 4, o + 4 + n);
  o += 4 + n;
  return b;
};
const summary = JSON.parse(new TextDecoder().decode(take()));
await writeFile(outZip, take());
console.log(summary.text || summary.error);
