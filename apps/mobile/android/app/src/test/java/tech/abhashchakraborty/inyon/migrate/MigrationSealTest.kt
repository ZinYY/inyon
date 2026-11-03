package tech.abhashchakraborty.inyon.migrate

import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MigrationSealTest {
    // a software key stands in for the Keystore's, which the JVM tests don't have
    private fun key(): SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    private val payload = """{"v":1,"secrets":{"phone-key":"seed"}}""".toByteArray()

    @Test
    fun opensWhatItSealedAndTheHeaderIsTheFormatItSays() {
        val k = key()
        val sealed = MigrationSeal.seal(k, payload)
        assertArrayEquals("INYM".toByteArray(), sealed.copyOfRange(0, 4))
        assertEquals(MigrationSeal.FORMAT, sealed[4])
        assertEquals(MigrationSeal.IV_BYTES, sealed[5].toInt())
        // header, then the ciphertext as long as the payload, then the 16-byte tag
        assertEquals(MigrationSeal.HEADER_BYTES + payload.size + 16, sealed.size)
        assertArrayEquals(payload, MigrationSeal.open(k, sealed))
    }

    @Test
    fun eachSealHasItsOwnIv() {
        val k = key()
        val a = MigrationSeal.seal(k, payload)
        val b = MigrationSeal.seal(k, payload)
        assertFalse(a.copyOfRange(6, MigrationSeal.HEADER_BYTES).contentEquals(b.copyOfRange(6, MigrationSeal.HEADER_BYTES)))
    }

    @Test
    fun anyChangedByteHeaderIncludedOpensNothing() {
        val k = key()
        val sealed = MigrationSeal.seal(k, payload)
        for (i in sealed.indices) {
            val changed = sealed.copyOf()
            changed[i] = (changed[i].toInt() xor 1).toByte()
            assertNull("byte $i", MigrationSeal.open(k, changed))
        }
    }

    @Test
    fun anotherKeyAShortFileOrAnotherFormatOpensNothing() {
        val sealed = MigrationSeal.seal(key(), payload)
        // a restore onto another phone: the file came, the key didn't
        assertNull(MigrationSeal.open(key(), sealed))
        assertNull(MigrationSeal.open(key(), sealed.copyOf(MigrationSeal.HEADER_BYTES + 15)))
        assertNull(MigrationSeal.open(key(), ByteArray(0)))
        val later = sealed.copyOf().also { it[4] = 2 }
        assertFalse(MigrationSeal.readable(later))
        assertTrue(MigrationSeal.readable(sealed))
    }
}
