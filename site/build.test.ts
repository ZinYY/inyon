import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stampInstaller } from "../installers/installers.ts";
import { CSP, checkReleaseFile, downloadTable, mb, oldSiteLinks, parseSums, securityTxt } from "./build.ts";

// /download said the APK was 2.7 MB while Chrome said 2.88 MB: sizes are decimal, as browsers count
test("sizes read as a browser shows them", () => {
  expect(mb(2_883_584)).toBe("2.9 MB");
  expect(mb(85_000_000)).toBe("85.0 MB");
  expect(mb(950_000)).toBe("950 KB");
  expect(mb(12)).toBe("1 KB");
});


const sha = (b: string | Buffer) => createHash("sha256").update(b).digest("hex");
const node = (f: string) => readFileSync(join(import.meta.dir, "../installers", f), "utf8");

// the site publishes a release file only when the signed SHA256SUMS lists it with that checksum
test("a release file is published only as the signed list has it", () => {
  const program = Buffer.from("the program");
  const list = parseSums(`${sha(program)}  inyon-linux-x64\n${sha("get")} *get.sh\n`);
  const none = () => undefined;
  expect(() => checkReleaseFile("inyon-linux-x64", program, list, none)).not.toThrow();
  expect(() => checkReleaseFile("inyon-linux-x64", Buffer.from("tampered"), list, none)).toThrow("checksum mismatch");
  expect(() => checkReleaseFile("inyon-linux-arm64", program, list, none)).toThrow("not in SHA256SUMS");
  // the list can't name itself or its signature
  expect(() => checkReleaseFile("SHA256SUMS", Buffer.from("x"), list, none)).not.toThrow();
  expect(() => checkReleaseFile("SHA256SUMS.minisig", Buffer.from("x"), list, none)).not.toThrow();
  // from 0.20 the installers are in the list like everything else
  expect(() => checkReleaseFile("get.sh", Buffer.from("get"), list, none)).not.toThrow();
  expect(() => checkReleaseFile("get.sh", Buffer.from("#!/bin/sh\nevil\n"), list, () => Buffer.from("#!/bin/sh\nevil\n"))).toThrow(
    "checksum mismatch",
  );
});

test("an installer the list leaves out (before 0.20) has to be the tagged commit's own copy", () => {
  const list = parseSums("");
  const script = Buffer.from("#!/bin/sh\necho hi\n");
  expect(() => checkReleaseFile("get.ps1", script, list, () => Buffer.from(script))).not.toThrow();
  expect(() => checkReleaseFile("get.ps1", Buffer.from("#!/bin/sh\nevil\n"), list, () => script)).toThrow("not the copy in the release's tagged commit");
  expect(() => checkReleaseFile("get.sh", script, list, () => undefined)).toThrow("tag isn't here");
});

test("the installers served at the root carry the sha256 of the list the site checked", () => {
  const list = sha("the list");
  const sh = stampInstaller("get.sh", node("get.sh"), list);
  expect(sh).toContain(`\nLIST_SHA256=${list}\n`);
  expect(sh.replace(list, "")).toBe(node("get.sh"));
  const ps1 = stampInstaller("get.ps1", node("get.ps1"), list);
  expect(ps1).toContain(`\n$listSha256 = '${list}'\n`);
  expect(ps1.replace(list, "")).toBe(node("get.ps1"));
  // an installer from before 0.20 has no place for it and is served as it is
  expect(stampInstaller("get.sh", "#!/bin/sh\necho old\n", list)).toBe("#!/bin/sh\necho old\n");
  // one that names the slot but hasn't one empty line for it is refused rather than half filled
  expect(() => stampInstaller("get.sh", sh, list)).toThrow("expected one empty LIST_SHA256 line, found 0");
  expect(() => stampInstaller("get.sh", "LIST_SHA256=\nLIST_SHA256=\n", list)).toThrow("found 2");
  expect(() => stampInstaller("get.ps1", node("get.ps1"), "not a sha")).toThrow("isn't a sha256");
});

// release notes from before 23 Oct 2026 link to inyon.abhashchakraborty.tech; the changelog shows inyon.tech
test("old release notes link to the site's new address", () => {
  expect(oldSiteLinks("see [Install](https://inyon.abhashchakraborty.tech/docs/install/#x) or https://inyon.abhashchakraborty.tech")).toBe(
    "see [Install](https://inyon.tech/docs/install/#x) or https://inyon.tech",
  );
  expect(oldSiteLinks("`curl -fsSL https://inyon.abhashchakraborty.tech/get.sh | sh`")).toBe("`curl -fsSL https://inyon.tech/get.sh | sh`");
  expect(oldSiteLinks("https://inyon.abhashchakraborty.tech?ref=release and https://inyon.abhashchakraborty.tech#install")).toBe(
    "https://inyon.tech?ref=release and https://inyon.tech#install",
  );
  // a host that only starts the same stays as it is
  expect(oldSiteLinks("https://inyon.abhashchakraborty.tech.example.com/")).toBe("https://inyon.abhashchakraborty.tech.example.com/");
});

// pages asked fonts.googleapis.com for their fonts on every visit, there was no privacy policy or
// terms, and security.txt was missing: check the built site for each
test("the built site asks no other server for anything, links its legal pages and says how to report a flaw", () => {
  const out = join(tmpdir(), `inyon-site-${process.pid}`);
  rmSync(out, { recursive: true, force: true });
  const run = Bun.spawnSync(["bun", join(import.meta.dir, "build.ts"), "--offline", "--out", out], { stderr: "pipe", stdout: "pipe" });
  expect(run.exitCode).toBe(0);
  try {
    const html = (p: string) => readFileSync(join(out, p), "utf8");
    const pages = ["index.html", "download/index.html", "docs/index.html", "docs/security/index.html", "privacy/index.html", "terms/index.html", "license/index.html", "404.html"];
    for (const p of pages) {
      const page = html(p);
      expect(page).not.toContain("fonts.googleapis.com");
      expect(page).toContain(`<meta http-equiv="Content-Security-Policy" content="${CSP}">`);
      // every script is a file from this site, or JSON-LD data that never runs
      for (const [tag] of page.matchAll(/<script\b[^>]*>/g)) expect(tag).toMatch(/src="\/assets\/|type="application\/ld\+json"/);
      for (const href of ["/privacy/", "/terms/", "/license/", "/docs/security/"]) expect(page).toContain(`href="${href}"`);
      expect(page).toMatch(/© 20\d\d(–20\d\d)? <a [^>]*>Abhash Chakraborty<\/a>/);
    }
    expect(html("privacy/index.html")).toContain("<h1>Privacy policy</h1>");
    expect(html("terms/index.html")).toContain("<h1>Terms of use</h1>");
    expect(html("license/index.html")).toContain("Permission is hereby granted, free of charge");
    for (const f of ["geist.woff2", "geist-mono.woff2"]) expect(existsSync(join(out, "assets/fonts", f))).toBe(true);
    const sitemap = html("sitemap.xml");
    for (const p of ["/privacy/", "/terms/", "/license/"]) expect(sitemap).toContain(`<loc>https://inyon.tech${p}</loc>`);
    const expires = Date.parse(html(".well-known/security.txt").match(/^Expires: (.*)$/m)![1]!);
    expect(expires - Date.now()).toBeGreaterThan(364 * 24 * 3600 * 1000);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}, 60_000);

test("security.txt names a contact and the policy, and expires a year after the build", () => {
  const txt = securityTxt(new Date("2026-10-23T12:00:00Z"));
  expect(txt).toContain("Contact: https://github.com/Abhash-Chakraborty/inyon/security/advisories/new\n");
  expect(txt).toContain("Expires: 2027-10-23T12:00:00Z\n");
  expect(txt).toContain("Policy: https://inyon.tech/docs/security/#reporting-a-vulnerability\n");
  expect(txt).toContain("Canonical: https://inyon.tech/.well-known/security.txt\n");
});

test("each system's row offers inyon alone, and the engine files for older versions sit with the rest", () => {
  const programs = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "windows-x64.exe", "windows-arm64.exe"].map((s) => `inyon-${s}`);
  const engines = ["linux-x64", "linux-x64-musl", "linux-arm64", "linux-arm64-musl", "darwin-x64", "darwin-arm64", "windows-x64.exe"].map((s) => `inyon-engine-${s}`);
  const assets = [...programs, ...engines]
    .flatMap((n) => [n, `${n}.gz`])
    .concat(["SHA256SUMS", "inyon-0.23.0.apk", "inyon-web.tar.gz"])
    .map((name) => ({ name, size: 15e6 }));
  const html = downloadTable({ version: "0.23.0", tag: "v0.23.0", date: "2026-11-01", assets });
  const [systems, rest] = html.split('id="everything-else"');
  for (const p of programs) expect(systems).toContain(`<code>${p}.gz</code>`);
  expect(systems).not.toContain("inyon-engine-");
  // Windows on ARM gets its own build, not the x64 engine
  expect(systems).toMatch(/Windows<span>arm64<\/span><\/th><td><a [^>]+><code>inyon-windows-arm64\.exe\.gz<\/code><\/a><span class="size">[^<]+<\/span><\/td>/);
  // each engine file once, as its .gz
  for (const e of engines) {
    expect(rest).toContain(`<code>${e}.gz</code>`);
    expect(rest).not.toContain(`<code>${e}</code>`);
  }
  for (const n of ["SHA256SUMS", "inyon-0.23.0.apk", "inyon-web.tar.gz"]) expect(rest).toContain(`<code>${n}</code>`);
  expect(rest).not.toContain("<code>inyon-linux-x64</code>");
});
