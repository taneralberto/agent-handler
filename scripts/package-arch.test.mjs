import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { packageArch, parseArgs } from "./package-arch.mjs";

function fixture(t) {
  const base = mkdtempSync("/tmp/opencode/package-arch-test-");
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const sourceRoot = join(base, "source");
  mkdirSync(sourceRoot);
  const put = (path, bytes = "fixture\n") => {
    mkdirSync(dirname(join(sourceRoot, path)), { recursive: true });
    writeFileSync(join(sourceRoot, path), bytes);
  };
  for (const path of ["Cargo.lock", "LICENSE", "src/lib.rs", "src/main.rs",
    "spikes/tauri-angular/package-lock.json", "spikes/tauri-angular/angular.json",
    "spikes/tauri-angular/tsconfig.json", "spikes/tauri-angular/tsconfig.app.json",
    "spikes/tauri-angular/src/index.html", "spikes/tauri-angular/src/main.ts",
    "spikes/tauri-angular/src-tauri/Cargo.lock", "spikes/tauri-angular/src-tauri/build.rs",
    "spikes/tauri-angular/src-tauri/tauri.conf.json", "spikes/tauri-angular/src-tauri/src/main.rs",
    "spikes/tauri-angular/src-tauri/src/lib.rs"]) put(path);
  put("Cargo.toml", '[package]\nversion = "0.1.0"\n');
  put("spikes/tauri-angular/src-tauri/Cargo.toml", '[package]\nversion = "0.1.0"\n');
  put("spikes/tauri-angular/package.json", '{"version":"0.1.0"}\n');
  put("packaging/arch/PKGBUILD.in", readFileSync(new URL("../packaging/arch/PKGBUILD.in", import.meta.url)));
  execFileSync("git", ["init", "--quiet", sourceRoot]);
  execFileSync("git", ["-C", sourceRoot, "add", "."]);
  return { base, sourceRoot, outputDir: join(base, "out"), put };
}

for (const { name, flags, encoded, expected } of [
  { name: "no caller flags", expected: [] },
  { name: "plain caller flags and spaces", flags: "-C opt-level=2\n--cfg caller", expected: ["-C", "opt-level=2", "--cfg", "caller"] },
  { name: "encoded caller flags and spaces", flags: "--cfg ignored_by_cargo", encoded: "--cfg\x1fcaller=\"with spaces\"", expected: ["--cfg", 'caller="with spaces"'] },
  { name: "empty encoded flags retain Cargo precedence", flags: "--cfg ignored_by_cargo", encoded: "", expected: [] },
]) {
  test(`package-arch build exports remap for both binaries: ${name}`, (t) => {
    const f = fixture(t);
    const srcdir = join(f.base, "build sources with spaces");
    const bin = join(f.base, "fake-bin");
    const capture = join(f.base, "cargo.jsonl");
    mkdirSync(join(srcdir, "agenthd-0.1.0/spikes/tauri-angular/dist/agenthd-tauri-angular-spike/browser"), { recursive: true });
    writeFileSync(join(srcdir, "agenthd-0.1.0/spikes/tauri-angular/dist/agenthd-tauri-angular-spike/browser/index.html"), "fixture");
    mkdirSync(bin);
    writeFileSync(join(bin, "npm"), "#!/bin/bash\nexit 0\n");
    writeFileSync(join(bin, "cargo"), `#!${process.execPath}
import { appendFileSync } from "node:fs";
appendFileSync(process.env.CAPTURE, JSON.stringify({ args: process.argv.slice(2), flags: process.env.CARGO_ENCODED_RUSTFLAGS.split("\\x1f"), plain: process.env.RUSTFLAGS, target: process.env.CARGO_TARGET_DIR }) + "\\n");
`);
    chmodSync(join(bin, "npm"), 0o755);
    chmodSync(join(bin, "cargo"), 0o755);
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, CAPTURE: capture };
    delete env.RUSTFLAGS;
    delete env.CARGO_ENCODED_RUSTFLAGS;
    if (flags !== undefined) env.RUSTFLAGS = flags;
    if (encoded !== undefined) env.CARGO_ENCODED_RUSTFLAGS = encoded;
    execFileSync("bash", ["-e", "-c", 'source "$1"; srcdir="$2"; build', "fixture", join(f.sourceRoot, "packaging/arch/PKGBUILD.in"), srcdir], { env });
    const calls = readFileSync(capture, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    assert.equal(calls.length, 2);
    assert.deepEqual(calls[0].args, ["build", "--release", "--locked", "--bin", "agenthd"]);
    assert.deepEqual(calls[1].args, ["build", "--release", "--locked", "--manifest-path", "spikes/tauri-angular/src-tauri/Cargo.toml", "--bin", "agenthd-gui", "--features", "custom-protocol"]);
    for (const call of calls) {
      assert.deepEqual(call.flags, [...expected, `--remap-path-prefix=${srcdir}=/usr/src/debug/agenthd`]);
      assert.equal(call.plain, flags);
      assert.equal(call.target, join(srcdir, "cargo-target"));
    }
  });
}

test("package-arch lists without writing and snapshots modified tracked and new working-tree bytes", (t) => {
  const f = fixture(t);
  f.put("src/lib.rs", "modified tracked bytes\n");
  f.put("src/new-production.rs", "new uncommitted bytes\n");
  f.put("spikes/tauri-angular/src/app/new.ts", "new GUI source\n");
  const { files } = packageArch({ ...f, list: true });
  assert.ok(files.includes("src/new-production.rs"));
  assert.ok(files.includes("spikes/tauri-angular/src/app/new.ts"));
  assert.equal(existsSync(f.outputDir), false);
  const result = packageArch(f);
  assert.deepEqual(readdirSync(f.outputDir).sort(), result.artifacts.map((path) => path.slice(f.outputDir.length + 1)).sort());
  const archive = result.artifacts[0];
  assert.equal(execFileSync("tar", ["-xOf", archive, "agenthd-0.1.0/src/lib.rs"], { encoding: "utf8" }), "modified tracked bytes\n");
  assert.equal(execFileSync("tar", ["-xOf", archive, "agenthd-0.1.0/src/new-production.rs"], { encoding: "utf8" }), "new uncommitted bytes\n");
  const members = execFileSync("tar", ["-tzf", archive], { encoding: "utf8" });
  assert.ok(members.includes("agenthd-0.1.0/packaging/arch/PKGBUILD.in"));
  assert.ok(!members.split("\n").includes("agenthd-0.1.0/PKGBUILD"));
});

test("package-arch includes the root npm entrypoint and optional lockfile in the archive", (t) => {
  const f = fixture(t);
  const manifest = readFileSync(new URL("../package.json", import.meta.url), "utf8");
  f.put("package.json", manifest);
  f.put("package-lock.json", '{"lockfileVersion":3}\n');
  const { files } = packageArch({ ...f, list: true });
  assert.ok(files.includes("package.json"));
  assert.ok(files.includes("package-lock.json"));
  const { artifacts } = packageArch(f);
  const archived = execFileSync("tar", ["-xOf", artifacts[0], "agenthd-0.1.0/package.json"], { encoding: "utf8" });
  assert.equal(archived, manifest);
  assert.equal(JSON.parse(archived).scripts["install:global"], "node scripts/install.mjs --force");
});

test("package-arch excludes caches, logs, secrets, environment and temporary files even when tracked", (t) => {
  const f = fixture(t);
  const excluded = ["target/data", "node_modules/pkg/file", "spikes/tauri-angular/dist/index.html",
    "spikes/tauri-angular/.angular/cache/data", ".npm/data", "src/cache/data", "logs/run.log",
    ".env", ".env.local", "credentials.json", "secrets/token", "src/cert.pem",
    "src/file.tmp", "src/file.swp", "src/file~"];
  for (const path of excluded) f.put(path);
  execFileSync("git", ["-C", f.sourceRoot, "add", "."]);
  const { files } = packageArch({ ...f, list: true });
  for (const path of excluded) assert.ok(!files.includes(path), path);
});

test("package-arch refuses source symlinks and special files", (t) => {
  const f = fixture(t);
  symlinkSync("/etc/os-release", join(f.sourceRoot, "src/external.rs"));
  assert.throws(() => packageArch({ ...f, list: true }), /symlink or special/);
  rmSync(join(f.sourceRoot, "src/external.rs"));
  f.put("src/pipe.rs", "tracked regular file\n");
  execFileSync("git", ["-C", f.sourceRoot, "add", "src/pipe.rs"]);
  rmSync(join(f.sourceRoot, "src/pipe.rs"));
  execFileSync("mkfifo", [join(f.sourceRoot, "src/pipe.rs")]);
  assert.throws(() => packageArch({ ...f, list: true }), /symlink or special/);
  assert.equal(existsSync(f.outputDir), false);
});

test("package-arch deterministic archive, actual SHA256, changed-source hash and valid Bash recipe", (t) => {
  const f = fixture(t);
  const first = packageArch(f);
  let second;
  const previousMask = process.umask(0o077);
  try {
    second = packageArch({ ...f, outputDir: join(f.base, "out-two") });
  } finally {
    process.umask(previousMask);
  }
  assert.equal(first.sha256, second.sha256);
  assert.deepEqual(readFileSync(first.artifacts[0]), readFileSync(second.artifacts[0]));
  const actual = createHash("sha256").update(readFileSync(first.artifacts[0])).digest("hex");
  assert.equal(first.sha256, actual);
  assert.equal(readFileSync(first.artifacts[2], "utf8"), `${actual}  agenthd-0.1.0.tar.gz\n`);
  const recipe = readFileSync(first.artifacts[1], "utf8");
  assert.ok(recipe.includes(`sha256sums=('${actual}')`));
  assert.ok(!recipe.includes("SKIP") && !recipe.includes("@SOURCE_SHA256@"));
  execFileSync("bash", ["-n", first.artifacts[1]]);
  f.put("src/lib.rs", "changed source\n");
  assert.notEqual(packageArch({ ...f, outputDir: join(f.base, "out-three") }).sha256, actual);
});

test("package-arch requires explicit absolute arguments", () => {
  assert.throws(() => parseArgs([]), /Usage/);
  assert.throws(() => parseArgs(["--source-root", "."]), /absolute/);
  assert.throws(() => parseArgs(["--source-root", "/source", "--output-dir", "out"]), /absolute/);
  assert.throws(() => parseArgs(["--unknown"]), /unexpected/);
  assert.deepEqual(parseArgs(["--source-root", "/source", "--output-dir", "/tmp/opencode/out", "--list"]),
    { sourceRoot: "/source", outputDir: "/tmp/opencode/out", list: true });
});

test("package-arch validates output root, source separation and symlink redirection without writing", (t) => {
  const f = fixture(t);
  for (const outputDir of ["relative", "/tmp/out-forbidden", "/tmp/opencode", f.sourceRoot, join(f.sourceRoot, "out")]) {
    assert.throws(() => packageArch({ ...f, outputDir, list: true }), /absolute|output must/);
  }
  symlinkSync(f.sourceRoot, join(f.base, "redirect"));
  assert.throws(() => packageArch({ ...f, outputDir: join(f.base, "redirect/out"), list: true }), /output must/);
  assert.equal(existsSync(join(f.sourceRoot, "out")), false);
});

test("package-arch never overwrites any existing output artifact", (t) => {
  const f = fixture(t);
  for (const artifact of ["PKGBUILD", "agenthd-0.1.0.tar.gz", "agenthd-0.1.0.tar.gz.sha256"]) {
    const outputDir = join(f.base, artifact.replaceAll(".", "-"));
    mkdirSync(outputDir);
    writeFileSync(join(outputDir, artifact), "keep me");
    assert.throws(() => packageArch({ ...f, outputDir }), /output already exists/);
    assert.equal(readFileSync(join(outputDir, artifact), "utf8"), "keep me");
  }
});

test("package-arch refuses existing directories but listing never writes to them", (t) => {
  const f = fixture(t);
  mkdirSync(f.outputDir);
  assert.throws(() => packageArch(f), /output already exists/);
  assert.ok(packageArch({ ...f, list: true }).files.includes("src/lib.rs"));
  assert.deepEqual(readdirSync(f.outputDir), []);
  writeFileSync(join(f.outputDir, "user-content"), "keep me");
  assert.throws(() => packageArch(f), /output already exists/);
  packageArch({ ...f, list: true });
  assert.deepEqual(readdirSync(f.outputDir), ["user-content"]);
  assert.equal(readFileSync(join(f.outputDir, "user-content"), "utf8"), "keep me");
  assert.deepEqual(readdirSync(f.base).sort(), ["out", "source"]);
});

test("package-arch cleans failed post-archive staging and can retry publication", (t) => {
  const f = fixture(t);
  const template = readFileSync(join(f.sourceRoot, "packaging/arch/PKGBUILD.in"), "utf8");
  f.put("packaging/arch/PKGBUILD.in", template.replace("@SOURCE_SHA256@", "invalid-token"));
  assert.throws(() => packageArch(f), /invalid checksum template/);
  assert.equal(existsSync(f.outputDir), false);
  assert.deepEqual(readdirSync(f.base), ["source"]);
  f.put("packaging/arch/PKGBUILD.in", template);
  const result = packageArch(f);
  assert.equal(result.artifacts.length, 3);
  assert.deepEqual(readdirSync(f.base).sort(), ["out", "source"]);
  assert.deepEqual(readdirSync(f.outputDir).sort(), ["PKGBUILD", "agenthd-0.1.0.tar.gz", "agenthd-0.1.0.tar.gz.sha256"]);
});

test("package-arch fails closed for unsupported files and missing required sources", (t) => {
  const f = fixture(t);
  f.put("spikes/tauri-angular/new-build.config", "new build input\n");
  assert.throws(() => packageArch({ ...f, list: true }), /unsupported source file/);
  rmSync(join(f.sourceRoot, "spikes/tauri-angular/new-build.config"));
  rmSync(join(f.sourceRoot, "LICENSE"));
  assert.throws(() => packageArch({ ...f, list: true }));
  assert.equal(existsSync(f.outputDir), false);
});
