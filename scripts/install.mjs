#!/usr/bin/env node
// scripts/install.mjs — single-shot paired install for agenthd CLI + GUI.
// Install root precedence: --root > $CARGO_INSTALL_ROOT > $CARGO_HOME > $HOME/.cargo.
// Empty env vars are treated as absent. The chosen value is resolved to an
// absolute path once and forwarded to BOTH cargo installs. [install].root
// (cargo's documented key) lives in .cargo/config.toml, NOT Cargo.toml;
// this installer does NOT read it. Flags: --help, --force, --root <dir>.
// `--root ''` and `--root` with no value are rejected before any npm/cargo.
// platformName (passed in or detected) governs ALL npm invocations:
// win32 -> spawn(comSpec, ['/d','/s','/c', <fixed npm line>], shell:false)
// otherwise -> spawn('npm', <argv>, shell:false). Cargo is always direct.
// Never launches the GUI and never writes agenthd settings; atomicity not promised.

import { spawn } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { resolve, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { homedir as defaultHomedir, platform as defaultPlatform } from "node:os";

export const HELP_TEXT = `Usage: node scripts/install.mjs [options]

Install / update the agenthd paired build (CLI + companion GUI) into the
same cargo root, building the Angular frontend first.

Options:
  --root <dir>     install root (absolute or relative to invoker cwd).
                   Empty values are rejected. Resolved to an absolute
                   path once and forwarded to both cargo installs so
                   both binaries land together. [install].root in
                   .cargo/config.toml is NOT consulted; pass --root
                   (or set CARGO_INSTALL_ROOT) to align with it.
  --force          forward --force to both cargo install invocations.
                   Not always on; cargo install --path can update the
                   same package without --force when the destination
                   matches what cargo would produce. --force overrides
                   cargo's overwrite refusal for conflicts (per Cargo's
                   documented semantics).
  --help, -h       show this help and exit 0.

Resolution order for the install root:
  --root > CARGO_INSTALL_ROOT > CARGO_HOME > $HOME/.cargo
  Empty env vars are treated as absent (no accidental install into "").

Requirements:
  - cargo, npm, node on PATH.
  - spikes/tauri-angular/{package.json,package-lock.json} present.
  - spikes/tauri-angular/src-tauri/Cargo.toml present.

On any non-zero npm/cargo exit the installer aborts immediately with
phase / path / code. It never launches the GUI and never writes
agenthd settings. Atomicity is not promised.
`;

const invalidArgs = (m) => Object.assign(new Error(m), { code: "invalid-args" });

export function parseArgs(argv) {
  let help = false, force = false, root = null;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--help" || a === "-h") help = true;
    else if (a === "--force") force = true;
    else if (a === "--root" || a.startsWith("--root=")) {
      const v = a === "--root" ? argv[++i] : a.slice("--root=".length);
      if (v === undefined) throw invalidArgs("--root requires a directory argument");
      if (v.length === 0) throw invalidArgs("--root requires a non-empty directory");
      if (a === "--root" && v.startsWith("--")) throw invalidArgs(`--root value cannot be a flag (got '${v}'; use --root=<dir> if you mean it)`);
      if (root !== null) throw invalidArgs("--root specified more than once");
      root = v;
    } else throw invalidArgs(`unexpected argument: ${a}`);
  }
  return { help, force, root };
}

export function repoRoot(importMetaUrl = import.meta.url) {
  return resolve(resolve(fileURLToPath(importMetaUrl), ".."), "..");
}

export function resolveInstallRoot(parsed, invokerCwd, env, homedirFn) {
  const cir = env.CARGO_INSTALL_ROOT;
  const ch = env.CARGO_HOME;
  const envRoot = (cir && cir.length > 0 ? cir : null) ?? (ch && ch.length > 0 ? ch : null);
  const candidate = parsed.root ?? envRoot ?? join(homedirFn(), ".cargo");
  if (typeof candidate !== "string" || candidate.length === 0) {
    throw Object.assign(new Error("install root resolved to empty string"), { code: "invalid-root" });
  }
  return resolve(invokerCwd, candidate);
}

// run(): single helper for npm + cargo. npm dispatches via comSpec /d /s /c
// <fixed line> on win32, or direct npm <argv> otherwise. cargo is always
// direct. Both use shell:false. --root is never in any npm line; it is
// passed to cargo via explicit argv. spawnFn is injected for tests.
export function run(cmd, argv, line, opts, spawnFn = spawn) {
  let c, a;
  if (cmd === "npm" && opts.platformName === "win32") {
    c = (opts.env && opts.env.ComSpec) || "cmd.exe";
    a = ["/d", "/s", "/c", line];
  } else {
    c = cmd;
    a = argv.slice();
  }
  return new Promise((res) => {
    const child = spawnFn(c, a, { cwd: opts.cwd, env: opts.env, shell: false, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    let stdout = "", stderr = "";
    child.stdout.on("data", (d) => { const s = d.toString(); stdout += s; process.stdout.write(s); });
    child.stderr.on("data", (d) => { const s = d.toString(); stderr += s; process.stderr.write(s); });
    child.on("error", (e) => res({ code: -1, stdout, stderr: stderr + `\n${e.message}` }));
    child.on("close", (code) => res({ code: code ?? -1, stdout, stderr }));
  });
}

export function buildPlan({ repoRootAbs, installRootAbs, force, run, platformName, env: envArg }) {
  const frontendDir = join(repoRootAbs, "spikes", "tauri-angular");
  const guiCrate = join(frontendDir, "src-tauri");
  const cliArgs = ["install", "--path", repoRootAbs, "--locked", "--bin", "agenthd", "--root", installRootAbs];
  const guiArgs = ["install", "--path", guiCrate, "--locked", "--bin", "agenthd-gui", "--features", "custom-protocol", "--root", installRootAbs];
  if (force) { cliArgs.push("--force"); guiArgs.push("--force"); }

  const distBrowserIndex = join(frontendDir, "dist", "agenthd-tauri-angular-spike", "browser", "index.html");
  const phaseErr = (label, p, code, r) => {
    const tail = r && r.stderr ? ` (${r.stderr.trim().split("\n").pop()})` : "";
    return Object.assign(new Error(`${label} at ${p} (${code})${tail}`), { code, path: p, phase: label });
  };

  const phases = [];
  phases.push(async () => {
    for (const t of ["cargo", "npm", "node"]) {
      const r = await run(t, ["--version"], t === "npm" ? "npm --version" : null, { cwd: repoRootAbs, env: envArg, platformName });
      if (r.code !== 0) throw phaseErr(`preflight ${t} --version`, repoRootAbs, "preflight", r);
    }
  });
  const mustExist = (label, p) => async () => {
    if (!existsSync(p) || (!statSync(p).isFile() && !statSync(p).isDirectory())) throw phaseErr(label, p, "missing");
  };
  phases.push(mustExist("validate package.json", join(frontendDir, "package.json")));
  phases.push(mustExist("validate package-lock.json", join(frontendDir, "package-lock.json")));
  phases.push(mustExist("validate companion Cargo.toml", join(guiCrate, "Cargo.toml")));
  phases.push(async () => {
    console.log(`[install] RUN npm ci --include=dev (cwd=${frontendDir})`);
    const r = await run("npm", ["ci", "--include=dev"], "npm ci --include=dev", { cwd: frontendDir, env: envArg, platformName });
    if (r.code !== 0) throw phaseErr("npm ci", frontendDir, "npm-failed", r);
  });
  phases.push(async () => {
    console.log(`[install] RUN npm run build (cwd=${frontendDir})`);
    const r = await run("npm", ["run", "build"], "npm run build", { cwd: frontendDir, env: envArg, platformName });
    if (r.code !== 0) throw phaseErr("npm run build", frontendDir, "npm-failed", r);
  });
  phases.push(mustExist("confirm dist/.../browser/index.html post-build", distBrowserIndex));
  phases.push(async () => {
    console.log(`[install] RUN cargo ${cliArgs.join(" ")}`);
    const r = await run("cargo", cliArgs, null, { cwd: repoRootAbs, env: envArg, platformName });
    if (r.code !== 0) throw phaseErr("cargo install --bin agenthd", repoRootAbs, "cargo-failed", r);
  });
  phases.push(async () => {
    console.log(`[install] RUN cargo ${guiArgs.join(" ")}`);
    const r = await run("cargo", guiArgs, null, { cwd: repoRootAbs, env: envArg, platformName });
    if (r.code !== 0) throw phaseErr("cargo install --bin agenthd-gui --features custom-protocol", repoRootAbs, "cargo-failed", r);
  });
  return phases;
}

export async function main({ argv, cwd, env, runner, homedirFn, platformName, repoRootAbs }) {
  let parsed;
  try { parsed = parseArgs(argv); }
  catch (err) {
    console.error(`[install] ABORT phase="parse args" path=<n/a> code=${err?.code ?? "invalid-args"}: ${err?.message}`);
    return 1;
  }
  if (parsed.help) { process.stdout.write(HELP_TEXT); return 0; }
  const repo = repoRootAbs ?? repoRoot();
  let root;
  try { root = resolveInstallRoot(parsed, cwd, env, homedirFn); }
  catch (err) {
    console.error(`[install] ABORT phase="resolve root" path=<n/a> code=${err?.code ?? "invalid-root"}: ${err?.message}`);
    return 1;
  }
  console.log(`[install] repo   = ${repo}`);
  console.log(`[install] root   = ${root}`);
  console.log(`[install] force  = ${parsed.force}`);

  const phases = buildPlan({ repoRootAbs: repo, installRootAbs: root, force: parsed.force, run: runner, platformName, env });
  for (const phase of phases) {
    try { await phase(); }
    catch (err) {
      const msg = err?.message ? `: ${err.message}` : "";
      console.error(`[install] ABORT phase="${err?.phase ?? "?"}" path=${err?.path ?? "<n/a>"} code=${err?.code ?? "error"}${msg}`);
      return 1;
    }
  }
  console.log("[install] completed (CLI + GUI installed into same cargo root)");
  return 0;
}

const invokedDirectly = (() => {
  try { return process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url; }
  catch { return false; }
})();

if (invokedDirectly) {
  process.exit(await main({
    argv: process.argv.slice(2),
    cwd: process.cwd(),
    env: process.env,
    runner: run,
    homedirFn: defaultHomedir,
    platformName: defaultPlatform(),
  }));
}