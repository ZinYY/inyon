import { type Scenario } from '../lib/scenario.ts'

/**
 * What machines download from the hub. Downloads are written down as size and checksum; the
 * harness puts placeholder builds where a checkout's cargo builds go (see seedCommands in
 * lib/hub.ts), so nothing is built or fetched.
 */
export default {
  name: 'node-dist',
  title: "Downloads for machines: the install scripts, the checksum list, programs and their checksums, bad requests",
  async run(ctx) {
    await ctx.step('install scripts and the checksum list (no sign-in needed)')
    await ctx.rest('GET', '/node/install.sh', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/install.ps1', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/SHA256SUMS', undefined, { as: 'anon' })
    // a hub running from a checkout has no signed release list, nor a certificate of its own
    for (const r of ['/node/release/SHA256SUMS', '/node/release/SHA256SUMS.minisig', '/node/release/version', '/node/hub.crt']) await ctx.rest('GET', r, undefined, { as: 'anon' })

    await ctx.step('the inyon command and the engine for Linux and macOS')
    for (const q of ['os=linux&arch=x64', 'os=linux&arch=arm64', 'os=darwin&arch=arm64']) {
      await ctx.rest('GET', `/node/inyon?${q}`, undefined, { as: 'anon', binary: true })
      await ctx.rest('GET', `/node/inyon.sha256?${q}`, undefined, { as: 'anon' })
      await ctx.rest('GET', `/node/inyon-engine.sha256?${q}`, undefined, { as: 'anon' })
    }
    await ctx.rest('GET', '/node/inyon.gz?os=linux&arch=x64', undefined, { as: 'anon', binary: true })
    await ctx.rest('GET', '/node/inyon-engine?os=linux&arch=x64', undefined, { as: 'anon', binary: true })
    await ctx.rest('GET', '/node/inyon-engine.gz?os=darwin&arch=x64', undefined, { as: 'anon', binary: true })

    await ctx.step('Windows: the inyon command, the engine, busybox checksums, the names from before the rename')
    await ctx.rest('GET', '/node/inyon.sha256?os=windows&arch=x64', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/inyon.sha256?os=windows&arch=arm64', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/inyon-engine.sha256?os=windows', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/busybox.exe.sha256', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/busybox.exe.sha256?arch=arm64', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/tether-node.exe.sha256', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/tether-node.exe', undefined, { as: 'anon', binary: true })
    await ctx.rest('GET', '/node/tether-node.ts', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/tether-node.ts.sha256', undefined, { as: 'anon' })

    await ctx.step('systems the hub has nothing for')
    await ctx.rest('GET', '/node/inyon?os=plan9&arch=x64', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/inyon-engine.sha256?os=linux&arch=mips', undefined, { as: 'anon' })
    await ctx.rest('GET', '/node/inyon.sha256?os=freebsd', undefined, { as: 'anon' })
  },
} satisfies Scenario
