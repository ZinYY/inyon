package tech.abhashchakraborty.inyon.migrate

import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The bytes of files/inyon-migrate.bin, apart from where the key and the file live (see
 * [MigrationExport]):
 *
 *   "INYM" | format (1 byte, = 1) | IV length (1 byte, = 12) | IV | ciphertext + 16-byte tag
 *
 * AES-256-GCM, with the 18-byte header as associated data, so a changed header fails like a
 * changed body. The app that installs over this one (same app id and signing key) opens it.
 */
object MigrationSeal {
    private val MAGIC = byteArrayOf(0x49, 0x4E, 0x59, 0x4D) // "INYM"
    const val FORMAT: Byte = 1
    const val IV_BYTES = 12
    private const val TAG_BITS = 128
    const val HEADER_BYTES = 4 + 1 + 1 + IV_BYTES

    /** `plaintext` sealed under `key`, with an IV the cipher picks (the Keystore refuses one from us). */
    fun seal(key: SecretKey, plaintext: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key)
        val iv = cipher.iv
        check(iv != null && iv.size == IV_BYTES) { "unexpected IV length ${iv?.size}" }
        val header = MAGIC + byteArrayOf(FORMAT, IV_BYTES.toByte()) + iv
        cipher.updateAAD(header)
        return header + cipher.doFinal(plaintext)
    }

    /** Whether `bytes` start like a file this build can open: the magic, format 1 and a 12-byte IV. */
    fun readable(bytes: ByteArray): Boolean =
        bytes.size >= HEADER_BYTES + TAG_BITS / 8 &&
            bytes.copyOfRange(0, 4).contentEquals(MAGIC) &&
            bytes[4] == FORMAT &&
            bytes[5].toInt() == IV_BYTES

    /** The plaintext, or null for anything else: another format, a short file, a wrong key, a changed byte. */
    fun open(key: SecretKey, bytes: ByteArray): ByteArray? {
        if (!readable(bytes)) return null
        return try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, bytes, 6, IV_BYTES))
            cipher.updateAAD(bytes, 0, HEADER_BYTES)
            cipher.doFinal(bytes, HEADER_BYTES, bytes.size - HEADER_BYTES)
        } catch (_: GeneralSecurityException) {
            null
        }
    }
}
