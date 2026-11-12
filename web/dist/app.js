// SSTV Stacker's page. Reads the chosen pictures, hands them to the worker
// (which runs the combining), and shows and saves the results. Nothing is
// sent anywhere.
"use strict";

const $ = (id) => document.getElementById(id);
const PICTURE = /\.(png|jpe?g|bmp)$/i;
// Earlier results, inside the folder chosen, aren't read again.
const SKIP_FOLDER = /^best copies( \d+)?$/i;

let originals = new Map(); // name -> File
let results = null; // { summary, zip, files: Map(name -> Uint8Array) }
let urls = [];

// ------------------------------------------------------------ choosing

function keep(name) {
  const parts = name.split("/");
  if (!PICTURE.test(parts[parts.length - 1])) return false;
  return !parts.some((p) => p.startsWith(".") || SKIP_FOLDER.test(p));
}

function fromInput(fileList, stripTop) {
  const files = [];
  for (const f of fileList) {
    let name = f.webkitRelativePath || f.name;
    if (stripTop && name.includes("/")) name = name.slice(name.indexOf("/") + 1);
    files.push([name, f]);
  }
  return files;
}

// Reads a dropped folder, and the folders inside it.
async function fromEntry(entry, prefix, out, depth) {
  if (entry.isFile) {
    const f = await new Promise((ok, bad) => entry.file(ok, bad));
    out.push([prefix + entry.name, f]);
  } else if (entry.isDirectory && depth < 8) {
    const reader = entry.createReader();
    const here = depth === 0 ? "" : prefix + entry.name + "/";
    for (;;) {
      const batch = await new Promise((ok, bad) => reader.readEntries(ok, bad));
      if (!batch.length) break;
      for (const e of batch) await fromEntry(e, here, out, depth + 1);
    }
  }
}

$("pick-folder").addEventListener("change", (e) => start(fromInput(e.target.files, true)));
$("pick-files").addEventListener("change", (e) => start(fromInput(e.target.files, false)));

const drop = $("drop");
drop.addEventListener("dragover", (e) => {
  e.preventDefault();
  drop.classList.add("over");
});
drop.addEventListener("dragleave", () => drop.classList.remove("over"));
drop.addEventListener("drop", async (e) => {
  e.preventDefault();
  drop.classList.remove("over");
  const items = [...(e.dataTransfer.items || [])];
  const entries = items.map((i) => i.webkitGetAsEntry && i.webkitGetAsEntry()).filter(Boolean);
  const files = [];
  if (entries.length) {
    const one = entries.length === 1 && entries[0].isDirectory;
    for (const en of entries) await fromEntry(en, "", files, one ? 0 : 1);
  } else {
    for (const f of e.dataTransfer.files) files.push([f.name, f]);
  }
  start(files);
});

// ------------------------------------------------------------ running

function dosNow() {
  const d = new Date();
  const date = ((Math.max(d.getFullYear(), 1980) - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate();
  const time = (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1);
  return { date, time };
}

function show(section) {
  for (const id of ["choose", "working", "results"]) $(id).hidden = id !== section;
}

async function start(chosen) {
  const files = chosen.filter(([name]) => keep(name));
  if (!files.length) {
    alert_("No PNG, JPEG or BMP pictures were found there.");
    return;
  }
  originals = new Map(files);
  show("working");
  setProgress(0, 1, "Reading " + files.length + " pictures…");
  // The input: for each file, its name and its bytes, each with its length.
  const enc = new TextEncoder();
  const parts = [];
  let total = 0;
  for (const [name, f] of files) {
    const n = enc.encode(name);
    const b = new Uint8Array(await f.arrayBuffer());
    parts.push(n, b);
    total += 8 + n.length + b.length;
  }
  const input = new Uint8Array(total);
  const view = new DataView(input.buffer);
  let at = 0;
  for (const p of parts) {
    view.setUint32(at, p.length, true);
    input.set(p, at + 4);
    at += 4 + p.length;
  }
  setProgress(0, 1, "Finding which copies go together…");
  const worker = new Worker("worker.js");
  const { date, time } = dosNow();
  worker.onmessage = (e) => {
    const m = e.data;
    if (m.type === "progress") {
      setProgress(m.done, m.of, "Combining picture " + m.done + " of " + m.of + "…");
    } else if (m.type === "done") {
      worker.terminate();
      finish(new Uint8Array(m.output));
    } else if (m.type === "error") {
      worker.terminate();
      show("choose");
      alert_("Something went wrong: " + m.message);
    }
  };
  worker.onerror = (e) => {
    worker.terminate();
    show("choose");
    alert_("Something went wrong: " + (e.message || "the stacker stopped"));
  };
  const colourFix = Number($("colour-fix").value) || 0;
  worker.postMessage({ input: input.buffer, colourFix, dosTime: time, dosDate: date }, [input.buffer]);
}

function setProgress(done, of, text) {
  $("status").textContent = text;
  const pct = of ? Math.round((done / of) * 100) : 0;
  $("meter-bar").style.width = pct + "%";
  $("meter").setAttribute("aria-valuenow", String(pct));
}

function alert_(text) {
  const p = $("choose").querySelector(".message") || document.createElement("p");
  p.className = "message problems";
  p.setAttribute("role", "alert");
  p.textContent = text;
  $("choose").appendChild(p);
}

// ------------------------------------------------------------ results

function parse(out) {
  const view = new DataView(out.buffer, out.byteOffset, out.byteLength);
  let at = 0;
  const take = () => {
    const n = view.getUint32(at, true);
    const b = out.subarray(at + 4, at + 4 + n);
    at += 4 + n;
    return b;
  };
  const dec = new TextDecoder();
  const summary = JSON.parse(dec.decode(take()));
  const zip = take();
  const files = new Map();
  while (at < out.length) {
    const name = dec.decode(take());
    files.set(name, take());
  }
  return { summary, zip, files };
}

function blobUrl(bytes, type) {
  const u = URL.createObjectURL(new Blob([bytes], { type }));
  urls.push(u);
  return u;
}

function typeOf(name) {
  if (/\.png$/i.test(name)) return "image/png";
  if (/\.jpe?g$/i.test(name)) return "image/jpeg";
  if (/\.bmp$/i.test(name)) return "image/bmp";
  return "application/octet-stream";
}

function fmt(v) {
  return v >= 999 ? "–" : v.toFixed(1);
}

function finish(out) {
  results = parse(out);
  const s = results.summary;
  if (s.error) {
    show("choose");
    alert_("Something went wrong: " + s.error);
    return;
  }
  $("summary").textContent = s.text + ".";
  const probs = $("problems");
  probs.replaceChildren();
  for (const u of s.unreadable) {
    const li = document.createElement("li");
    li.textContent = "Couldn't read " + u.name + ": " + u.why;
    probs.appendChild(li);
  }
  probs.hidden = !s.unreadable.length;
  $("save-folder").hidden = !("showDirectoryPicker" in window);
  $("saved").textContent = "";
  const grid = $("grid");
  grid.replaceChildren();
  // Combined pictures first, most copies first.
  const made = [...s.made].sort((a, b) => b.copies - a.copies || a.file.localeCompare(b.file));
  for (const m of made) grid.appendChild(card(m));
  $("choose").hidden = true;
  $("working").hidden = true;
  $("results").hidden = false;
  $("results-h").scrollIntoView({ block: "start" });
}

function card(m) {
  const node = $("card").content.firstElementChild.cloneNode(true);
  const before = node.querySelector(".before");
  const after = node.querySelector(".after");
  const original = originals.get(m.strongest);
  node.querySelector(".name").textContent = m.file;
  const lines = Math.round(m.coverage * 100) + "%" + (m.missing_rows ? " (" + m.missing_rows + " lines missing)" : "");
  node.querySelector(".coverage").textContent = lines;
  if (m.copies > 1) {
    if (original) before.src = blobUrl(original, typeOf(m.strongest));
    before.alt = "The best single copy, " + m.strongest;
    after.src = blobUrl(results.files.get(m.file), "image/png");
    after.alt = "Combined from " + m.copies + " copies";
    node.querySelector(".after-cap").textContent = "Combined from " + m.copies;
    const noise = node.querySelector(".noise");
    noise.textContent = fmt(m.best_single_noise) + " → " + fmt(m.noise);
    if (m.noise < m.best_single_noise) noise.classList.add("better");
    noise.title = "Estimated noise, 0 to 255 levels: lower is better";
    const body = node.querySelector("tbody");
    for (const [i, c] of (m.record ? m.record.copies : []).entries()) {
      const tr = document.createElement("tr");
      const notes = [];
      if (i === m.record.strongest) notes.push("strongest");
      if (c.swapped) notes.push("colours put back");
      if (c.note) notes.push(c.note);
      if (!c.used) notes.push("not used");
      for (const [text, cls] of [
        [c.name, "copy"],
        [c.shift[0] + ", " + c.shift[1], ""],
        [fmt(c.noise), ""],
        [notes.join("; "), ""],
      ]) {
        const td = document.createElement("td");
        td.textContent = text;
        if (cls) td.className = cls;
        tr.appendChild(td);
      }
      body.appendChild(tr);
    }
  } else {
    node.classList.add("single");
    before.parentElement.remove();
    after.src = blobUrl(results.files.get(m.file), typeOf(m.file));
    after.alt = m.file;
    node.querySelector(".after-cap").textContent = "Received once: passed on as it was";
    node.querySelector(".noise").textContent = fmt(m.noise);
    node.querySelector(".record").remove();
  }
  return node;
}

// ------------------------------------------------------------ saving

$("save-zip").addEventListener("click", () => {
  if (!results) return;
  const a = document.createElement("a");
  a.href = blobUrl(results.zip, "application/zip");
  a.download = "Best copies.zip";
  document.body.appendChild(a);
  a.click();
  a.remove();
  $("saved").textContent = "Saved as Best copies.zip, in your downloads.";
});

$("save-folder").addEventListener("click", async () => {
  if (!results) return;
  let root;
  try {
    root = await window.showDirectoryPicker({ mode: "readwrite" });
  } catch {
    return; // chose nothing
  }
  try {
    // A new folder each time, so nothing already there is overwritten.
    let name = "Best copies";
    for (let n = 2; await exists(root, name); n++) name = "Best copies " + n;
    const dir = await root.getDirectoryHandle(name, { create: true });
    for (const [path, bytes] of results.files) {
      const parts = path.split("/");
      let d = dir;
      for (const p of parts.slice(0, -1)) d = await d.getDirectoryHandle(p, { create: true });
      const fh = await d.getFileHandle(parts[parts.length - 1], { create: true });
      const w = await fh.createWritable();
      await w.write(bytes);
      await w.close();
    }
    $("saved").textContent = "Saved in " + root.name + "/" + name + ".";
  } catch (err) {
    $("saved").textContent = "Couldn't save there: " + (err.message || err);
  }
});

async function exists(dir, name) {
  try {
    await dir.getDirectoryHandle(name);
    return true;
  } catch {
    return false;
  }
}

$("again").addEventListener("click", () => {
  for (const u of urls) URL.revokeObjectURL(u);
  urls = [];
  results = null;
  originals = new Map();
  $("pick-folder").value = "";
  $("pick-files").value = "";
  $("results").hidden = true;
  $("choose").hidden = false;
  const msg = $("choose").querySelector(".message");
  if (msg) msg.remove();
  window.scrollTo(0, 0);
});
