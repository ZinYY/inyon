/**
 * SHA-256 a piece at a time, for checking an upload arrived whole. Web Crypto can't do this: its
 * digest wants the whole file in memory at once (a video on a phone), and it's missing on a hub
 * reached over plain http, which browsers don't count as a secure context.
 */

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
])

export class Sha256 {
  private h = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19])
  private w = new Uint32Array(64)
  /** bytes of a block still short of 64 */
  private tail = new Uint8Array(64)
  private held = 0
  private length = 0

  update(data: Uint8Array): this {
    let at = 0
    this.length += data.length
    if (this.held) {
      const n = Math.min(64 - this.held, data.length)
      this.tail.set(data.subarray(0, n), this.held)
      this.held += n
      at = n
      if (this.held < 64) return this
      this.block(this.tail, 0)
      this.held = 0
    }
    for (; at + 64 <= data.length; at += 64) this.block(data, at)
    this.tail.set(data.subarray(at), 0)
    this.held = data.length - at
    return this
  }

  /** The hash of everything so far, as 64 hex digits. The hasher can't take more after this. */
  hex(): string {
    const bits = this.length * 8
    const pad = new Uint8Array((this.held < 56 ? 64 : 128) - this.held)
    pad[0] = 0x80
    const end = new DataView(pad.buffer)
    end.setUint32(pad.length - 8, Math.floor(bits / 2 ** 32))
    end.setUint32(pad.length - 4, bits >>> 0)
    this.update(pad)
    return [...this.h].map((x) => x.toString(16).padStart(8, '0')).join('')
  }

  private block(b: Uint8Array, at: number) {
    const w = this.w
    for (let i = 0; i < 16; i++) w[i] = (b[at + i * 4] << 24) | (b[at + i * 4 + 1] << 16) | (b[at + i * 4 + 2] << 8) | b[at + i * 4 + 3]
    for (let i = 16; i < 64; i++) {
      const x = w[i - 15], y = w[i - 2]
      const s0 = ((x >>> 7) | (x << 25)) ^ ((x >>> 18) | (x << 14)) ^ (x >>> 3)
      const s1 = ((y >>> 17) | (y << 15)) ^ ((y >>> 19) | (y << 13)) ^ (y >>> 10)
      w[i] = w[i - 16] + s0 + w[i - 7] + s1
    }
    const s = this.h
    let a = s[0], c1 = s[1], c2 = s[2], d = s[3], e = s[4], f = s[5], g = s[6], h = s[7]
    for (let i = 0; i < 64; i++) {
      const s1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7))
      const t1 = (h + s1 + ((e & f) ^ (~e & g)) + K[i] + w[i]) | 0
      const s0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10))
      const t2 = (s0 + ((a & c1) ^ (a & c2) ^ (c1 & c2))) | 0
      h = g
      g = f
      f = e
      e = (d + t1) | 0
      d = c2
      c2 = c1
      c1 = a
      a = (t1 + t2) | 0
    }
    s[0] += a
    s[1] += c1
    s[2] += c2
    s[3] += d
    s[4] += e
    s[5] += f
    s[6] += g
    s[7] += h
  }
}
