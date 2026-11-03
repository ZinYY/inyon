#!/usr/bin/env node
// Runs the inyon program from the package npm picked for this system
// (@abhash-chakraborty/inyon-<platform>-<arch>). Nothing runs at install time.
"use strict";

const { spawnSync } = require("node:child_process");
const { dirname, join } = require("node:path");

const pkg = `@abhash-chakraborty/inyon-${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "inyon.exe" : "inyon";

let bin;
try {
  bin = join(dirname(require.resolve(`${pkg}/package.json`)), "bin", exe);
} catch {
  console.error(
    `inyon: ${pkg} isn't installed. There's no build for ${process.platform} ${process.arch}, ` +
      "or optional dependencies were skipped (--omit=optional). Reinstall without that, or use " +
      "the installer at https://inyon.tech/docs/install/",
  );
  process.exit(1);
}

const run = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error(`inyon: couldn't run ${bin}: ${run.error.message}`);
  process.exit(1);
}
if (run.signal) process.kill(process.pid, run.signal);
process.exit(run.status ?? 1);
