package tech.abhashchakraborty.inyon.ssh

import java.io.IOException
import java.nio.ByteBuffer
import java.util.Base64

/**
 * Reads an unencrypted OpenSSH ed25519 private key ("-----BEGIN OPENSSH PRIVATE KEY-----") into
 * the 32-byte seed and 32-byte public key, the form the app passes keys it generated itself.
 */
class OpenSshEd25519 private constructor(val seed: ByteArray, val publicKey: ByteArray) {

    companion object {
        fun parse(text: String): OpenSshEd25519 {
            val body = text
                .replace("-----BEGIN OPENSSH PRIVATE KEY-----", "")
                .replace("-----END OPENSSH PRIVATE KEY-----", "")
                .replace(Regex("\\s"), "")
            val b = ByteBuffer.wrap(Base64.getDecoder().decode(body))
            val magic = "openssh-key-v1\u0000".toByteArray(Charsets.US_ASCII)
            val head = ByteArray(magic.size)
            b.get(head)
            if (!head.contentEquals(magic)) throw IOException("not an OpenSSH private key")
            val cipher = string(b)
            string(b) // kdf name
            bytes(b) // kdf options
            if (cipher != "none") throw IOException("encrypted keys are not supported here")
            if (b.getInt() != 1) throw IOException("expected exactly one key")
            bytes(b) // public key blob
            val priv = ByteBuffer.wrap(bytes(b))
            if (priv.getInt() != priv.getInt()) throw IOException("corrupt private key (check ints differ)")
            val type = string(priv)
            if (type != "ssh-ed25519") throw IOException("not an ed25519 key: $type")
            val pub = bytes(priv)
            val secret = bytes(priv) // seed followed by the public key
            if (pub.size != 32 || secret.size != 64) throw IOException("unexpected ed25519 key sizes")
            return OpenSshEd25519(secret.copyOf(32), pub)
        }

        private fun bytes(b: ByteBuffer): ByteArray = ByteArray(b.getInt()).also { b.get(it) }

        private fun string(b: ByteBuffer) = String(bytes(b), Charsets.UTF_8)
    }
}
