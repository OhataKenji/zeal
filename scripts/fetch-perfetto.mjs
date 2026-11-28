// Fetches the pinned Perfetto UI static assets into
// src-tauri/resources/perfetto. Every file is verified against the SHA-256
// digest recorded in src-tauri/resources/perfetto-manifest.json, including
// files already on disk, so a warm cache is re-validated rather than trusted.
// Downloads are bounded-concurrency and land via temp file + rename, so a
// partial file is never visible under its final name. Node builtins only.
import { createHash } from "node:crypto";
import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const MANIFEST_PATH = join(ROOT, "src-tauri", "resources", "perfetto-manifest.json");
const OUT_DIR = join(ROOT, "src-tauri", "resources", "perfetto");
const CONCURRENCY = 6;
const FETCH_TIMEOUT_MS = 60_000;
const manifest = JSON.parse(await readFile(MANIFEST_PATH, "utf8"));
const base = manifest.baseUrl;
const entries = Object.entries(manifest.resources);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("base64");
}

async function verified(file, expected) {
  try {
    return sha256(await readFile(file)) === expected;
  } catch {
    return false;
  }
}

async function fetchOne([name, expected]) {
  const dest = join(OUT_DIR, name);
  const want = expected.replace(/^sha256-/, "");
  if (await verified(dest, want)) return "cached";
  await mkdir(dirname(dest), { recursive: true });
  const temp = `${dest}.tmp-${process.pid}`;
  try {
    const response = await fetch(new URL(name, base), {
      signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status} for ${name}`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (sha256(bytes) !== want) {
      throw new Error(`digest mismatch for ${name}`);
    }
    await writeFile(temp, bytes);
    await rename(temp, dest);
    return "fetched";
  } finally {
    await rm(temp, { force: true });
  }
}

const results = { cached: 0, fetched: 0 };
const queue = [...entries];
await Promise.all(
  Array.from({ length: CONCURRENCY }, async () => {
    for (let item = queue.shift(); item; item = queue.shift()) {
      results[await fetchOne(item)] += 1;
    }
  }),
);
console.log(
  `perfetto assets: ${results.fetched} fetched, ${results.cached} verified from cache ` +
    `(${entries.length} total) -> ${OUT_DIR}`,
);
