package tech.abhashchakraborty.inyon.ssh

/**
 * Splits a byte stream into UTF-8 lines on '\n', dropping a trailing '\r'. Splitting happens on
 * bytes, so a multi-byte character cut across two reads still decodes correctly.
 */
class LineSplitter {

    private var carry = ByteArray(SMALL)
    private var carryLen = 0

    fun feed(b: ByteArray, off: Int, len: Int): List<String> {
        var out: MutableList<String>? = null
        var start = off
        val end = off + len
        for (i in off until end) {
            if (b[i] != NEWLINE) continue
            val line = if (carryLen == 0) {
                decode(b, start, i - start)
            } else {
                append(b, start, i - start)
                decode(carry, 0, carryLen).also { resetCarry() }
            }
            (out ?: ArrayList<String>().also { out = it }).add(line)
            start = i + 1
        }
        if (start < end) append(b, start, end - start)
        return out ?: emptyList()
    }

    /** The unterminated last line, or null when there is none. */
    fun finish(): String? {
        if (carryLen == 0) return null
        return decode(carry, 0, carryLen).also { resetCarry() }
    }

    private fun append(b: ByteArray, off: Int, len: Int) {
        if (len == 0) return
        if (carryLen + len > carry.size) carry = carry.copyOf(maxOf(carry.size * 2, carryLen + len))
        System.arraycopy(b, off, carry, carryLen, len)
        carryLen += len
    }

    private fun resetCarry() {
        carryLen = 0
        // don't keep a multi-MB buffer around after one huge line
        if (carry.size > 1 shl 20) carry = ByteArray(SMALL)
    }

    private companion object {
        const val SMALL = 256
        const val NEWLINE: Byte = 10 // '\n'
        const val RETURN: Byte = 13 // '\r'

        fun decode(b: ByteArray, off: Int, len: Int): String {
            val n = if (len > 0 && b[off + len - 1] == RETURN) len - 1 else len
            return String(b, off, n, Charsets.UTF_8)
        }
    }
}
