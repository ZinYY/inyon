package tech.abhashchakraborty.inyon.migrate

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.io.IOException
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * files/inyon-migrate.bin: the phone's identity, pairings, hubs and server sign-ins, sealed
 * ([MigrationSeal]) under an AES key that never leaves the Keystore, for the Rust app that will
 * install over this one. It has the same app id and signing key, so the same files folder and
 * Keystore entries; it calls [read] and, once its own store holds the result, [discard]. Backups
 * and transfers to a new phone leave the file behind (data_extraction_rules.xml).
 */
object MigrationExport {
    const val PAYLOAD_VERSION = 1
    private const val ALIAS = "inyon-migrate-v1"
    private const val FILE = "inyon-migrate.bin"

    /** Seals and replaces the file in one step: a crash leaves the previous file, never half of one. */
    fun write(context: Context, plaintext: ByteArray) {
        val sealed = try {
            MigrationSeal.seal(key(create = true)!!, plaintext)
        } catch (_: KeyPermanentlyInvalidatedException) {
            keyStore().deleteEntry(ALIAS)
            MigrationSeal.seal(key(create = true)!!, plaintext)
        }
        val file = AtomicFile(File(context.filesDir, FILE))
        val out = file.startWrite()
        try {
            out.write(sealed)
            file.finishWrite(out) // fsync, then rename over the old file
        } catch (e: IOException) {
            file.failWrite(out)
            throw e
        }
    }

    /**
     * The plaintext JSON, or null when there is nothing to import here: no file, an unknown format,
     * no key (a restore onto another phone, a wiped Keystore) or a failed tag. Never throws.
     */
    @JvmStatic
    fun read(context: Context): ByteArray? {
        val f = File(context.filesDir, FILE)
        if (!f.exists()) return null
        return try {
            val bytes = AtomicFile(f).readFully()
            if (!MigrationSeal.readable(bytes)) return null
            MigrationSeal.open(key(create = false) ?: return null, bytes)
        } catch (_: GeneralSecurityException) {
            null
        } catch (_: IOException) {
            null
        }
    }

    /** Deletes the file and the key. Call it only after the import is committed. */
    @JvmStatic
    fun discard(context: Context) {
        AtomicFile(File(context.filesDir, FILE)).delete()
        val ks = keyStore()
        if (ks.containsAlias(ALIAS)) ks.deleteEntry(ALIAS)
    }

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    /**
     * No user authentication and no unlocked-device requirement: the export runs as the app goes
     * to the background, often with the screen off. No StrongBox: most phones don't have one, and
     * a TEE key is enough for a file that lives from one install to the next.
     */
    private fun key(create: Boolean): SecretKey? {
        (keyStore().getKey(ALIAS, null) as? SecretKey)?.let { return it }
        if (!create) return null
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            .apply { init(spec) }
            .generateKey()
    }
}
