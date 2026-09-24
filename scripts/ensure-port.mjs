// Frees TCP 5173 when it's held by a *suspended* (Ctrl+Z) vite/tauri dev
// server from this project — the usual cause of vite's "Port 5173 is
// already in use" crash. Only touches stopped-state processes whose working
// directory is this project and whose command line is vite/tauri; anything
// else aborts with a clear error instead of killing blindly.

import { execSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const PORT = 5173;
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function portInUse() {
  return new Promise((resolve) => {
    const s = net.connect(PORT, "127.0.0.1");
    s.on("connect", () => {
      s.end();
      resolve(true);
    });
    s.on("error", () => resolve(false));
  });
}

if (!(await portInUse())) process.exit(0);

if (process.platform !== "linux") {
  console.error(
    `Port ${PORT} is already in use. Stop the other dev server (Ctrl+C, not Ctrl+Z) and retry.`
  );
  process.exit(1);
}

// Suspended processes sit in state T and hold the port forever. Reap only
// those that belong to this project's dev server.
const victims = [];
for (const entry of fs.readdirSync("/proc")) {
  if (!/^\d+$/.test(entry)) continue;
  try {
    const stat = fs.readFileSync(`/proc/${entry}/stat`, "utf8");
    const state = stat.slice(stat.lastIndexOf(")") + 2).split(" ")[0];
    if (state !== "T") continue;
    const cmd = fs.readFileSync(`/proc/${entry}/cmdline`, "utf8").replace(/\0/g, " ");
    if (!/(\.bin\/vite|tauri dev|bin\/tauri)/.test(cmd)) continue;
    if (fs.readlinkSync(`/proc/${entry}/cwd`) !== ROOT) continue;
    victims.push(entry);
  } catch {
    // Process vanished mid-scan — ignore.
  }
}

if (victims.length === 0) {
  console.error(
    `Port ${PORT} is already in use by something else. Free it and retry\n` +
      `(a previous dev server suspended with Ctrl+Z — not Ctrl+C — is the usual cause;\n` +
      ` run \`jobs\` in that terminal and \`kill %1\`, or \`ss -tlnp | grep ${PORT}\`).`
  );
  process.exit(1);
}

for (const pid of victims) {
  try {
    process.kill(Number(pid), "SIGKILL");
  } catch {
    // Already gone — fine.
  }
}

// Give the kernel a moment, then confirm.
await new Promise((r) => setTimeout(r, 1500));
if (await portInUse()) {
  try {
    const out = execSync("ss -tlnp 2>/dev/null | grep 5173 || true", { encoding: "utf8" });
    console.error(`Port ${PORT} is still busy:\n${out}Free it and retry.`);
  } catch {
    console.error(`Port ${PORT} is still busy. Free it and retry.`);
  }
  process.exit(1);
}
console.log(`Reaped suspended dev server (pid ${victims.join(", ")}); port ${PORT} is free.`);
