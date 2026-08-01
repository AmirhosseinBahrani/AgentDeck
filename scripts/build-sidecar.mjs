// Builds the MCP server and stages it where Tauri expects a sidecar.
//
// The app hands each agent an --mcp-config pointing at `deck-mcp` beside the running executable,
// which is the only way a worker can call claim_task_done. `cargo build` for the app does not
// build a sibling crate, and Tauri bundles only the main binary, so without this the packaged
// app ships a config referring to a binary that is not there — and every task stalls until it
// is reaped, with nothing in the UI explaining why.
//
// Tauri matches sidecars by host target triple and strips it when bundling, so the staged name
// has to carry one.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const triple = execFileSync("rustc", ["-vV"], { encoding: "utf8" })
  .split("\n")
  .find((line) => line.startsWith("host:"))
  ?.slice("host:".length)
  .trim();

if (!triple) {
  throw new Error("could not determine the host target triple from `rustc -vV`");
}

execFileSync("cargo", ["build", "--release", "-p", "deck-mcp"], {
  cwd: root,
  stdio: "inherit",
});

const suffix = triple.includes("windows") ? ".exe" : "";
const dest = join(root, "src-tauri", "binaries");
mkdirSync(dest, { recursive: true });
copyFileSync(
  join(root, "target", "release", `deck-mcp${suffix}`),
  join(dest, `deck-mcp-${triple}${suffix}`),
);

console.log(`staged deck-mcp-${triple}${suffix}`);
