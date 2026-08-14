// Downloads the pdfium runtime library for the HOST platform into `src-tauri/`
// so the Tauri bundler can ship it as a per-OS resource. The app points
// liteparse/PDFium at it at runtime via `PDFIUM_LIB_PATH` (see src-tauri lib.rs).
//
// Source + version are kept in lock-step with `liteparse-pdfium-sys` (which
// downloads the same asset at build time): run-llama/pdfium-binaries, tag
// `chromium/7897`. If you bump the liteparse dependency and PDFs break, update
// PDFIUM_TAG here to match the crate's `PDFIUM_RELEASE_TAG`.
//
// Per platform: Windows -> pdfium.dll, Linux -> libpdfium.so, macOS ->
// libpdfium.dylib (universal, lipo of arm64 + x64 so one .dmg covers both).

import {
  existsSync,
  mkdirSync,
  rmSync,
  copyFileSync,
  readdirSync,
  statSync,
} from "node:fs";
import { writeFile } from "node:fs/promises";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";

const PDFIUM_TAG = "chromium/7897";
const BASE_URL =
  "https://github.com/run-llama/pdfium-binaries/releases/download";

const here = dirname(fileURLToPath(import.meta.url));
const srcTauri = join(here, "..", "src-tauri");

function hostTarget() {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "win32") {
    return {
      libName: "pdfium.dll",
      stems: [arch === "arm64" ? "pdfium-win-arm64" : "pdfium-win-x64"],
      universal: false,
    };
  }
  if (platform === "linux") {
    return {
      libName: "libpdfium.so",
      stems: [arch === "arm64" ? "pdfium-linux-arm64" : "pdfium-linux-x64"],
      universal: false,
    };
  }
  if (platform === "darwin") {
    // The release matrix builds `universal-apple-darwin`, so ship a fat dylib
    // that loads on both Apple Silicon and Intel.
    return {
      libName: "libpdfium.dylib",
      stems: ["pdfium-mac-arm64", "pdfium-mac-x64"],
      universal: true,
    };
  }
  throw new Error(`prepare-pdfium: unsupported platform ${platform}/${arch}`);
}

async function downloadTo(url, dest) {
  const res = await fetch(url); // fetch follows the GitHub release redirect
  if (!res.ok) {
    throw new Error(`prepare-pdfium: GET ${url} -> HTTP ${res.status}`);
  }
  await writeFile(dest, Buffer.from(await res.arrayBuffer()));
}

function findFile(dir, name) {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      const found = findFile(full, name);
      if (found) return found;
    } else if (entry === name) {
      return full;
    }
  }
  return null;
}

async function fetchLib(stem, libName, workDir) {
  const tgz = join(workDir, `${stem}.tgz`);
  const url = `${BASE_URL}/${encodeURIComponent(PDFIUM_TAG)}/${stem}.tgz`;
  console.log(`prepare-pdfium: GET ${url}`);
  await downloadTo(url, tgz);
  const outDir = join(workDir, stem);
  mkdirSync(outDir, { recursive: true });
  // tar is available on Windows 10+, macOS and Linux runners.
  execFileSync("tar", ["-xzf", tgz, "-C", outDir]);
  const lib = findFile(outDir, libName);
  if (!lib) {
    throw new Error(`prepare-pdfium: ${libName} not found inside ${stem}.tgz`);
  }
  return lib;
}

async function main() {
  const { libName, stems, universal } = hostTarget();
  const dest = join(srcTauri, libName);
  const workDir = join(tmpdir(), `prepare-pdfium-${process.pid}-${Date.now()}`);
  mkdirSync(workDir, { recursive: true });
  try {
    const libs = [];
    for (const stem of stems) {
      libs.push(await fetchLib(stem, libName, workDir));
    }
    if (universal && libs.length === 2) {
      execFileSync("lipo", ["-create", ...libs, "-output", dest]);
      // libloading dlopen()s by absolute path, but keep the install name sane.
      try {
        execFileSync("install_name_tool", ["-id", "@rpath/libpdfium.dylib", dest]);
      } catch {
        /* non-fatal */
      }
    } else {
      copyFileSync(libs[0], dest);
    }
    const size = existsSync(dest) ? statSync(dest).size : 0;
    console.log(`prepare-pdfium: wrote ${dest} (${(size / 1e6).toFixed(1)} MB)`);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
