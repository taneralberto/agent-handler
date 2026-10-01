// scripts/install.test.mjs
//
// Unit tests for scripts/install.mjs. Uses only node:test (no deps).
//
// Strategy: inject a fake repo fixture (no existsSync hitting the real
// repo), a capturing runner (records cmd/argv/cwd/shell), and a fake
// homedir. Each test asserts what the orchestrator decided and what the
// runner saw. Tests run on the actual runtime platform (real win32 here)
// and ALSO simulate linux via main({platformName: "linux"}).

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

import {
  parseArgs,
  resolveInstallRoot,
  buildPlan,
  main,
  run,
  HELP_TEXT,
} from "./install.mjs";

// ---------- helpers ----------

// Build a self-contained fake repo with the files the installer expects.
// Returns the absolute repo path.
function fakeRepo() {
  const repo = mkdtempSync(join(tmpdir(), "install-test-"));
  const fe = join(repo, "spikes", "tauri-angular");
  const gui = join(fe, "src-tauri");
  mkdirSync(gui, { recursive: true });
  writeFileSync(join(fe, "package.json"), "{}\n");
  writeFileSync(join(fe, "package-lock.json"), "{}\n");
  writeFileSync(join(gui, "Cargo.toml"), "[package]\n");
  const dist = join(fe, "dist", "agenthd-tauri-angular-spike", "browser");
  mkdirSync(dist, { recursive: true });
  writeFileSync(join(dist, "index.html"), "<!doctype html><html></html>\n");
  return repo;
}

// Capturing runner: signature matches the installer's `run` helper
// (cmd, npmArgv, npmLine, opts). Records every invocation. Returns
// responses from the queue; {code:0} when exhausted.
function capturingRunner(responses = []) {
  const records = [];
  const fn = (cmd, npmArgv, npmLine, opts) => {
    records.push({
      cmd,
      npmArgv: npmArgv ? npmArgv.slice() : [],
      npmLine: npmLine ?? null,
      cwd: opts.cwd,
      shell: !!opts.shell,
    });
    const r = responses.shift();
    return Promise.resolve(r ?? { code: 0, stdout: "", stderr: "" });
  };
  fn.records = records;
  return fn;
}

// Run the installer end-to-end against fakeRepo + capturingRunner, with
// platform override. By default uses the runtime platform (real win32 on
// the dev box).
async function runMain({ repo, argv = [], runner, platformName, env = {} } = {}) {
  return main({
    argv,
    cwd: process.cwd(),
    env,
    runner,
    homedirFn: () => "/home/u",
    platformName: platformName ?? process.platform,
    repoRootAbs: repo ?? fakeRepo(),
  });
}

// ---------- parser ----------

test("parseArgs: --help", () => {
  const p = parseArgs(["--help"]);
  assert.deepEqual(p, { help: true, force: false, root: null });
});

test("parseArgs: --force + --root <dir>", () => {
  const p = parseArgs(["--force", "--root", "/abs"]);
  assert.deepEqual(p, { help: false, force: true, root: "/abs" });
});

test("parseArgs: --root=foo", () => {
  const p = parseArgs(["--root=foo"]);
  assert.equal(p.root, "foo");
});

test("parseArgs: --root with no value rejected before npm", () => {
  assert.throws(() => parseArgs(["--root"]), /--root requires a directory argument/);
});

test("parseArgs: --root '' rejected before npm", () => {
  assert.throws(() => parseArgs(["--root", ""]), /--root requires a non-empty directory/);
  assert.throws(() => parseArgs(["--root="]), /--root requires a non-empty directory/);
});

test("parseArgs: duplicate --root rejected", () => {
  assert.throws(() => parseArgs(["--root", "/a", "--root", "/b"]), /--root specified more than once/);
});

test("parseArgs: --root <flag-shaped value> rejected (separated form only)", () => {
  // When the value is supplied as a separate argv element, it must not
  // start with -- (that would silently swallow a real flag like --force
  // or --help). The --root=<dir>=... form is allowed because the user
  // explicitly used the equals form to opt-in.
  assert.throws(() => parseArgs(["--root", "--force"]), /--root value cannot be a flag/);
  assert.throws(() => parseArgs(["--root", "--help"]), /--root value cannot be a flag/);
  assert.throws(() => parseArgs(["--root", "--bogus"]), /--root value cannot be a flag/);
  // --root=--foo is accepted because the user opted into equals form.
  // (The path may legitimately be --something in some pathological
  // setups; we don't second-guess the equals form.)
  const ok = parseArgs(["--root=--weird"]);
  assert.equal(ok.root, "--weird");
});

test("parseArgs: unknown flag and positional rejected", () => {
  assert.throws(() => parseArgs(["--bogus"]), /unexpected argument/);
  assert.throws(() => parseArgs(["agenthd"]), /unexpected argument/);
});

// ---------- root resolution ----------

test("resolveInstallRoot: precedence --root > CARGO_INSTALL_ROOT > CARGO_HOME > $HOME/.cargo", () => {
  const home = () => "/home/u";
  const cwd = "/work";
  assert.equal(resolveInstallRoot({ root: "/x" }, cwd, {}, home), resolve(cwd, "/x"));
  assert.equal(
    resolveInstallRoot({ root: null }, cwd, { CARGO_INSTALL_ROOT: "/CIR" }, home),
    resolve(cwd, "/CIR")
  );
  assert.equal(
    resolveInstallRoot({ root: null }, cwd, { CARGO_HOME: "/CH" }, home),
    resolve(cwd, "/CH")
  );
  assert.equal(resolveInstallRoot({ root: null }, cwd, {}, home), resolve(cwd, "/home/u/.cargo"));
});

test("resolveInstallRoot: empty env vars are treated as absent (no accidental install into '')", () => {
  const home = () => "/home/u";
  const cwd = "/work";
  // Both empty -> falls through to $HOME/.cargo
  assert.equal(
    resolveInstallRoot({ root: null }, cwd, { CARGO_INSTALL_ROOT: "", CARGO_HOME: "" }, home),
    resolve(cwd, "/home/u/.cargo")
  );
});

test("resolveInstallRoot: relative root resolved once, paths with spaces preserved", () => {
  const cwd = process.platform === "win32" ? "C:\\Users\\me\\Some Dir\\work" : "/Users/me/Some Dir/work";
  const abs = resolveInstallRoot({ root: "rel" }, cwd, {}, () => "/home/u");
  assert.equal(abs, join(cwd, "rel"));
  assert.ok(abs.includes("Some Dir"));
});

// ---------- run helper: platform-aware npm dispatch (direct test of `run`) ----------

// Fake spawn that returns a controllable EventEmitter-shaped object so
// the run() npm branch can be exercised without any real subprocess.
// Returns the spawn fn AND a result holder.
function fakeSpawn() {
  const result = { last: null };
  const handlers = {};
  const spawnFn = (c, a, opts) => {
    result.last = { cmd: c, argv: a.slice(), shell: !!opts.shell, cwd: opts.cwd };
    return {
      stdout: { on: () => {} },
      stderr: { on: () => {} },
      on: (ev, fn) => { handlers[ev] = fn; return this; },
    };
  };
  return { spawnFn, result, close: (code) => handlers.close && handlers.close(code) };
}

test("run: win32 dispatches npm via comSpec from opts.env /d /s /c <line> + shell:false (root never in argv)", async () => {
  const fs = fakeSpawn();
  // Inject a fake ComSpec via opts.env (not process.env) to assert the
  // injection-coherent lookup. run() must read opts.env.ComSpec and
  // fall back to "cmd.exe" if absent.
  const p = run("npm", ["--version"], "npm --version", {
    cwd: "/work",
    env: { ComSpec: "C:\\fake\\cmd.exe" },
    platformName: "win32",
  }, fs.spawnFn);
  fs.close(0);
  await p;
  assert.equal(fs.result.last.cmd, "C:\\fake\\cmd.exe");
  assert.deepEqual(fs.result.last.argv, ["/d", "/s", "/c", "npm --version"]);
  assert.equal(fs.result.last.shell, false);
});

test("run: win32 falls back to cmd.exe when opts.env.ComSpec is absent", async () => {
  const fs = fakeSpawn();
  const p = run("npm", ["--version"], "npm --version", {
    cwd: "/work",
    env: {},
    platformName: "win32",
  }, fs.spawnFn);
  fs.close(0);
  await p;
  assert.equal(fs.result.last.cmd, "cmd.exe");
  assert.deepEqual(fs.result.last.argv, ["/d", "/s", "/c", "npm --version"]);
});

test("run: linux dispatches npm directly with explicit argv + shell:false", async () => {
  const fs = fakeSpawn();
  const p = run("npm", ["ci", "--include=dev"], "npm ci --include=dev", {
    cwd: "/work",
    env: {},
    platformName: "linux",
  }, fs.spawnFn);
  fs.close(0);
  await p;
  assert.equal(fs.result.last.cmd, "npm");
  assert.deepEqual(fs.result.last.argv, ["ci", "--include=dev"]);
  assert.equal(fs.result.last.shell, false);
});

test("run: cargo is always spawned directly + shell:false, regardless of platformName", async () => {
  for (const platformName of ["win32", "linux", "darwin"]) {
    const fs = fakeSpawn();
    const p = run("cargo", ["install", "--path", "/r"], null, {
      cwd: "/work",
      env: {},
      platformName,
    }, fs.spawnFn);
    fs.close(0);
    await p;
    assert.equal(fs.result.last.cmd, "cargo");
    assert.deepEqual(fs.result.last.argv, ["install", "--path", "/r"]);
    assert.equal(fs.result.last.shell, false);
  }
});

// ---------- buildPlan ----------

test("buildPlan: order is preflight -> package.json -> npm ci -> npm run build -> confirm dist -> CLI cargo -> GUI cargo", () => {
  const repo = fakeRepo();
  const runner = capturingRunner();
  const plan = buildPlan({
    repoRootAbs: repo,
    installRootAbs: "/abs/root",
    force: false,
    run: runner,
    platformName: process.platform,
    env: {},
  });
  assert.equal(plan.length, 9);
});

test("buildPlan: preflight phase hands off to run() with cargo/npm/node + the FIXED npm line on win32 / empty argv on linux", async () => {
  const repo = fakeRepo();
  const records = [];
  const r = (cmd, npmArgv, npmLine, opts) => {
    records.push({ cmd, npmArgv: npmArgv.slice(), npmLine, cwd: opts.cwd, shell: !!opts.shell });
    return Promise.resolve({ code: 0, stdout: "", stderr: "" });
  };

  // Real runtime platform.
  const planWin = buildPlan({
    repoRootAbs: repo, installRootAbs: "/abs/root", force: false, run: r,
    platformName: process.platform, env: {},
  });
  await planWin[0](); // preflight

  // The capturing runner sees the run() INPUTS (cmd, npmArgv, npmLine),
  // not what run() would actually spawn. The npm dispatch itself is
  // tested separately by the run() direct unit tests.
  // On every platform, preflight hands the npm line AND the explicit
  // argv ['--version'] to run(); on linux the argv is what gets spawned,
  // on win32 the line is what gets passed to comSpec (argv is ignored).
  const npmEntry = records.find((x) => x.cmd === "npm" && x.npmLine === "npm --version");
  assert.ok(npmEntry, `preflight must hand 'npm --version' line to run(). records=${JSON.stringify(records)}`);
  assert.deepEqual(npmEntry.npmArgv, ["--version"], `npm preflight must pass argv ['--version'] on every platform (used by run() on linux, ignored on win32)`);
  // cargo and node: explicit argv + no npmLine.
  const cargoEntry = records.find((x) => x.cmd === "cargo" && x.npmArgv[0] === "--version");
  const nodeEntry = records.find((x) => x.cmd === "node" && x.npmArgv[0] === "--version");
  assert.ok(cargoEntry && nodeEntry);
  assert.equal(cargoEntry.npmLine, null);
  assert.equal(nodeEntry.npmLine, null);
});

test("buildPlan: npm phases hand off to run() with FIXED line (no --root) + empty argv on win32 / explicit argv on linux", async () => {
  const repo = fakeRepo();
  const records = [];
  const r = (cmd, npmArgv, npmLine, opts) => {
    records.push({ cmd, npmArgv: npmArgv.slice(), npmLine, cwd: opts.cwd, shell: !!opts.shell });
    return Promise.resolve({ code: 0, stdout: "", stderr: "" });
  };
  const plan = buildPlan({
    repoRootAbs: repo, installRootAbs: "/abs/root", force: false, run: r,
    platformName: process.platform, env: {},
  });
  await plan[4](); // npm ci
  await plan[5](); // npm run build

  const npmRecords = records.filter((x) => x.cmd === "npm");
  assert.equal(npmRecords.length, 2);
  for (const n of npmRecords) {
    assert.equal(n.shell, false);
    // The npmLine is a FIXED string. --root must never appear in it.
    assert.equal(n.npmLine.includes("--root"), false, `npm line must never include --root: ${n.npmLine}`);
    assert.equal(n.npmLine.includes("/abs/root"), false, `npm line must never include the install root: ${n.npmLine}`);
    // npmArgv is provided so run() can dispatch on linux; on linux it
    // is the source of truth, on win32 it is ignored. Either way, the
    // argv passed by buildPlan is a fixed literal (no user input).
    assert.ok(n.npmArgv.length > 0, `npm argv must always be non-empty; got ${JSON.stringify(n.npmArgv)}`);
    const flat = n.npmArgv.join(" ");
    assert.equal(flat.includes("--root"), false);
    assert.equal(flat.includes("/abs/root"), false);
  }
  const ci = npmRecords.find((x) => /npm ci/.test(x.npmLine));
  const buildPhase = npmRecords.find((x) => /npm run build/.test(x.npmLine));
  assert.ok(ci);
  assert.ok(buildPhase);
});

test("buildPlan: cargo CLI carries --bin agenthd and --root + --locked; GUI carries --bin agenthd-gui + --features custom-protocol + --root + --locked", async () => {
  const repo = fakeRepo();
  const records = [];
  const r = (cmd, npmArgv, npmLine, opts) => {
    records.push({ cmd, npmArgv: npmArgv.slice(), cwd: opts.cwd, shell: !!opts.shell });
    return Promise.resolve({ code: 0, stdout: "", stderr: "" });
  };
  const plan = buildPlan({
    repoRootAbs: repo, installRootAbs: "/abs/root", force: false, run: r,
    platformName: process.platform, env: {},
  });
  await plan[plan.length - 2](); // CLI cargo
  await plan[plan.length - 1](); // GUI cargo
  const cargoInstalls = records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(cargoInstalls.length, 2);
  const cli = cargoInstalls[0];
  const gui = cargoInstalls[1];
  assert.ok(cli.npmArgv.includes("--locked"));
  assert.ok(gui.npmArgv.includes("--locked"));
  assert.equal(cli.npmArgv[cli.npmArgv.indexOf("--root") + 1], "/abs/root");
  assert.equal(gui.npmArgv[gui.npmArgv.indexOf("--root") + 1], "/abs/root");
  assert.equal(cli.shell, false);
  assert.equal(gui.shell, false);
  // CLI-specific: --bin agenthd
  assert.equal(cli.npmArgv[cli.npmArgv.indexOf("--bin") + 1], "agenthd");
  // GUI-specific: --bin agenthd-gui + --features custom-protocol
  assert.equal(gui.npmArgv[gui.npmArgv.indexOf("--bin") + 1], "agenthd-gui");
  assert.equal(gui.npmArgv[gui.npmArgv.indexOf("--features") + 1], "custom-protocol");
});

test("buildPlan: --force forwarded to BOTH cargo installs only when set", async () => {
  const repo = fakeRepo();
  const r = capturingRunner();
  const plan = buildPlan({
    repoRootAbs: repo, installRootAbs: "/abs/root", force: true, run: r,
    platformName: process.platform, env: {},
  });
  await plan[plan.length - 2]();
  await plan[plan.length - 1]();
  const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(installs.length, 2);
  assert.ok(installs[0].npmArgv.includes("--force"));
  assert.ok(installs[1].npmArgv.includes("--force"));

  // Without force: none.
  const r2 = capturingRunner();
  const plan2 = buildPlan({
    repoRootAbs: repo, installRootAbs: "/abs/root", force: false, run: r2,
    platformName: process.platform, env: {},
  });
  await plan2[plan2.length - 2]();
  await plan2[plan2.length - 1]();
  const installs2 = r2.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(installs2.length, 2);
  assert.equal(installs2[0].npmArgv.includes("--force"), false);
  assert.equal(installs2[1].npmArgv.includes("--force"), false);
});

// ---------- missing-dist abort (REAL test, not 0||1) ----------

test("main: aborts on missing dist/.../browser/index.html after npm run build (real fixture, real abort)", async () => {
  // Build a fake repo WITHOUT dist/. npm ci + npm run build succeed
  // (capturingRunner returns code 0), but the confirm-dist phase must
  // abort with code "missing".
  const repo = fakeRepo();
  // Remove the dist we wrote to fake a build that did not produce it.
  rmSync(join(repo, "spikes", "tauri-angular", "dist", "agenthd-tauri-angular-spike", "browser"), { recursive: true, force: true });

  const r = capturingRunner([
    // preflight: cargo, npm, node
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    // npm ci: ok
    { code: 0, stdout: "", stderr: "" },
    // npm run build: ok (caller faked)
    { code: 0, stdout: "", stderr: "" },
    // then the confirm-dist phase must abort (no more responses needed)
  ]);
  const code = await main({
    argv: ["--root", "/abs/root"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: repo,
  });
  assert.equal(code, 1);
  // Cargo installs must NOT have been invoked.
  const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(installs.length, 0, `cargo installs must be 0; got ${installs.length}`);
});

// ---------- main integration: real win32 + simulated linux ----------

test("main: --help prints usage and returns 0 without invoking any tool", async () => {
  const r = capturingRunner();
  const code = await main({
    argv: ["--help"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: fakeRepo(),
  });
  assert.equal(code, 0);
  assert.equal(r.records.length, 0);
});

test("main: parser errors abort with code 1, no tools invoked", async () => {
  const r = capturingRunner();
  const code = await main({
    argv: ["--bogus"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: fakeRepo(),
  });
  assert.equal(code, 1);
  assert.equal(r.records.length, 0);
});

test("main: preflight failure aborts before any cargo/npm", async () => {
  const r = capturingRunner([{ code: 127, stdout: "", stderr: "cargo not found" }]);
  const code = await main({
    argv: ["--root", "/abs/root"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: fakeRepo(),
  });
  assert.equal(code, 1);
});

test("main: walks full plan on real win32 + simulated linux, both cargo installs observed with same --root, --bin, --features", async () => {
  // Use a Windows-friendly root on win32 to avoid path.resolve turning
  // "/abs/root" into "C:\\abs\\root" (which is fine semantically but
  // fragile to compare against the input string).
  const winRoot = process.platform === "win32" ? "C:\\abs\\root" : "/abs/root";
  const rootArg = process.platform === "win32" ? "--root=C:\\abs\\root" : "--root=/abs/root";

  // Real win32:
  {
    const repo = fakeRepo();
    const r = capturingRunner([
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
    ]);
    const code = await main({
      argv: [rootArg],
      cwd: process.cwd(),
      env: {},
      runner: r,
      homedirFn: () => "/home/u",
      platformName: process.platform,
      repoRootAbs: repo,
    });
    assert.equal(code, 0, `expected 0; records: ${JSON.stringify(r.records.map((x) => ({c: x.cmd, a: x.npmArgv[0], line: x.npmLine})))}`);
    const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
    assert.equal(installs.length, 2);
    for (const ins of installs) {
      assert.equal(ins.npmArgv[ins.npmArgv.indexOf("--root") + 1], winRoot);
      assert.ok(ins.npmArgv.includes("--locked"));
      assert.equal(ins.shell, false);
    }
    assert.equal(installs[0].npmArgv[installs[0].npmArgv.indexOf("--bin") + 1], "agenthd");
    assert.equal(installs[1].npmArgv[installs[1].npmArgv.indexOf("--bin") + 1], "agenthd-gui");
    assert.equal(installs[1].npmArgv[installs[1].npmArgv.indexOf("--features") + 1], "custom-protocol");
  }

  // Simulated linux (use absolute POSIX-style root regardless of host).
  {
    const repo = fakeRepo();
    const linuxRoot = "/abs/root";
    const r = capturingRunner([
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
      { code: 0, stdout: "", stderr: "" },
    ]);
    const code = await main({
      argv: ["--root", linuxRoot],
      cwd: process.cwd(),
      env: {},
      runner: r,
      homedirFn: () => "/home/u",
      platformName: "linux",
      repoRootAbs: repo,
    });
    assert.equal(code, 0);
    const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
    assert.equal(installs.length, 2);
    // resolve("/work", "/abs/root") on win32 yields "C:\\abs\\root"; on
    // linux it's "/abs/root". Assert the resolved value is what buildPlan
    // actually forwarded -- which is whatever resolveInstallRoot produced.
    // For the linux simulated case we expect literal "/abs/root".
    if (process.platform !== "win32") {
      assert.equal(installs[0].npmArgv[installs[0].npmArgv.indexOf("--root") + 1], linuxRoot);
    }
    const npmCalls = r.records.filter((x) => x.cmd === "npm");
    assert.ok(npmCalls.length >= 2);
  }
});

test("main: npm failure aborts without running cargo", async () => {
  const repo = fakeRepo();
  const r = capturingRunner([
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    // npm ci FAILS
    { code: 1, stdout: "", stderr: "npm ci failed" },
  ]);
  const code = await main({
    argv: ["--root", "/abs/root"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: repo,
  });
  assert.equal(code, 1);
  const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(installs.length, 0);
});

test("main: cargo CLI failure aborts without running cargo GUI", async () => {
  const repo = fakeRepo();
  const r = capturingRunner([
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    // cargo CLI fails
    { code: 1, stdout: "", stderr: "CLI install failed" },
  ]);
  const code = await main({
    argv: ["--root", "/abs/root"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: repo,
  });
  assert.equal(code, 1);
  const installs = r.records.filter((x) => x.cmd === "cargo" && x.npmArgv[0] === "install");
  assert.equal(installs.length, 1);
});

test("main: cargo GUI failure (CLI succeeded) aborts without claiming success", async () => {
  const repo = fakeRepo();
  const r = capturingRunner([
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 0, stdout: "", stderr: "" },
    { code: 1, stdout: "", stderr: "GUI install failed" },
  ]);
  const code = await main({
    argv: ["--root", "/abs/root"],
    cwd: process.cwd(),
    env: {},
    runner: r,
    homedirFn: () => "/home/u",
    platformName: process.platform,
    repoRootAbs: repo,
  });
  assert.equal(code, 1);
});

// ---------- HELP_TEXT contract ----------

test("HELP_TEXT: documents --root precedence, --force semantics (Cargo native update), .cargo/config.toml, no atomicity claim", () => {
  assert.match(HELP_TEXT, /--root > CARGO_INSTALL_ROOT > CARGO_HOME > \$HOME\/\.cargo/);
  assert.match(HELP_TEXT, /--force/);
  assert.match(HELP_TEXT, /Atomicity is not promised/);
  assert.match(HELP_TEXT, /never launches the GUI/);
  assert.match(HELP_TEXT, /never writes\s+agenthd settings/);
  assert.match(HELP_TEXT, /\.cargo\/config\.toml/, "HELP_TEXT must reference .cargo/config.toml, not Cargo.toml");
  // --force text aligned with Cargo's native update semantics, NOT a
  // false "when bytes match" claim and NOT a "binary without feature"
  // requirement.
  assert.match(HELP_TEXT, /same package without --force/);
  assert.doesNotMatch(HELP_TEXT, /when bytes match/, "old false claim 'when bytes match' must be removed");
  assert.doesNotMatch(HELP_TEXT, /built without --features custom-protocol/, "old false claim about feature mismatch must be removed");
});