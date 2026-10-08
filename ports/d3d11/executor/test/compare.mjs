// Compare two PNGs of the same scene, such as the browser build's frame
// (scripts/visual-check/capture.mjs) and render_test's Direct3D 11 one:
//
//   node ports/d3d11/executor/test/compare.mjs browser.png d3d11.png
//
// Prints the mean and 95th percentile of the per-pixel difference (the
// largest of red, green and blue, 0 to 255) and the share of pixels that
// differ by more than 24. Needs `npm run build` first, for the decoder.
//
// Licence: AGPL-3.0-only, as VistaWASM.
import { readFileSync } from "node:fs";
import { decodePng } from "../../../../dist/png-decode.js";

const [first, second] = process.argv.slice(2);

if (!first || !second) {
  console.error("usage: node compare.mjs a.png b.png");
  process.exit(2);
}

const [a, b] = await Promise.all([first, second].map((path) => decodePng(new Uint8Array(readFileSync(path)))));

if (a.width !== b.width || a.height !== b.height || a.bitDepth !== 8 || b.bitDepth !== 8) {
  console.error(`the images differ in size or depth: ${a.width} x ${a.height}, ${b.width} x ${b.height}`);
  process.exit(2);
}

const pixels = a.width * a.height;
const differences = new Uint8Array(pixels);

for (let pixel = 0; pixel < pixels; pixel += 1) {
  let largest = 0;

  for (let channel = 0; channel < 3; channel += 1) {
    const left = a.data[pixel * a.channels + channel];
    const right = b.data[pixel * b.channels + channel];
    largest = Math.max(largest, Math.abs(left - right));
  }

  differences[pixel] = largest;
}

const sorted = Array.from(differences).sort((x, y) => x - y);
const mean = sorted.reduce((sum, value) => sum + value, 0) / pixels;
const over = sorted.filter((value) => value > 24).length;
console.log(
  `mean ${mean.toFixed(2)}, 95th percentile ${sorted[Math.floor(pixels * 0.95)]}, ` +
    `largest ${sorted[pixels - 1]}, ${((100 * over) / pixels).toFixed(1)}% of pixels differ by more than 24`
);
