// Builds inyon.tech: the home page, the download page, the docs (site/docs/*.md)
// and the changelog (from GitHub releases). Static files only; GitHub Pages serves them.
//
//   bun site/build.ts                 build into site/out, release details read from GitHub
//   bun site/build.ts --mirror        also download the latest release's files into /download,
//                                     each checked against its SHA256SUMS (the Pages workflow)
//   bun site/build.ts --offline       skip GitHub entirely (no download table, no changelog)
import { createHash, createPublicKey, verify } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { type Installer, isInstaller, stampInstaller } from "../installers/installers.ts";

const ROOT = dirname(import.meta.dir);
const SITE = import.meta.dir;
/** A short hash of a file in site/assets, added to its URL so a new page never meets an old cached copy */
const assetVersion = (name: string) =>
  createHash("sha256").update(readFileSync(join(SITE, "assets", name))).digest("hex").slice(0, 10);
const CSS_URL = `/assets/site.css?v=${assetVersion("site.css")}`;
/**
 * Every page's Content-Security-Policy. Pages load only this site's own files; the exceptions are
 * the Product Hunt badge on the home page and the video, which loads from YouTube when it's played.
 * GitHub Pages can't send headers, so it goes in a meta tag (which can't set frame-ancestors).
 */
export const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: https://api.producthunt.com",
  "font-src 'self'",
  "connect-src 'self'",
  "frame-src https://www.youtube-nocookie.com",
  "object-src 'none'",
  "base-uri 'self'",
  "form-action 'self'",
].join("; ");
/** the year the code was first published, through this one */
const COPYRIGHT_YEAR = new Date().getUTCFullYear() > 2026 ? `2026–${new Date().getUTCFullYear()}` : "2026";
const JS_URL = `/assets/site.js?v=${assetVersion("site.js")}`;
const args = process.argv.slice(2);
const OUT = args.includes("--out") ? args[args.indexOf("--out") + 1] : join(SITE, "out");
const MIRROR = args.includes("--mirror");
const OFFLINE = args.includes("--offline");
const REPO = "Abhash-Chakraborty/inyon";
const ORIGIN = "https://inyon.tech";
const GITHUB = `https://github.com/${REPO}`;
/** The 90-second video on YouTube. Once set, it takes the screenshot's place under the home hero. */
const YOUTUBE_ID = "NKEb2Pn6ZvQ";

// The sidebar, in reading order. Every page in site/docs must be listed here (checked below).
const NAV: { title: string; pages: string[] }[] = [
  { title: "Start here", pages: ["index", "quick-start", "setups", "install"] },
  { title: "Using Inyon", pages: ["overview", "sessions", "companion-agent", "controls", "terminal", "notifications", "settings", "skills", "pages", "temp-files"] },
  { title: "Phone", pages: ["android", "browser-copy"] },
  { title: "Machines and hubs", pages: ["computers", "windows-app", "hub", "machines", "companions"] },
  { title: "Reference", pages: ["cli", "configuration", "agents", "security", "troubleshooting", "changelog"] },
];

interface Page {
  slug: string;
  title: string;
  description: string;
  html: string;
  toc: { id: string; text: string; level: number }[];
  source?: string;
  /** when the page last changed, from git */
  modified?: string;
}

interface Asset {
  name: string;
  size: number;
  sha256?: string;
}

interface Release {
  version: string;
  tag: string;
  date: string;
  assets: Asset[];
  /** true when the uncompressed programs were left out to stay inside the Pages size limit */
  gzOnly?: boolean;
}

const HOME = {
  title: "Inyon: your coding agents, from your phone",
  description:
    "Watch, answer and steer Claude Code, Codex, Gemini CLI, OpenCode, GitHub Copilot and Antigravity CLI running in tmux on your own computers, from your phone or any browser.",
};
const DOWNLOAD_DESCRIPTION =
  "Download Inyon for Android, Linux, macOS and Windows. Every file is checked against the release's SHA256SUMS.";
const CHANGELOG_DESCRIPTION = "Every release of Inyon, newest first.";
const AUTHOR = { "@type": "Person", name: "Abhash Chakraborty", url: "https://abhashchakraborty.tech" };

/** The pages outside the docs that say what Inyon sends where and on what terms: site/legal/*.md, and the licence. */
const LEGAL = ["privacy", "terms"] as const;
const LICENSE_DESCRIPTION = "Inyon's code is free to use, change and share under the MIT licence.";

function legalMeta(slug: (typeof LEGAL)[number]) {
  return frontmatter(readFileSync(join(SITE, "legal", `${slug}.md`), "utf8"));
}

// ---------- link previews ----------

export const OG_SIZE = { width: 1200, height: 630 };

/** One page's link preview, as site/og.ts draws it into site/og/<id>.png. */
export interface Card {
  id: string;
  path: string;
  title: string;
  description: string;
  eyebrow?: string;
  chip?: string;
}

/** Every page that gets its own link preview: home, download, then the docs in sidebar order. */
export function cards(): Card[] {
  const list: Card[] = [
    { id: "home", path: "/", title: HOME.title, description: HOME.description },
    {
      id: "download",
      path: "/download/",
      title: "Download Inyon",
      description: DOWNLOAD_DESCRIPTION,
      eyebrow: "Android · Linux · macOS · Windows",
      chip: "Download",
    },
  ];
  for (const slug of LEGAL) {
    const { meta } = legalMeta(slug);
    list.push({ id: slug, path: `/${slug}/`, title: meta.title, description: meta.description ?? "", eyebrow: "Inyon", chip: "Legal" });
  }
  list.push({ id: "license", path: "/license/", title: "MIT licence", description: LICENSE_DESCRIPTION, eyebrow: "Inyon", chip: "Legal" });
  for (const group of NAV)
    for (const slug of group.pages) {
      const meta =
        slug === "changelog"
          ? { title: "Changelog", description: CHANGELOG_DESCRIPTION }
          : frontmatter(readFileSync(join(SITE, "docs", `${slug}.md`), "utf8")).meta;
      list.push({
        id: slug === "index" ? "docs" : slug,
        path: url(slug),
        title: meta.title,
        description: meta.description ?? "",
        eyebrow: group.title,
        chip: "Docs",
      });
    }
  return list;
}

/** What a preview was drawn from: the card, the drawing code and the pictures in it. A page whose
 *  key no longer matches site/og/keys.json has a stale picture. */
export function ogKey(card: Card): string {
  const h = createHash("sha256")
    .update(readFileSync(join(SITE, "og.ts")))
    .update(readFileSync(join(ROOT, "apps/web/public/icon.svg")))
    .update(JSON.stringify([card.id, card.path, card.title, card.description, card.eyebrow, card.chip]));
  if (card.id === "home") h.update(readFileSync(join(ROOT, "docs/phones-light.webp")));
  return h.digest("hex").slice(0, 12);
}

const ogKeys: Record<string, string> = existsSync(join(SITE, "og/keys.json"))
  ? JSON.parse(readFileSync(join(SITE, "og/keys.json"), "utf8"))
  : {};
const stale = new Set<string>();

/** The preview image for a page, or the home page's when this page's is missing or out of date. */
function ogImage(card: Card): string {
  const key = ogKey(card);
  if (ogKeys[card.id] === key && existsSync(join(SITE, "og", `${card.id}.png`)))
    return `${ORIGIN}/og/${card.id}.png?v=${key}`;
  stale.add(card.id);
  return `${ORIGIN}/og/home.png`;
}

// ---------- helpers ----------

const esc = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const strip = (html: string) =>
  html
    .replace(/<[^>]+>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&");
const slugify = (s: string) =>
  s
    .toLowerCase()
    .replace(/[`'’"]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
const url = (slug: string) => (slug === "index" ? "/docs/" : `/docs/${slug}/`);
/** A file's size in decimal units, as browsers show a download (2,883,584 bytes is 2.9 MB, not 2.7). */
export const mb = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1e3))} KB`);

/** A WebP or PNG's size in pixels, read from its header, so pictures reserve their space. */
function imageSize(file: string): { width: number; height: number } | undefined {
  const b = readFileSync(file);
  if (b.toString("ascii", 0, 4) === "RIFF" && b.toString("ascii", 8, 12) === "WEBP") {
    const kind = b.toString("ascii", 12, 16);
    if (kind === "VP8X") return { width: 1 + b.readUIntLE(24, 3), height: 1 + b.readUIntLE(27, 3) };
    if (kind === "VP8 ") return { width: b.readUInt16LE(26) & 0x3fff, height: b.readUInt16LE(28) & 0x3fff };
    if (kind === "VP8L") {
      const n = b.readUInt32LE(21);
      return { width: 1 + (n & 0x3fff), height: 1 + ((n >>> 14) & 0x3fff) };
    }
  }
  if (b.readUInt32BE(0) === 0x89504e47) return { width: b.readUInt32BE(16), height: b.readUInt32BE(20) };
  return undefined;
}

/** When a file (or the newest of several) last changed, from git. Undefined in a shallow clone,
 *  where every file would look as new as the last commit. */
let shallow: boolean | undefined;
function lastChanged(...paths: string[]): string | undefined {
  shallow ??= Bun.spawnSync(["git", "rev-parse", "--is-shallow-repository"], { cwd: ROOT }).stdout.toString().trim() !== "false";
  if (shallow) return undefined;
  const out = Bun.spawnSync(["git", "log", "-1", "--format=%cI", "--", ...paths], { cwd: ROOT }).stdout.toString().trim();
  return out || undefined;
}
const newest = (...dates: (string | undefined)[]) =>
  dates.filter((d): d is string => !!d).sort((a, b) => Date.parse(b) - Date.parse(a))[0];

/** Structured data for search engines, safe inside a <script> element. */
const jsonLd = (data: object) =>
  `<script type="application/ld+json">${JSON.stringify({ "@context": "https://schema.org", ...data }).replace(/</g, "\\u003c")}</script>`;

function frontmatter(src: string): { meta: Record<string, string>; body: string } {
  const m = src.match(/^---\n([\s\S]*?)\n---\n/);
  if (!m) return { meta: {}, body: src };
  const meta: Record<string, string> = {};
  for (const line of m[1].split("\n")) {
    const i = line.indexOf(":");
    if (i > 0) meta[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return { meta, body: src.slice(m[0].length) };
}

/** Markdown to the site's HTML: heading anchors, a table of contents, copyable code, callouts,
 *  links between pages and images from the repo's docs/ folder. */
function render(md: string): { html: string; toc: Page["toc"] } {
  let html = Bun.markdown.html(md);
  const toc: Page["toc"] = [];
  const seen = new Map<string, number>();

  html = html.replace(/<h([23])>([\s\S]*?)<\/h\1>/g, (_, level: string, inner: string) => {
    const text = strip(inner);
    let id = slugify(text);
    const n = seen.get(id) ?? 0;
    seen.set(id, n + 1);
    if (n) id = `${id}-${n}`;
    toc.push({ id, text, level: Number(level) });
    return `<h${level} id="${id}"><a class="anchor" href="#${id}" aria-hidden="true" tabindex="-1">#</a>${inner}</h${level}>`;
  });

  html = html.replace(/<table>([\s\S]*?)<\/table>/g, '<div class="table"><table>$1</table></div>');

  html = html.replace(
    /<pre><code(?: class="language-([\w-]+)")?>([\s\S]*?)<\/code><\/pre>/g,
    (_, lang: string | undefined, code: string) => {
      const label = lang ? `<span class="lang">${lang === "sh" ? "shell" : lang}</span>` : "";
      // shell comments read as notes, not commands
      const body =
        lang === "sh" || lang === "powershell"
          ? code.replace(/(^|\s)(#[^\n]*)/g, '$1<span class="c">$2</span>')
          : code;
      return `<div class="code">${label}<button class="copy" type="button" aria-label="Copy">Copy</button><pre><code>${body}</code></pre></div>`;
    },
  );

  // > **Note:** … / **Warning:** … / **Tip:** … become callouts
  html = html.replace(
    /<blockquote>\s*<p><strong>(Note|Warning|Tip)[:.]?<\/strong>:?\s*([\s\S]*?)<\/blockquote>/g,
    (_, kind: string, rest: string) =>
      `<aside class="callout ${kind.toLowerCase()}"><span class="callout-label">${kind}</span><p>${rest}</aside>`,
  );

  // links between docs pages: quick-start.md#x → /docs/quick-start/#x
  html = html.replace(/href="([\w-]+)\.md(#[\w-]*)?"/g, (_, slug: string, hash = "") => `href="${url(slug)}${hash}"`);
  html = html.replace(/href="\.\.\/\.\.\/(SECURITY|CONTRIBUTING|LICENSE|NOTICE)(\.md)?"/g, `href="${GITHUB}/blob/main/$1$2"`);
  // images from the repo's docs/ folder are copied to /img; a -light image with a -dark twin
  // follows the reader's colour scheme
  html = html.replace(/<img src="\.\.\/\.\.\/docs\/([\w.-]+)"([^>]*)>/g, (_, file: string, rest: string) => {
    const size = /\swidth=/.test(rest) ? undefined : imageSize(join(ROOT, "docs", file));
    const dims = size ? ` width="${size.width}" height="${size.height}"` : "";
    const img = `<img src="/img/${file}"${rest.replace(/\s*\/$/, "")}${dims} loading="lazy" decoding="async">`;
    const dark = file.replace("-light.", "-dark.");
    return dark !== file && existsSync(join(ROOT, "docs", dark))
      ? `<picture><source media="(prefers-color-scheme: dark)" srcset="/img/${dark}">${img}</picture>`
      : img;
  });
  // external links say so
  html = html.replace(/<a href="(https?:\/\/[^"]+)">/g, (m, href: string) =>
    href.startsWith(ORIGIN) ? m : `<a href="${href}" class="ext" rel="noopener">`,
  );
  return { html, toc };
}

// ---------- GitHub ----------

async function gh(path: string): Promise<unknown> {
  const headers: Record<string, string> = { accept: "application/vnd.github+json", "user-agent": "inyon-site" };
  if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const r = await fetch(`https://api.github.com/repos/${REPO}${path}`, { headers });
  if (!r.ok) throw new Error(`GitHub ${path}: ${r.status}`);
  return r.json();
}

interface GhRelease {
  tag_name: string;
  name: string;
  body: string;
  published_at: string;
  draft: boolean;
  prerelease: boolean;
  html_url: string;
  assets: { name: string; size: number; browser_download_url: string }[];
}

/** What /download may take up, leaving room for the pages and pictures under Pages' 1 GB. */
const MIRROR_BUDGET = 900 * 1024 * 1024;

// Files the site serves from /download. The old tether-node copies stay on GitHub only.
const mirrored = (name: string) => !name.startsWith("tether-node");

/**
 * Whether a release is whole: the Android build attaches the APK and lists it in SHA256SUMS
 * (re-signing the list) a few minutes after the programs are out, and the inyon command refuses a
 * list without that line. Until then the site keeps serving the release before it.
 */
async function complete(r: GhRelease): Promise<boolean> {
  const apk = `inyon-${r.tag_name.replace(/^v/, "")}.apk`;
  const sums = r.assets.find((a) => a.name === "SHA256SUMS");
  if (!sums || !r.assets.some((a) => a.name === apk)) return false;
  const res = await fetch(sums.browser_download_url, { redirect: "follow" });
  if (!res.ok) throw new Error(`SHA256SUMS of ${r.tag_name}: ${res.status}`);
  return (await res.text()).split("\n").some((l) => l.trim().replace(/\s+\*?/, " ").split(" ")[1] === apk);
}

async function latestRelease(): Promise<{ release: Release; raw: GhRelease } | null> {
  if (OFFLINE) return null;
  const list = ((await gh("/releases?per_page=10")) as GhRelease[]).filter(
    (r) => !r.draft && !r.prerelease && /^v\d+\.\d+\.\d+$/.test(r.tag_name),
  );
  let raw: GhRelease | undefined;
  for (const r of list) {
    if (await complete(r)) {
      raw = r;
      break;
    }
    console.log(`  ${r.tag_name} isn't complete yet (no APK in its SHA256SUMS), so it waits`);
  }
  if (!raw) throw new Error("none of the last 10 releases is complete");
  let assets = raw.assets.filter((a) => mirrored(a.name)).map((a) => ({ name: a.name, size: a.size }));
  // GitHub Pages publishes at most 1 GB. Every installer and updater takes a program's .gz copy
  // when there is one, so the plain copies are the ones to leave on GitHub if a release outgrows it
  const total = assets.reduce((n, a) => n + a.size, 0);
  const names = new Set(assets.map((a) => a.name));
  const gzOnly = total > MIRROR_BUDGET;
  if (gzOnly) {
    assets = assets.filter((a) => !names.has(`${a.name}.gz`));
    console.log(`  ${mb(total)} is over the ${mb(MIRROR_BUDGET)} budget: serving the .gz copies only`);
  }
  return {
    raw,
    release: {
      version: raw.tag_name.replace(/^v/, ""),
      tag: raw.tag_name,
      date: raw.published_at.slice(0, 10),
      assets,
      gzOnly,
    },
  };
}

/** Inyon's release key (minisign key 0F27F489FF0BBD53), the same one apps/cli/src/signing.rs carries. */
export const RELEASE_KEY = "RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6";

/**
 * Whether `sig` (a .minisig file) signs `data` with `key` (a minisign public key's base64 line).
 * Only the hashed kind ("ED": Ed25519 over the file's BLAKE2b-512) is accepted, as in the CLI, and
 * the trusted comment's own signature has to check out too.
 */
export function minisignOk(data: Buffer, sig: string, key: string): boolean {
  const pk = Buffer.from(key.trim(), "base64");
  const lines = sig.split("\n").map((l) => l.trim());
  if (pk.length !== 42 || pk.subarray(0, 2).toString() !== "Ed" || lines.length < 4) return false;
  const s = Buffer.from(lines[1], "base64");
  const comment = lines[2].match(/^trusted comment: (.*)$/)?.[1];
  const global = Buffer.from(lines[3], "base64");
  if (s.length !== 74 || s.subarray(0, 2).toString() !== "ED" || comment === undefined || global.length !== 64) return false;
  if (!s.subarray(2, 10).equals(pk.subarray(2, 10))) return false;
  const pub = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: pk.subarray(10).toString("base64url") }, format: "jwk" });
  const digest = createHash("blake2b512").update(data).digest();
  const sigBytes = s.subarray(10);
  return (
    verify(null, digest, pub, sigBytes) && verify(null, Buffer.concat([sigBytes, Buffer.from(comment)]), pub, global)
  );
}

/** sha256sum's lines ("<hex>  name", "*" marking binary mode) as file name to checksum. */
export function parseSums(sums: string): Map<string, string> {
  const expected = new Map<string, string>();
  for (const line of sums.split("\n")) {
    const m = line.trim().match(/^([0-9a-f]{64})\s+\*?(.+)$/);
    if (m) expected.set(m[2], m[1]);
  }
  return expected;
}

/**
 * An installer as the commit tagged `tag` has it, from this checkout's git. Only releases before
 * 0.20 need this, and they kept the installers in apps/node (since 0.23 they're in installers/).
 */
function taggedInstaller(tag: string, name: Installer): Buffer | undefined {
  const p = Bun.spawnSync(["git", "show", `${tag}:apps/node/${name}`], { cwd: ROOT, stdout: "pipe", stderr: "ignore" });
  return p.exitCode === 0 ? Buffer.from(p.stdout) : undefined;
}

/**
 * Throws unless release file `name` may be published as `data`: listed in the signed SHA256SUMS
 * with that checksum. The list can't name itself or its signature. Releases before 0.20 left the
 * installers out of it, so for those an installer has to be byte for byte the one in the commit
 * the release is tagged on (`tagged`), or the site keeps serving what it had.
 */
export function checkReleaseFile(
  name: string,
  data: Buffer,
  expected: Map<string, string>,
  tagged: (name: Installer) => Buffer | undefined,
): void {
  const want = expected.get(name);
  if (want) {
    if (createHash("sha256").update(data).digest("hex") !== want) throw new Error(`${name}: checksum mismatch`);
    return;
  }
  if (name === "SHA256SUMS" || name === "SHA256SUMS.minisig") return;
  if (!isInstaller(name)) throw new Error(`${name}: not in SHA256SUMS`);
  const copy = tagged(name);
  if (!copy) throw new Error(`${name}: not in SHA256SUMS, and the release's tag isn't here to check it against`);
  if (!copy.equals(data)) throw new Error(`${name}: not in SHA256SUMS, and not the copy in the release's tagged commit`);
}

/** Downloads every mirrored file of the release and refuses to publish one that doesn't match the
 *  release's SHA256SUMS, so the domain can never serve something the release didn't sign off. */
async function mirror(release: Release, raw: GhRelease) {
  const dir = join(OUT, "download");
  mkdirSync(dir, { recursive: true });
  const fetchAsset = async (name: string) => {
    const a = raw.assets.find((x) => x.name === name);
    if (!a) throw new Error(`release has no ${name}`);
    const r = await fetch(a.browser_download_url, { redirect: "follow" });
    if (!r.ok) throw new Error(`download ${name}: ${r.status}`);
    return Buffer.from(await r.arrayBuffer());
  };
  const sumsBytes = await fetchAsset("SHA256SUMS");
  const sums = sumsBytes.toString("utf8");
  // from 0.14 the list is signed, and inyon update refuses a copy without its signature, so the
  // site publishes nothing unless the list carries a good one
  const signed = raw.assets.some((a) => a.name === "SHA256SUMS.minisig");
  if (signed) {
    if (!minisignOk(sumsBytes, (await fetchAsset("SHA256SUMS.minisig")).toString("utf8"), RELEASE_KEY)) {
      throw new Error("SHA256SUMS isn't signed by the release key");
    }
    console.log("  SHA256SUMS signature checked");
  } else if (!/^0\.(\d|1[0-3])\./.test(release.version)) {
    throw new Error(`${release.tag} has no SHA256SUMS.minisig`);
  }
  // the short installer URLs at the root must keep working, so a release without them isn't served
  for (const name of ["get.sh", "get.ps1"]) {
    if (!release.assets.some((a) => a.name === name)) throw new Error(`${release.tag} has no ${name}`);
  }
  const expected = parseSums(sums);
  const sumsSha = createHash("sha256").update(sumsBytes).digest("hex");
  for (const asset of release.assets) {
    const data = asset.name === "SHA256SUMS" ? sumsBytes : await fetchAsset(asset.name);
    checkReleaseFile(asset.name, data, expected, (name) => taggedInstaller(release.tag, name));
    asset.sha256 = createHash("sha256").update(data).digest("hex");
    writeFileSync(join(dir, asset.name), data);
    // the installers are also served short, at the site's root, with the sha256 of the list just
    // checked filled in: that copy needs no minisign to trust it
    if (isInstaller(asset.name)) writeFileSync(join(OUT, asset.name), stampInstaller(asset.name, data.toString("utf8"), sumsSha));
    // a stable name for the newest APK
    if (asset.name.endsWith(".apk")) writeFileSync(join(dir, "inyon.apk"), data);
    console.log(`  ${asset.name} ${mb(asset.size)} ${expected.has(asset.name) ? "checked" : ""}`);
  }
}

async function releases(): Promise<GhRelease[]> {
  if (OFFLINE) return [];
  return ((await gh("/releases?per_page=100")) as GhRelease[]).filter((r) => !r.draft && !r.prerelease);
}

/** A release's notes as HTML, headings demoted to sit under its version. */
/** Links in notes written before the move to inyon.tech point here directly instead of through the old address's redirect. */
export const oldSiteLinks = (md: string) => md.replace(/https?:\/\/inyon\.abhashchakraborty\.tech(?=[/?#)\s"'>]|$)/g, ORIGIN);

function notes(r: GhRelease): string {
  // GitHub's "Full Changelog" line is a bare URL; each release links to its page instead
  const md = oldSiteLinks(r.body || "").replace(/^#+\s.*\n/, "").replace(/^\*\*Full Changelog\*\*:.*$/m, "");
  // without their ids the headings' # links would point nowhere, so they go too
  return render(md)
    .html.replace(/<h[23] id="[^"]*"><a class="anchor"[^>]*>#<\/a>/g, "<h4>")
    .replace(/<h[23] id="[^"]*">/g, "<h4>")
    .replace(/<\/h[23]>/g, "</h4>");
}

const releaseId = (r: GhRelease) => `v${slugify(r.tag_name.replace(/^v/, ""))}`;

function changelog(list: GhRelease[]): string {
  if (!list.length) return `<p>The release notes are on <a class="ext" href="${GITHUB}/releases">GitHub</a>.</p>`;
  return list
    .map((r) => {
      const v = r.tag_name.replace(/^v/, "");
      const id = releaseId(r);
      return `<section class="release" id="${id}"><h2><a class="anchor" href="#${id}" aria-hidden="true" tabindex="-1">#</a>${esc(v)}<time datetime="${r.published_at}">${r.published_at.slice(0, 10)}</time></h2>${notes(r)}<p class="edit"><a class="ext" href="${r.html_url}" rel="noopener">${esc(v)} on GitHub</a></p></section>`;
    })
    .join("\n");
}

/** The releases as an Atom feed, so feed readers (and GitHub-watching tools) can follow them. */
function feed(list: GhRelease[]): string {
  const entries = list.slice(0, 30).map((r) => {
    const v = r.tag_name.replace(/^v/, "");
    const link = `${ORIGIN}/docs/changelog/#${releaseId(r)}`;
    return `  <entry>
    <title>Inyon ${esc(v)}</title>
    <id>${esc(r.html_url)}</id>
    <link rel="alternate" type="text/html" href="${link}"/>
    <published>${r.published_at}</published>
    <updated>${r.published_at}</updated>
    <content type="html">${esc(notes(r))}</content>
  </entry>`;
  });
  return `<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Inyon releases</title>
  <subtitle>${esc(CHANGELOG_DESCRIPTION)}</subtitle>
  <id>${ORIGIN}/releases.xml</id>
  <link rel="self" type="application/atom+xml" href="${ORIGIN}/releases.xml"/>
  <link rel="alternate" type="text/html" href="${ORIGIN}/docs/changelog/"/>
  <updated>${list[0]?.published_at ?? new Date().toISOString()}</updated>
  <author><name>${AUTHOR.name}</name><uri>${AUTHOR.url}</uri></author>
  <icon>${ORIGIN}/icon.svg</icon>
${entries.join("\n")}
</feed>
`;
}

// ---------- layout ----------

const ICON = readFileSync(join(ROOT, "apps/web/public/icon.svg"), "utf8");

function layout(opts: {
  /** the whole <title> */
  title: string;
  description: string;
  path: string;
  body: string;
  version?: string;
  bodyClass?: string;
  /** the link preview; none for pages search engines shouldn't list */
  card?: Card;
  type?: "website" | "article";
  modified?: string;
  data?: object[];
}): string {
  const full = opts.title;
  const canonical = `${ORIGIN}${opts.path}`;
  const image = opts.card && ogImage(opts.card);
  const alt = opts.card?.id === "home" ? `${HOME.title}, beside two phones showing Inyon` : opts.card?.chip === "Legal" ? `${opts.card.title}: Inyon` : `${opts.card?.title}: Inyon docs`;
  const head = opts.card
    ? `<link rel="canonical" href="${canonical}">
<meta property="og:type" content="${opts.type ?? "website"}">
<meta property="og:site_name" content="Inyon">
<meta property="og:locale" content="en_GB">
<meta property="og:title" content="${esc(full)}">
<meta property="og:description" content="${esc(opts.description)}">
<meta property="og:url" content="${canonical}">
<meta property="og:image" content="${image}">
<meta property="og:image:type" content="image/png">
<meta property="og:image:width" content="${OG_SIZE.width}">
<meta property="og:image:height" content="${OG_SIZE.height}">
<meta property="og:image:alt" content="${esc(alt)}">${opts.modified ? `\n<meta property="article:modified_time" content="${opts.modified}">` : ""}
<meta name="twitter:card" content="summary_large_image">
<meta name="twitter:title" content="${esc(full)}">
<meta name="twitter:description" content="${esc(opts.description)}">
<meta name="twitter:image" content="${image}">
<meta name="twitter:image:alt" content="${esc(alt)}">
${(opts.data ?? []).map(jsonLd).join("\n")}`
    : `<meta name="robots" content="noindex">`;
  const nav = [
    ["/docs/", "Docs"],
    ["/download/", "Download"],
    ["/docs/changelog/", "Changelog"],
  ]
    .map(([href, label]) => `<a href="${href}"${opts.path.startsWith(href) && !(href === "/docs/" && opts.path.startsWith("/docs/changelog")) ? ' aria-current="page"' : ""}>${label}</a>`)
    .join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(full)}</title>
<meta name="description" content="${esc(opts.description)}">
<meta name="author" content="${AUTHOR.name}">
${head}
<meta name="theme-color" content="#e9e8e4" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#1b1a18" media="(prefers-color-scheme: dark)">
<link rel="icon" href="/icon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="alternate" type="application/atom+xml" title="Inyon releases" href="/releases.xml">
<meta http-equiv="Content-Security-Policy" content="${CSP}">
<meta name="referrer" content="strict-origin-when-cross-origin">
<link rel="preload" href="/assets/fonts/geist.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="${CSS_URL}">
<script src="${JS_URL}" defer></script>
</head>
<body class="${opts.bodyClass ?? ""}">
<a class="skip" href="#main">Skip to content</a>
<header class="top">
  <div class="top-in">
    <a class="brand" href="/"><span class="mark" aria-hidden="true">${ICON}</span>Inyon${opts.version ? `<span class="ver">${esc(opts.version)}</span>` : ""}</a>
    <nav class="top-nav" aria-label="Site">${nav}</nav>
    <button class="search-btn" type="button" data-search aria-label="Search the docs"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3"/></svg><span>Search</span><kbd>/</kbd></button>
    <a class="gh" href="${GITHUB}" aria-label="Inyon on GitHub" rel="noopener"><svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg></a>
    <button class="menu-btn" type="button" data-menu aria-label="Open the menu" aria-expanded="false"><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 6h14M3 10h14M3 14h14"/></svg></button>
  </div>
</header>
${opts.body}
<footer class="foot">
  <div class="foot-in">
    <div class="foot-about">
      <a class="brand" href="/"><span class="mark" aria-hidden="true">${ICON}</span>Inyon</a>
      <p>Watch, answer and steer the coding agents running on your computers, from your phone or any browser.</p>
      <p>This site is downloads and docs only. Inyon runs on your own machines, and nothing here connects to them.</p>
    </div>
    <nav class="foot-cols" aria-label="Footer">
      <div><p class="side-title">Start</p><ul>
        <li><a href="/docs/quick-start/">Quick start</a></li>
        <li><a href="/docs/setups/">Choose a setup</a></li>
        <li><a href="/docs/install/">Install</a></li>
        <li><a href="/docs/android/">Android app</a></li>
      </ul></div>
      <div><p class="side-title">Reference</p><ul>
        <li><a href="/docs/hub/">Running a hub</a></li>
        <li><a href="/docs/cli/">The inyon command</a></li>
        <li><a href="/docs/agents/">Agents</a></li>
        <li><a href="/docs/troubleshooting/">Troubleshooting</a></li>
      </ul></div>
      <div><p class="side-title">Project</p><ul>
        <li><a href="/download/">Download</a></li>
        <li><a href="/docs/changelog/">Changelog</a></li>
        <li><a href="${GITHUB}" class="ext" rel="noopener">GitHub</a></li>
        <li><a href="${GITHUB}/issues" class="ext" rel="noopener">Report an issue</a></li>
      </ul></div>
    </nav>
  </div>
  <div class="foot-base">
    <div class="foot-base-in">
      <span>© ${COPYRIGHT_YEAR} <a href="https://abhashchakraborty.tech" rel="noopener">Abhash Chakraborty</a>${opts.version ? ` · Inyon ${esc(opts.version)}` : ""}</span>
      <nav class="foot-legal" aria-label="Legal"><a href="/privacy/">Privacy</a><a href="/terms/">Terms</a><a href="/docs/security/">Security</a><a href="/license/">MIT licence</a></nav>
    </div>
  </div>
</footer>
<div class="search" data-search-dialog hidden>
  <div class="search-panel" role="dialog" aria-modal="true" aria-label="Search the docs">
    <div class="search-field"><svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="9" cy="9" r="5.5"/><path d="m13.2 13.2 3.3 3.3"/></svg><input type="search" placeholder="Search the docs" aria-label="Search the docs" autocomplete="off" spellcheck="false"><kbd>Esc</kbd></div>
    <ul class="search-results" role="listbox"></ul>
  </div>
</div>
</body>
</html>
`;
}

/** A page outside the docs: one column, no sidebar, with the date it last changed. */
function legalPage(opts: { slug: string; title: string; description: string; html: string; updated?: string; version?: string; source?: string }): string {
  const path = `/${opts.slug}/`;
  const card = cardFor(path);
  const body = `
<main id="main" class="page legal">
  <div class="doc">
    <p class="eyebrow">Inyon${opts.updated ? ` · Updated ${esc(longDate(opts.updated))}` : ""}</p>
    <h1>${esc(opts.title)}</h1>
    <p class="lede">${esc(opts.description)}</p>
    <div class="prose">${opts.html}</div>
    ${opts.source ? `<p class="edit"><a class="ext" href="${GITHUB}/blob/main/${opts.source}" rel="noopener">See this page's history on GitHub</a></p>` : ""}
  </div>
</main>`;
  return layout({
    title: `${opts.title} · Inyon`,
    description: opts.description,
    path,
    body,
    version: opts.version,
    card,
    modified: opts.updated,
    data: [{ "@type": "WebPage", name: opts.title, description: opts.description, url: `${ORIGIN}${path}`, inLanguage: "en", publisher: AUTHOR, isPartOf: { "@id": `${ORIGIN}/#website` }, ...(opts.updated ? { dateModified: opts.updated } : {}) }],
  });
}

const longDate = (iso: string) => new Date(`${iso.slice(0, 10)}T00:00:00Z`).toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });

/**
 * How to reach me about a vulnerability, in the format scanners and researchers look for (RFC 9116).
 * It has to say when it expires; the site is rebuilt with every release, well inside a year.
 */
export function securityTxt(now: Date): string {
  const expires = new Date(now.getTime() + 365 * 24 * 3600 * 1000);
  return [
    `Contact: ${GITHUB}/security/advisories/new`,
    `Expires: ${expires.toISOString().replace(/\.\d+Z$/, "Z")}`,
    `Policy: ${ORIGIN}/docs/security/#reporting-a-vulnerability`,
    `Canonical: ${ORIGIN}/.well-known/security.txt`,
    "Preferred-Languages: en",
    "",
  ].join("\n");
}

function sidebar(current: string): string {
  return NAV.map(
    (group) =>
      `<div class="side-group"><p class="side-title">${group.title}</p><ul>${group.pages
        .map((slug) => {
          const p = pages.get(slug)!;
          return `<li><a href="${url(slug)}"${slug === current ? ' aria-current="page"' : ""}>${esc(p.nav ?? p.title)}</a></li>`;
        })
        .join("")}</ul></div>`,
  ).join("");
}

function docPage(page: Page & { nav?: string }, version?: string): string {
  const order = NAV.flatMap((g) => g.pages);
  const i = order.indexOf(page.slug);
  const prev = i > 0 ? pages.get(order[i - 1]) : undefined;
  const next = i < order.length - 1 ? pages.get(order[i + 1]) : undefined;
  const group = NAV.find((g) => g.pages.includes(page.slug))?.title ?? "";
  const toc = page.toc.filter((t) => t.level === 2);
  const body = `
<div class="docs">
  <aside class="side" data-side aria-label="Docs">${sidebar(page.slug)}</aside>
  <main id="main" class="doc">
    <p class="eyebrow">${esc(group)}</p>
    <h1>${esc(page.title)}</h1>
    ${page.description ? `<p class="lede">${esc(page.description)}</p>` : ""}
    <div class="prose">${page.html}</div>
    <nav class="pager" aria-label="Pages">
      ${prev ? `<a class="prev" href="${url(prev.slug)}"><span>Previous</span>${esc(prev.title)}</a>` : "<span></span>"}
      ${next ? `<a class="next" href="${url(next.slug)}"><span>Next</span>${esc(next.title)}</a>` : "<span></span>"}
    </nav>
    ${page.source ? `<p class="edit"><a class="ext" href="${GITHUB}/blob/main/site/docs/${page.source}" rel="noopener">Edit this page on GitHub</a></p>` : ""}
  </main>
  ${toc.length > 1 ? `<nav class="toc" aria-label="On this page"><p class="side-title">On this page</p><ul>${toc.map((t) => `<li><a href="#${t.id}">${esc(t.text)}</a></li>`).join("")}</ul></nav>` : '<div class="toc"></div>'}
</div>`;
  const path = url(page.slug);
  const card = cardFor(path);
  const modified = page.modified;
  const crumbs = [{ name: "Docs", item: `${ORIGIN}/docs/` }];
  if (page.slug !== "index") crumbs.push({ name: page.title, item: `${ORIGIN}${path}` });
  return layout({
    title: `${page.title} · Inyon docs`,
    description: page.description,
    path,
    body,
    version,
    card,
    type: "article",
    modified,
    data: [
      {
        "@type": "TechArticle",
        headline: page.title,
        description: page.description,
        url: `${ORIGIN}${path}`,
        image: ogImage(card),
        inLanguage: "en",
        ...(modified ? { dateModified: modified } : {}),
        author: AUTHOR,
        publisher: AUTHOR,
        isPartOf: { "@id": `${ORIGIN}/#website` },
        about: { "@id": `${ORIGIN}/#app` },
      },
      {
        "@type": "BreadcrumbList",
        itemListElement: crumbs.map((c, i) => ({ "@type": "ListItem", position: i + 1, ...c })),
      },
    ],
  });
}

/**
 * The README's release badge, saved for the release this build serves. Its URL names the version,
 * so no cache can hold an older one; the site lets it be kept for 10 minutes, and pages.yml clears
 * GitHub's copy after each deploy. It's served from this domain, so only plain shapes are kept.
 */
async function releaseBadges(version: string) {
  mkdirSync(join(OUT, "badge"), { recursive: true });
  for (const mode of ["light", "dark"] as const) {
    const src = `https://shieldcn.dev/badge/release-v${encodeURIComponent(version.replaceAll("-", "--"))}.svg?variant=secondary&mode=${mode}&logo=lu:Tag`;
    let svg = "";
    for (let tries = 0; tries < 3 && !svg; tries++) {
      const r = await fetch(src).catch(() => null);
      const body = r?.ok ? await r.text() : "";
      const tags = [...body.matchAll(/<([a-zA-Z][\w:-]*)/g)].map((m) => m[1]);
      if (body.startsWith("<svg") && tags.every((t) => ["svg", "g", "path"].includes(t)) && !/\son\w+=|href|url\(/i.test(body)) svg = body;
    }
    if (!svg) {
      console.warn(`release badge: ${src} didn't give a plain SVG; drawing a simple one`);
      const [bg, fg] = mode === "dark" ? ["#27272a", "#fafafa"] : ["#f4f4f5", "#18181b"];
      const text = `release v${version}`;
      const width = 24 + Math.round(text.length * 7.4);
      svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="32" viewBox="0 0 ${width} 32"><rect width="${width}" height="32" rx="6" fill="${bg}"/><text x="12" y="21" fill="${fg}" font-family="-apple-system,'Segoe UI',Helvetica,Arial,sans-serif" font-size="13">${text}</text></svg>`;
    }
    writeFileSync(join(OUT, "badge", `release-${mode}.svg`), svg);
  }
}

let cardList: Card[] | undefined;
const cardFor = (path: string) => (cardList ??= cards()).find((c) => c.path === path)!;

// ---------- build ----------

const pages = new Map<string, Page & { nav?: string }>();

async function main() {
  rmSync(OUT, { recursive: true, force: true });
  mkdirSync(OUT, { recursive: true });

  const latest = await latestRelease().catch((e) => {
    if (MIRROR) throw e;
    console.warn(`release details unavailable: ${e.message}`);
    return null;
  });
  if (MIRROR && latest) {
    console.log(`mirroring ${latest.release.tag}`);
    await mirror(latest.release, latest.raw);
  }
  const version = latest?.release.version;
  if (version) await releaseBadges(version);

  // docs
  for (const file of readdirSync(join(SITE, "docs")).filter((f) => f.endsWith(".md"))) {
    const slug = basename(file, ".md");
    const { meta, body } = frontmatter(readFileSync(join(SITE, "docs", file), "utf8"));
    if (!meta.title) throw new Error(`${file}: no title`);
    const { html, toc } = render(body);
    const modified = lastChanged(`site/docs/${file}`);
    pages.set(slug, { slug, title: meta.title, nav: meta.nav, description: meta.description ?? "", html, toc, source: file, modified });
  }
  const releaseList = await releases().catch((e) => {
    console.warn(`changelog unavailable: ${e.message}`);
    return [];
  });
  pages.set("changelog", {
    slug: "changelog",
    title: "Changelog",
    description: CHANGELOG_DESCRIPTION,
    html: changelog(releaseList),
    toc: [],
    modified: releaseList[0]?.published_at,
  });
  const listed = new Set(NAV.flatMap((g) => g.pages));
  for (const slug of pages.keys()) if (!listed.has(slug)) throw new Error(`site/docs/${slug}.md is not in NAV`);
  for (const slug of listed) if (!pages.has(slug)) throw new Error(`NAV lists ${slug}, which has no page`);

  for (const page of pages.values()) {
    const dir = join(OUT, url(page.slug));
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, "index.html"), docPage(page, version));
  }

  // links between pages must land somewhere
  for (const page of pages.values()) {
    for (const [, slug, hash] of page.html.matchAll(/href="\/docs\/(?:([\w-]+)\/)?(#[\w-]+)?"/g)) {
      const target = pages.get(slug ?? "index");
      if (!target) throw new Error(`${page.slug}: link to missing page ${slug}`);
      if (hash && target.slug !== "changelog" && !target.toc.some((t) => `#${t.id}` === hash))
        throw new Error(`${page.slug}: link to missing section ${slug}${hash}`);
    }
  }

  // search index: one entry per page and per section
  const index: { t: string; h?: string; u: string; x: string }[] = [];
  for (const page of pages.values()) {
    if (page.slug === "changelog") continue;
    const parts = page.html.split(/(?=<h[23] id=")/);
    for (const part of parts) {
      const m = part.match(/^<h[23] id="([^"]+)">([\s\S]*?)<\/h[23]>/);
      const text = strip(m ? part.slice(m[0].length) : part)
        .replace(/\bCopy\b/g, "")
        .replace(/\s+/g, " ")
        .trim();
      index.push({
        t: page.title,
        h: m ? strip(m[2]).replace(/^#/, "") : undefined,
        u: url(page.slug) + (m ? `#${m[1]}` : ""),
        x: (m ? "" : `${page.description} `) + text.slice(0, 600),
      });
    }
  }
  mkdirSync(join(OUT, "assets"), { recursive: true });
  writeFileSync(join(OUT, "assets/search.json"), JSON.stringify(index));

  // The video loads nothing from YouTube until it's played: a poster and a link, swapped for the
  // player on click (site.js). Without scripts the link opens it on YouTube.
  const video = (id: string) =>
    `<figure class="video"><a class="video-play" href="https://www.youtube.com/watch?v=${id}" data-youtube="${id}"><img src="/img/video-poster.webp" width="1920" height="1080" alt="Inyon in 90 seconds: a coding agent asks a question in tmux, and the answer is tapped on a phone"><span class="play"><svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true"><path d="M8 5.5v13l11-6.5z"/></svg>Watch Inyon in 90 seconds</span></a></figure>`;
  // home and download pages
  const fill = (html: string) =>
    html
      .replaceAll("{{version}}", version ? esc(version) : "")
      .replaceAll("{{apk}}", latest ? "/download/inyon.apk" : `${GITHUB}/releases/latest`)
      .replaceAll(
        "{{apk-meta}}",
        latest
          ? `Inyon ${esc(latest.release.version)} · ${mb(latest.release.assets.find((a) => a.name.endsWith(".apk"))?.size ?? 0)}`
          : "From the latest release",
      )
      .replace("{{downloads}}", () => downloadTable(latest?.release))
      .replaceAll("{{github}}", GITHUB)
      .replace(/<!--shot-->[\s\S]*?<!--\/shot-->/, (shot) => (YOUTUBE_ID ? video(YOUTUBE_ID) : shot));
  const home = readFileSync(join(SITE, "pages/home.html"), "utf8");
  // the app as search engines describe it, on the home and download pages
  const app = {
    "@type": "SoftwareApplication",
    "@id": `${ORIGIN}/#app`,
    name: "Inyon",
    description: HOME.description,
    url: `${ORIGIN}/`,
    applicationCategory: "DeveloperApplication",
    operatingSystem: "Android, Linux, macOS, Windows",
    ...(latest ? { softwareVersion: latest.release.version, dateModified: latest.raw.published_at } : {}),
    downloadUrl: `${ORIGIN}/download/`,
    releaseNotes: `${ORIGIN}/docs/changelog/`,
    screenshot: `${ORIGIN}/img/hero-light.webp`,
    image: ogImage(cardFor("/")),
    offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
    license: "https://opensource.org/licenses/MIT",
    isAccessibleForFree: true,
    author: AUTHOR,
    sameAs: [GITHUB],
  };
  const website = {
    "@type": "WebSite",
    "@id": `${ORIGIN}/#website`,
    name: "Inyon",
    url: `${ORIGIN}/`,
    inLanguage: "en",
    publisher: AUTHOR,
  };
  writeFileSync(
    join(OUT, "index.html"),
    layout({
      title: HOME.title,
      description: HOME.description,
      path: "/",
      body: fill(home),
      version,
      bodyClass: "home",
      card: cardFor("/"),
      data: [website, app],
    }),
  );
  mkdirSync(join(OUT, "download"), { recursive: true });
  writeFileSync(
    join(OUT, "download/index.html"),
    layout({
      title: "Download Inyon for Android, Linux, macOS and Windows",
      description: DOWNLOAD_DESCRIPTION,
      path: "/download/",
      body: fill(readFileSync(join(SITE, "pages/download.html"), "utf8")),
      version,
      card: cardFor("/download/"),
      data: [app],
    }),
  );
  if (latest) {
    const r = latest.release;
    writeFileSync(
      join(OUT, "download/latest.json"),
      JSON.stringify({ version: r.version, tag: r.tag, date: r.date, files: r.assets }, null, 2),
    );
  }
  for (const slug of LEGAL) {
    const { meta, body } = legalMeta(slug);
    if (!meta.title || !meta.description || !meta.updated) throw new Error(`site/legal/${slug}.md needs a title, description and updated date`);
    mkdirSync(join(OUT, slug), { recursive: true });
    writeFileSync(
      join(OUT, slug, "index.html"),
      legalPage({ slug, title: meta.title, description: meta.description, html: render(body).html, updated: meta.updated, version, source: `site/legal/${slug}.md` }),
    );
  }
  mkdirSync(join(OUT, "license"), { recursive: true });
  writeFileSync(
    join(OUT, "license", "index.html"),
    legalPage({
      slug: "license",
      title: "MIT licence",
      description: LICENSE_DESCRIPTION,
      html: `<pre class="licence"><code>${esc(readFileSync(join(ROOT, "LICENSE"), "utf8").trim())}</code></pre>\n<p>Inyon is built on open-source libraries, each under its own licence. The repository’s <code>Cargo.lock</code> and <code>bun.lock</code> list every one. This site uses the Geist and Geist Mono fonts, under the <a href="/assets/fonts/geist-OFL.txt">SIL Open Font License</a>.</p>`,
      updated: lastChanged("LICENSE"),
      version,
      source: "LICENSE",
    }),
  );
  mkdirSync(join(OUT, ".well-known"), { recursive: true });
  writeFileSync(join(OUT, ".well-known/security.txt"), securityTxt(new Date()));

  writeFileSync(
    join(OUT, "404.html"),
    layout({
      title: "Page not found · Inyon",
      description: "This page doesn't exist.",
      path: "/404.html",
      body: readFileSync(join(SITE, "pages/404.html"), "utf8"),
      version,
    }),
  );

  // static files
  cpSync(join(SITE, "assets"), join(OUT, "assets"), { recursive: true });
  cpSync(join(SITE, "sw.js"), join(OUT, "sw.js"));
  cpSync(join(ROOT, "apps/web/public/icon.svg"), join(OUT, "icon.svg"));
  mkdirSync(join(OUT, "img"), { recursive: true });
  for (const f of readdirSync(join(ROOT, "docs")).filter((f) => /\.(webp|png|svg)$/.test(f)))
    cpSync(join(ROOT, "docs", f), join(OUT, "img", f));
  // link previews and the bookmark icon
  mkdirSync(join(OUT, "og"), { recursive: true });
  for (const f of readdirSync(join(SITE, "og")).filter((f) => f.endsWith(".png")))
    cpSync(join(SITE, "og", f), join(OUT, f === "apple-touch-icon.png" ? f : `og/${f}`));
  if (existsSync(join(SITE, "img")))
    for (const f of readdirSync(join(SITE, "img"))) cpSync(join(SITE, "img", f), join(OUT, "img", f));
  writeFileSync(join(OUT, ".nojekyll"), "");
  writeFileSync(join(OUT, "CNAME"), `${new URL(ORIGIN).host}\n`);
  const order = NAV.flatMap((g) => g.pages);
  const sitemap: [string, string | undefined][] = [
    ["/", lastChanged("site/pages/home.html")],
    ["/download/", newest(lastChanged("site/pages/download.html"), latest?.raw.published_at)],
    ...LEGAL.map((slug): [string, string | undefined] => [`/${slug}/`, legalMeta(slug).meta.updated]),
    ["/license/", lastChanged("LICENSE")],
    ...order.map((slug): [string, string | undefined] => [url(slug), pages.get(slug)!.modified]),
  ];
  writeFileSync(
    join(OUT, "sitemap.xml"),
    `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${sitemap
      .map(([p, d]) => `  <url><loc>${ORIGIN}${p}</loc>${d ? `<lastmod>${d.slice(0, 10)}</lastmod>` : ""}</url>`)
      .join("\n")}\n</urlset>\n`,
  );
  writeFileSync(join(OUT, "robots.txt"), `User-agent: *\nAllow: /\nSitemap: ${ORIGIN}/sitemap.xml\n`);
  writeFileSync(join(OUT, "releases.xml"), feed(releaseList));
  writeLlms(order);
  if (stale.size)
    console.warn(`link previews out of date for ${[...stale].join(", ")}: run bun site/og.ts (they use the home page's for now)`);
  console.log(`built ${pages.size} docs pages into ${OUT}${version ? ` (Inyon ${version})` : ""}`);
}

// ---------- llms.txt ----------

/** The docs' Markdown with its links made absolute, so it reads correctly off the site. */
const absolute = (md: string) =>
  md
    .replace(/\]\(([\w-]+)\.md(#[\w-]*)?\)/g, (_, slug: string, hash = "") => `](${ORIGIN}${url(slug)}${hash})`)
    .replace(/\]\(\.\.\/\.\.\/docs\/([\w.-]+)\)/g, `](${ORIGIN}/img/$1)`)
    .replace(/\]\(\.\.\/\.\.\/(SECURITY|CONTRIBUTING|LICENSE|NOTICE)(\.md)?\)/g, `](${GITHUB}/blob/main/$1$2)`);

/** /llms.txt (an index of the docs for AI assistants and search tools, llmstxt.org) and
 *  /llms-full.txt (every docs page's Markdown in one file). */
function writeLlms(order: string[]) {
  const sections = NAV.map(
    (g) =>
      `## ${g.title}\n\n${g.pages
        .map((slug) => {
          const p = pages.get(slug)!;
          return `- [${p.title}](${ORIGIN}${url(slug)})${p.description ? `: ${p.description}` : ""}`;
        })
        .join("\n")}`,
  );
  writeFileSync(
    join(OUT, "llms.txt"),
    `# Inyon\n\n> ${HOME.description}\n\nInyon runs on your own computers and phone; this site has its documentation and downloads. The source is at ${GITHUB} (MIT). Every docs page in one file: ${ORIGIN}/llms-full.txt\n\n${sections.join("\n\n")}\n\n## Optional\n\n- [Download](${ORIGIN}/download/): ${DOWNLOAD_DESCRIPTION}\n`,
  );
  const full = order
    .filter((slug) => pages.get(slug)?.source)
    .map((slug) => {
      const p = pages.get(slug)!;
      const { body } = frontmatter(readFileSync(join(SITE, "docs", p.source!), "utf8"));
      return `# ${p.title}\n\nSource: ${ORIGIN}${url(slug)}\n\n${p.description ? `${p.description}\n\n` : ""}${absolute(body).trim()}\n`;
    });
  writeFileSync(join(OUT, "llms-full.txt"), `${full.join("\n---\n\n")}`);
}

// ---------- download table ----------

// the .gz file each system needs: inyon alone, Alpine sharing Linux's static build. The
// inyon-engine-* files are for older versions of inyon (from 0.23 copies of it), listed with the rest
const PLATFORMS: { os: string; arch: string; match: RegExp }[] = [
  { os: "Linux", arch: "x64", match: /^inyon-linux-x64\.gz$/ },
  { os: "Linux", arch: "arm64", match: /^inyon-linux-arm64\.gz$/ },
  { os: "Alpine (musl)", arch: "x64", match: /^inyon-linux-x64\.gz$/ },
  { os: "Alpine (musl)", arch: "arm64", match: /^inyon-linux-arm64\.gz$/ },
  { os: "macOS", arch: "Apple silicon", match: /^inyon-darwin-arm64\.gz$/ },
  { os: "macOS", arch: "Intel", match: /^inyon-darwin-x64\.gz$/ },
  { os: "Windows", arch: "x64", match: /^inyon-windows-x64\.exe\.gz$/ },
  { os: "Windows", arch: "arm64", match: /^inyon-windows-arm64\.exe\.gz$/ },
];

export function downloadTable(r?: Release): string {
  if (!r) return `<p>The files are on <a class="ext" href="${GITHUB}/releases/latest">the latest release</a>.</p>`;
  const link = (a: Asset) =>
    `<a href="/download/${encodeURIComponent(a.name)}" download><code>${esc(a.name)}</code></a><span class="size">${mb(a.size)}</span>`;
  const rows = PLATFORMS.map((p) => {
    const files = r.assets.filter((a) => p.match.test(a.name));
    if (!files.length) return "";
    return `<tr><th scope="row">${p.os}<span>${p.arch}</span></th><td>${files.map(link).join("<br>")}</td></tr>`;
  }).join("");
  // the rest: anything that isn't one of those programs, compressed or not (the engine files for
  // older versions, the web app's tarball, the APK, the checksums and their signature, the
  // installers), a file with a .gz beside it once
  const programs = new Set(r.assets.filter((a) => PLATFORMS.some((p) => p.match.test(a.name))).map((a) => a.name));
  const names = new Set(r.assets.map((a) => a.name));
  const extra = r.assets.filter((a) => !programs.has(a.name) && !programs.has(`${a.name}.gz`) && !names.has(`${a.name}.gz`));
  return `<div class="table"><table class="files"><thead><tr><th>System</th><th>Files (gzip)</th></tr></thead><tbody>${rows}</tbody></table></div>
<p class="note">${r.gzOnly ? `The uncompressed copies of this release are on <a class="ext" href="${GITHUB}/releases/tag/${esc(r.tag)}">GitHub</a>.` : "Each file is also served uncompressed, without the <code>.gz</code>."} The installers pick the right ones for you.</p>
<h2 id="everything-else">Everything else</h2>
<div class="table"><table class="files"><tbody>${extra.map((a) => `<tr><td>${link(a)}</td></tr>`).join("")}</tbody></table></div>`;
}

if (import.meta.main) await main();
