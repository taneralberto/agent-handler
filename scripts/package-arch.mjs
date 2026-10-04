#!/usr/bin/env node
// Local working-tree snapshot, not a release publisher or package builder.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { constants, copyFileSync, existsSync, lstatSync, mkdirSync, mkdtempSync,
  readFileSync, realpathSync, rmSync, writeFileSync, chmodSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const NAME = "agenthd-0.1.0";
const REQUIRED = ["Cargo.toml", "Cargo.lock", "LICENSE", "src/lib.rs", "src/main.rs",
  "packaging/arch/PKGBUILD.in", "spikes/tauri-angular/package.json",
  "spikes/tauri-angular/package-lock.json", "spikes/tauri-angular/angular.json",
  "spikes/tauri-angular/tsconfig.json", "spikes/tauri-angular/tsconfig.app.json",
  "spikes/tauri-angular/src/index.html", "spikes/tauri-angular/src/main.ts",
  "spikes/tauri-angular/src-tauri/Cargo.toml", "spikes/tauri-angular/src-tauri/Cargo.lock",
  "spikes/tauri-angular/src-tauri/build.rs", "spikes/tauri-angular/src-tauri/tauri.conf.json",
  "spikes/tauri-angular/src-tauri/src/main.rs", "spikes/tauri-angular/src-tauri/src/lib.rs"];

export function parseArgs(argv) {
  const options = { list: false };
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    if (flag === "--list" && !options.list) options.list = true;
    else if (flag === "--source-root" || flag === "--output-dir") {
      const key = flag === "--source-root" ? "sourceRoot" : "outputDir";
      const value = argv[++i];
      if (!value || !isAbsolute(value) || options[key]) throw new Error(`${flag} requires one absolute path`);
      options[key] = value;
    } else throw new Error(`unexpected argument: ${flag}`);
  }
  if (!options.sourceRoot || !options.outputDir) {
    throw new Error("Usage: node scripts/package-arch.mjs --source-root <absolute> --output-dir <absolute under /tmp/opencode> [--list]");
  }
  return options;
}

function inside(root, path) {
  const suffix = relative(root, path);
  return suffix !== "" && suffix !== ".." && !suffix.startsWith("../") && !isAbsolute(suffix);
}

function excluded(path) {
  return path.split("/").some((part) =>
    /^(\.git|target|node_modules|dist|\.angular|\.npm|\.?cache|.*-cache|logs?|tmp|temp|credentials?|secrets?)$/i.test(part)
    || /^\.env(?:\.|$)/i.test(part)
    || /(?:\.(?:log|tmp|temp|swp|swo|bak|pem|key|p12|pfx)|~)$/i.test(part)
    || /^(?:credentials?|secrets?)(?:[._-]|$)/i.test(part));
}

function supported(path) {
  if (/^(src|tests|agents|skills|scripts|packaging|docs)\//.test(path)) return true;
  if (/^(Cargo\.(toml|lock)|package(-lock)?\.json|LICENSE|\.gitignore|[^/]+\.md)$/.test(path)) return true;
  const prefix = "spikes/tauri-angular/";
  if (!path.startsWith(prefix)) return false;
  const local = path.slice(prefix.length);
  return /^(src|public)\//.test(local)
    || /^(src-tauri\/(src|icons|capabilities)\/)/.test(local)
    || /^(src-tauri\/)?(\.gitignore|Cargo\.(toml|lock)|build\.rs|tauri\.conf\.json)$/.test(local)
    || /^(package(-lock)?\.json|angular\.json|tsconfig[^/]*\.json|[^/]+\.(md|mjs))$/.test(local);
}

export function snapshotFiles(sourceRoot) {
  const paths = execFileSync("git", ["-C", sourceRoot, "ls-files", "--cached", "--others", "--exclude-standard", "-z"], { encoding: "utf8" }).split("\0").filter(Boolean);
  const files = [...new Set(paths)].filter((path) => !excluded(path)).sort();
  for (const path of files) {
    if (path.includes("\n") || path.includes("\r") || !inside(sourceRoot, resolve(sourceRoot, path))) throw new Error(`unsafe source path: ${path}`);
    // Check every component: a regular file beneath a symlink is not local source.
    let current = sourceRoot;
    const parts = path.split("/");
    for (const [index, part] of parts.entries()) {
      current = join(current, part);
      const stat = lstatSync(current);
      if (stat.isSymbolicLink() || (index === parts.length - 1 ? !stat.isFile() : !stat.isDirectory())) {
        throw new Error(`symlink or special source file refused: ${path}`);
      }
    }
    if (!supported(path)) throw new Error(`unsupported source file (review snapshot scope): ${path}`);
  }
  for (const path of REQUIRED) if (!files.includes(path)) throw new Error(`required source file missing: ${path}`);
  for (const path of ["Cargo.toml", "spikes/tauri-angular/src-tauri/Cargo.toml"]) {
    if (!/^version\s*=\s*"0\.1\.0"\s*$/m.test(readFileSync(join(sourceRoot, path), "utf8"))) throw new Error(`unsupported package version: ${path}`);
  }
  if (JSON.parse(readFileSync(join(sourceRoot, "spikes/tauri-angular/package.json"), "utf8")).version !== "0.1.0") throw new Error("unsupported frontend version");
  return files;
}

export function packageArch({ sourceRoot, outputDir, list = false }) {
  if (!isAbsolute(sourceRoot) || !isAbsolute(outputDir)) throw new Error("source and output paths must be absolute");
  sourceRoot = realpathSync(sourceRoot);
  outputDir = resolve(outputDir);
  // Resolve the existing ancestor without creating anything (including in --list).
  let ancestor = outputDir;
  while (!existsSync(ancestor)) ancestor = dirname(ancestor);
  const physicalOutput = resolve(realpathSync(ancestor), relative(ancestor, outputDir));
  const approvedRoot = realpathSync("/tmp/opencode");
  if (!inside(approvedRoot, outputDir) || !inside(approvedRoot, physicalOutput)
    || physicalOutput !== outputDir || outputDir === sourceRoot || inside(sourceRoot, outputDir)) {
    throw new Error("output must be outside source, strictly under /tmp/opencode, without symlink redirection");
  }
  const files = snapshotFiles(sourceRoot);
  if (list) return { files };
  const artifacts = [`${NAME}.tar.gz`, "PKGBUILD", `${NAME}.tar.gz.sha256`];
  if (lstatSync(outputDir, { throwIfNoEntry: false })) throw new Error(`output already exists: ${outputDir}`);
  mkdirSync(dirname(outputDir), { recursive: true });
  const stage = mkdtempSync(join(dirname(outputDir), ".snapshot-"));
  try {
    const tree = join(stage, NAME);
    mkdirSync(tree, { mode: 0o755 });
    for (const path of files) {
      const destination = join(tree, path);
      mkdirSync(dirname(destination), { recursive: true, mode: 0o755 });
      copyFileSync(join(sourceRoot, path), destination, constants.COPYFILE_EXCL);
      chmodSync(destination, 0o644);
    }
    execFileSync("tar", ["--sort=name", "--mtime=@0", "--owner=0", "--group=0", "--numeric-owner",
      "--mode=u=rwX,go=rX", "--format=gnu", "--use-compress-program=gzip -n",
      "-cf", join(stage, artifacts[0]), "-C", stage, NAME]);
    const sha256 = createHash("sha256").update(readFileSync(join(stage, artifacts[0]))).digest("hex");
    const template = readFileSync(join(tree, "packaging/arch/PKGBUILD.in"), "utf8");
    if (template.split("@SOURCE_SHA256@").length !== 2 || template.includes("SKIP")) throw new Error("invalid checksum template");
    writeFileSync(join(stage, "PKGBUILD"), template.replace("@SOURCE_SHA256@", sha256));
    writeFileSync(join(stage, artifacts[2]), `${sha256}  ${artifacts[0]}\n`);
    rmSync(tree, { recursive: true });
    // Sibling staging keeps publication on one filesystem; GNU mv refuses even
    // an empty destination created after the initial check, without replacing it.
    execFileSync("mv", ["--no-clobber", "--no-target-directory", "--", stage, outputDir]);
    if (existsSync(stage)) throw new Error(`output already exists: ${outputDir}`);
    return { files, sha256, artifacts: artifacts.map((name) => join(outputDir, name)) };
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const result = packageArch(parseArgs(process.argv.slice(2)));
    console.log(result.artifacts ? result.artifacts.join("\n") : result.files.join("\n"));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
