#!/usr/bin/env node
/**
 * One-command launcher: `npm run app`
 *
 * Checks the toolchain (Node, Rust), installs OS dependencies automatically
 * (system webview libs + Tesseract OCR where a package manager exists),
 * runs `npm ci` when node_modules is stale, then starts the desktop app via
 * `tauri dev`.
 *
 * Flags:
 *   --check        diagnose only: print what's missing, change nothing
 *   --no-install   fail instead of installing anything
 *   --test         run the core test suite before launching
 *
 * Tesseract is best-effort everywhere: without it the app still runs with
 * text extraction disabled.
 */

import { spawn, spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { homedir, platform } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const OS = platform(); // 'linux' | 'darwin' | 'win32' | ...
const ARGS = new Set(process.argv.slice(2));
const CHECK_ONLY = ARGS.has("--check");
const NO_INSTALL = ARGS.has("--no-install") || CHECK_ONLY;
const RUN_TESTS = ARGS.has("--test");
const SHELL = OS === "win32";

let failed = false;
const step = (msg) => console.log(`\n== ${msg}`);
const ok = (msg) => console.log(`   ok: ${msg}`);
const warn = (msg) => console.log(`   warn: ${msg}`);
const fail = (msg) => {
  failed = true;
  console.log(`   MISSING: ${msg}`);
};

/** Run a command, return true when exit code is 0. */
function run(cmd, args = [], opts = {}) {
  const r = spawnSync(cmd, args, { stdio: "ignore", shell: SHELL, ...opts });
  return r.status === 0;
}

function capture(cmd, args = []) {
  const r = spawnSync(cmd, args, { encoding: "utf8", shell: SHELL });
  return r.status === 0 ? (r.stdout || "").trim() : null;
}

/** Install step: run it, or in check/no-install mode just report it. */
function ensure(desc, checkFn, installFn) {
  if (checkFn()) {
    ok(desc);
    return true;
  }
  if (NO_INSTALL) {
    fail(`${desc} (auto-install disabled)`);
    return false;
  }
  console.log(`   installing: ${desc}...`);
  if (installFn()) {
    if (checkFn()) {
      ok(`${desc} (installed)`);
      return true;
    }
  }
  fail(desc);
  return false;
}

// ---- individual checks -------------------------------------------------------

function checkNode() {
  const major = Number(process.versions.node.split(".")[0]);
  if (major >= 20) {
    ok(`node ${process.versions.node}`);
    return true;
  }
  fail(`node >= 20 required (found ${process.versions.node}) — install from https://nodejs.org`);
  return false;
}

function checkRust() {
  return ensure(
    "rust toolchain (cargo)",
    () => run("cargo", ["--version"]),
    () => {
      if (OS === "win32") {
        // rustup-init via winget, then make cargo visible to child processes.
        if (
          run("winget", [
            "install", "-e", "--id", "Rustlang.Rustup",
            "--accept-source-agreements", "--accept-package-agreements",
          ])
        ) {
          process.env.PATH = `${path.join(homedir(), ".cargo", "bin")}${path.delimiter}${process.env.PATH}`;
          return run("rustup", ["toolchain", "install", "stable", "--profile", "minimal"]);
        }
        return false;
      }
      if (!run("curl", ["--version"])) {
        console.log("   curl is required to install rustup automatically.");
        return false;
      }
      const okInstall =
        run("sh", [
          "-c",
          "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable",
        ]) || run("sh", ["-c", "curl -sSf https://sh.rustup.rs | sh -s -- -y"]);
      if (okInstall) {
        process.env.PATH = `${path.join(homedir(), ".cargo", "bin")}${path.delimiter}${process.env.PATH}`;
        return run("cargo", ["--version"]);
      }
      return false;
    }
  );
}

const LINUX_WEBVIEW_PKGS = [
  "libwebkit2gtk-4.1-dev",
  "libgtk-3-dev",
  "libayatana-appindicator3-dev",
  "librsvg2-dev",
  "patchelf",
];

function detectApt() {
  return run("apt-get", ["--version"]);
}

function checkLinuxWebview() {
  const hasHeaders =
    run("pkg-config", ["--modversion", "javascriptcoregtk-4.1"]) &&
    run("pkg-config", ["--modversion", "gtk+-3.0"]);
  return ensure("linux webview build headers (webkit2gtk, gtk3, ayatana, rsvg, patchelf)", () => hasHeaders || webviewRecheck(), () => {
    if (!detectApt()) {
      console.log("   non-apt distro detected. Install the Tauri prerequisites manually:");
      console.log("   Fedora: sudo dnf install webkit2gtk4.1-devel gtk3-devel libayatana-appindicator-gtk3-devel librsvg2-devel patchelf");
      console.log("   Arch:   sudo pacman -S webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg patchelf");
      console.log("   See https://v2.tauri.app/start/prerequisites/ for others.");
      return false;
    }
    if (!run("sudo", ["-n", "true"])) {
      console.log("   passwordless sudo is unavailable. Run manually:");
      console.log(`   sudo apt-get update && sudo apt-get install -y ${LINUX_WEBVIEW_PKGS.join(" ")}`);
      return false;
    }
    return (
      run("sudo", ["apt-get", "update"]) &&
      run("sudo", ["apt-get", "install", "-y", ...LINUX_WEBVIEW_PKGS])
    );
  });
}

// pkg-config based recheck after install (closure reads fresh state).
function webviewRecheck() {
  return (
    run("pkg-config", ["--modversion", "javascriptcoregtk-4.1"]) &&
    run("pkg-config", ["--modversion", "gtk+-3.0"])
  );
}

function checkLinuxDisplay() {
  if (process.env.DISPLAY || process.env.WAYLAND_DISPLAY) {
    ok("display available");
    return true;
  }
  warn("no DISPLAY/WAYLAND_DISPLAY — the window needs a desktop or `xvfb-run npm run app`");
  return true; // non-fatal: headless build still works
}

function checkTesseract() {
  const desc = "tesseract OCR binary";
  if (run("tesseract", ["--version"])) {
    ok(desc);
    return true;
  }
  const hint = "the app runs without it (text extraction stays disabled)";
  if (NO_INSTALL) {
    warn(`${desc} not found — ${hint}`);
    return true;
  }
  console.log(`   installing (optional): ${desc}...`);
  const installed = installTesseract();
  if (installed && run("tesseract", ["--version"])) {
    ok(`${desc} (installed)`);
  } else {
    warn(`${desc} not installed — ${hint}`);
  }
  return true; // never fatal
}

function installTesseract() {
  if (OS === "linux" && detectApt()) {
    if (!run("sudo", ["-n", "true"])) {
      console.log("   run manually: sudo apt-get install -y tesseract-ocr");
      return false;
    }
    return run("sudo", ["apt-get", "install", "-y", "tesseract-ocr"]);
  }
  if (OS === "darwin") {
    if (!run("brew", ["--version"])) {
      console.log("   install Homebrew (https://brew.sh), then: brew install tesseract");
      return false;
    }
    return run("brew", ["install", "tesseract"]);
  }
  if (OS === "win32") {
    const installed = run("winget", [
      "install", "-e", "--id", "UB-Mannheim.TesseractOCR",
      "--accept-source-agreements", "--accept-package-agreements",
    ]);
    if (installed) warn("tesseract installed — restart your terminal if `tesseract` is not on PATH yet");
    return installed;
  }
  console.log("   install tesseract for your OS: https://tesseract-ocr.github.io/tessdoc/Installation.html");
  return false;
}

function checkMacosTools() {
  if (OS !== "darwin") return true;
  return ensure(
    "xcode command line tools",
    () => run("xcode-select", ["-p"]),
    () => {
      console.log("   run `xcode-select --install` and follow the prompt, then re-run.");
      return false;
    }
  );
}

function checkNpmDeps() {
  const nm = path.join(ROOT, "node_modules");
  const lock = path.join(ROOT, "package-lock.json");
  let stale = !existsSync(nm);
  if (!stale && existsSync(lock)) {
    try {
      stale = statSync(lock).mtimeMs > statSync(nm).mtimeMs;
    } catch {
      stale = true;
    }
  }
  return ensure("npm dependencies", () => !stale, () => {
    const r = spawnSync("npm", ["ci"], { cwd: ROOT, stdio: "inherit", shell: SHELL });
    return r.status === 0;
  });
}

// ---- main --------------------------------------------------------------------

function main() {
  console.log(`Screenshot Memory launcher (${OS}, ${CHECK_ONLY ? "check mode" : "auto-install"})`);

  step("toolchain");
  const nodeOk = checkNode();
  const rustOk = checkRust();

  step("system dependencies");
  let sysOk = true;
  if (OS === "linux") {
    sysOk = checkLinuxWebview() && sysOk;
    checkLinuxDisplay();
  } else if (OS === "darwin") {
    sysOk = checkMacosTools() && sysOk;
  } else if (OS === "win32") {
    ok("windows: WebView2 ships with the OS (evergreen)");
  } else {
    warn(`unrecognized platform '${OS}' — attempting generic launch`);
  }
  checkTesseract(); // best-effort, never fatal

  step("javascript dependencies");
  const npmOk = checkNpmDeps();

  if (CHECK_ONLY) {
    console.log(failed || !nodeOk || !rustOk || !sysOk || !npmOk ? "\ncheck: NOT READY" : "\ncheck: READY — run `npm run app`");
    process.exit(failed || !nodeOk || !rustOk || !sysOk || !npmOk ? 1 : 0);
  }

  if (!nodeOk || !rustOk || !sysOk || !npmOk || failed) {
    console.log("\nsetup incomplete — fix the MISSING items above and re-run `npm run app`.");
    console.log("Diagnose without changing anything: `npm run app:check`.");
    process.exit(1);
  }

  if (RUN_TESTS) {
    step("core tests");
    const t = spawnSync("cargo", ["test", "-p", "shotmemory-core", "--quiet"], {
      cwd: ROOT,
      stdio: "inherit",
      shell: SHELL,
    });
    if (t.status !== 0) {
      console.log("\ncore tests failed — not launching.");
      process.exit(1);
    }
  }

  step("starting Screenshot Memory");
  console.log("   first launch compiles the Rust backend (a few minutes), then opens the window.");
  const child = spawn("npm", ["run", "tauri", "--", "dev"], {
    cwd: ROOT,
    stdio: "inherit",
    shell: SHELL,
  });
  child.on("exit", (code) => process.exit(code ?? 0));
}

main();
