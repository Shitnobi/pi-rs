// Ground-truth generator for crates/pi-photon/tests/parity.rs (see
// PARITY.md "Photon wasm under wasmtime"). Runs the REAL
// @silvia-odwyer/photon-node under Node over the inputs in /tmp/imgtest/
// and writes the gt-* files the Rust byte-parity test asserts against.
//
// Machine setup (run after cloning pi-rs + /tmp/pi-repo per BOOTSTRAPME):
//   mkdir -p /tmp/imgtest
//   cp parity/fixtures/{small.png,tall.jpg,img.bmp,small.gif,small.webp} /tmp/imgtest/
//   # large.png: any deterministic >2000px PNG; parity/fixtures/resize.png
//   # (2100x300) is what the original /tmp/imgtest used.
//   cp parity/fixtures/resize.png /tmp/imgtest/large.png
//   node scripts/gen-photon-ground-truth.mjs
//
// Pipeline mirrors the Rust test exactly: decode -> PNG roundtrip; resize
// (2000x2000 cap, aspect-preserving, Math.round, Lanczos3); PNG + JPEG
// (q80/85/70/55/40) encodes of the resized image.
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const PI_REPO = process.env.PI_REPO || "/home/shinobi/AI/pi-repo";
const require = createRequire(import.meta.url);

const photon = require(PI_REPO + "/node_modules/@silvia-odwyer/photon-node/photon_rs.js");
const { PhotonImage, SamplingFilter } = photon;

const DIR = "/tmp/imgtest";
const cases = [
  ["small.png", "small-png"],
  ["large.png", "large-png"],
  ["tall.jpg", "tall-jpg"],
  ["img.bmp", "bmp"],
  ["small.gif", "gif"],
  ["small.webp", "webp"],
];

for (const [file, label] of cases) {
  const bytes = new Uint8Array(fs.readFileSync(path.join(DIR, file)));
  const img = PhotonImage.new_from_byteslice(bytes);
  const ow = img.get_width();
  const oh = img.get_height();
  console.log(`${file}: ${ow}x${oh}`);

  // decode -> PNG re-encode round trip
  fs.writeFileSync(
    path.join(DIR, `gt-${label}-roundtrip.png`),
    Buffer.from(img.get_bytes()),
  );

  // resize path (same math as the Rust test)
  let tw = ow;
  let th = oh;
  if (tw > 2000) {
    th = Math.round((th * 2000.0) / tw);
    tw = 2000;
  }
  if (th > 2000) {
    tw = Math.round((tw * 2000.0) / th);
    th = 2000;
  }
  if (tw !== ow || th !== oh) {
    const resized = photon.resize(img, tw, th, SamplingFilter.Lanczos3);
    fs.writeFileSync(
      path.join(DIR, `gt-${label}-resized.png`),
      Buffer.from(resized.get_bytes()),
    );
    for (const q of [80, 85, 70, 55, 40]) {
      fs.writeFileSync(
        path.join(DIR, `gt-${label}-resized-q${q}.jpg`),
        Buffer.from(resized.get_bytes_jpeg(q)),
      );
    }
  }
}
console.log("ground truth written to", DIR);
