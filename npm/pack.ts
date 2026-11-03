// Builds the npm packages for one release: @abhash-chakraborty/inyon (a small launcher) and one
// package per system holding that release's `inyon` program. The programs come from the GitHub
// release and are only used after SHA256SUMS carries a good signature from the release key and
// each file matches it.
//
//   bun npm/pack.ts v0.16.1 <out dir>
//
// .github/workflows/npm.yml publishes what this writes.
import { createHash } from "node:crypto";
import { chmodSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

import { minisignOk, RELEASE_KEY } from "../site/build.ts";

const REPO = "Abhash-Chakraborty/inyon";
const SCOPE = "@abhash-chakraborty";

/** npm's names for each system, and the release file built for it. */
export const TARGETS = [
  { os: "linux", cpu: "x64", file: "inyon-linux-x64" },
  { os: "linux", cpu: "arm64", file: "inyon-linux-arm64" },
  { os: "darwin", cpu: "x64", file: "inyon-darwin-x64" },
  { os: "darwin", cpu: "arm64", file: "inyon-darwin-arm64" },
  { os: "win32", cpu: "x64", file: "inyon-windows-x64.exe" },
  { os: "win32", cpu: "arm64", file: "inyon-windows-arm64.exe" },
] as const;

const OS_NAME = { linux: "Linux", darwin: "macOS", win32: "Windows" } as const;

const sha256 = (b: Buffer) => createHash("sha256").update(b).digest("hex");

async function download(tag: string, name: string): Promise<Buffer> {
  const url = `https://github.com/${REPO}/releases/download/${tag}/${name}`;
  const r = await fetch(url, { redirect: "follow" });
  if (!r.ok) throw new Error(`${name}: ${r.status} from ${url}`);
  return Buffer.from(await r.arrayBuffer());
}

/** The checksum SHA256SUMS lists for `name` (sha256sum format, "*" marks binary mode). */
function sumFor(sums: string, name: string): string | undefined {
  for (const line of sums.split("\n")) {
    const [hash, file] = line.trim().split(/\s+/);
    if (file === name || file === `*${name}`) return hash;
  }
}

async function main() {
  const [tag, out] = process.argv.slice(2);
  if (!tag?.match(/^v\d+\.\d+\.\d+$/) || !out) {
    console.error("usage: bun npm/pack.ts v<major.minor.patch> <out dir>");
    process.exit(2);
  }
  const version = tag.slice(1);

  const sumsBytes = await download(tag, "SHA256SUMS");
  const sig = (await download(tag, "SHA256SUMS.minisig")).toString("utf8");
  if (!minisignOk(sumsBytes, sig, RELEASE_KEY)) throw new Error(`${tag}: SHA256SUMS isn't signed by the release key`);
  const sums = sumsBytes.toString("utf8");

  rmSync(out, { recursive: true, force: true });
  const root = join(import.meta.dir, "..");
  const license = join(root, "LICENSE");
  const optional: Record<string, string> = {};

  for (const t of TARGETS) {
    const want = sumFor(sums, t.file);
    if (!want) throw new Error(`${tag}: SHA256SUMS has no ${t.file}`);
    const bytes = await download(tag, t.file);
    if (sha256(bytes) !== want) throw new Error(`${t.file} doesn't match SHA256SUMS`);

    const name = `${SCOPE}/inyon-${t.os}-${t.cpu}`;
    const dir = join(out, `inyon-${t.os}-${t.cpu}`);
    const exe = t.os === "win32" ? "inyon.exe" : "inyon";
    mkdirSync(join(dir, "bin"), { recursive: true });
    writeFileSync(join(dir, "bin", exe), bytes);
    chmodSync(join(dir, "bin", exe), 0o755);
    cpSync(license, join(dir, "LICENSE"));
    const pkg = {
      name,
      version,
      description: `The inyon command for ${OS_NAME[t.os]} on ${t.cpu}. Install ${SCOPE}/inyon instead of this one.`,
      homepage: "https://inyon.tech",
      repository: { type: "git", url: `git+https://github.com/${REPO}.git`, directory: "npm" },
      license: "MIT",
      author: "Abhash Chakraborty",
      os: [t.os],
      cpu: [t.cpu],
      files: [`bin/${exe}`],
      preferUnplugged: true,
      publishConfig: { access: "public", provenance: true },
    };
    writeFileSync(join(dir, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
    writeFileSync(
      join(dir, "README.md"),
      `# ${name}\n\nThe \`inyon\` command from [Inyon ${version}](https://github.com/${REPO}/releases/tag/${tag}) ` +
        `for ${OS_NAME[t.os]} on ${t.cpu}, the file \`${t.file}\` with SHA-256 \`${want}\`.\n\n` +
        `Install [\`${SCOPE}/inyon\`](https://www.npmjs.com/package/${SCOPE}/inyon) instead; it picks this package on its own.\n`,
    );
    optional[name] = version;
    console.log(`${name}@${version}  ${t.file}  ${want}`);
  }

  // the program has to say it's this version, so an older release's signed files can't pass as this one
  const here = TARGETS.find((t) => t.os === process.platform && t.cpu === process.arch);
  if (!here) throw new Error(`can't run any of the programs on ${process.platform} ${process.arch} to check their version`);
  const exe = join(out, `inyon-${here.os}-${here.cpu}`, "bin", here.os === "win32" ? "inyon.exe" : "inyon");
  const said = spawnSync(exe, ["--version"], { encoding: "utf8", env: { ...process.env, HOME: tmpdir() } });
  if (said.stdout?.trim() !== `inyon ${version}`) {
    throw new Error(`${here.file} says "${said.stdout?.trim() || said.stderr?.trim()}", not inyon ${version}`);
  }

  const main = join(out, "inyon");
  cpSync(join(import.meta.dir, "inyon"), main, { recursive: true });
  cpSync(license, join(main, "LICENSE"));
  const pkg = JSON.parse(readFileSync(join(main, "package.json"), "utf8"));
  pkg.version = version;
  pkg.optionalDependencies = optional;
  writeFileSync(join(main, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
  console.log(`${SCOPE}/inyon@${version}`);
}

if (import.meta.main) await main();
