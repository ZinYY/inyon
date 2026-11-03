package tech.abhashchakraborty.inyon.device

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairLinkTest {
    @Test
    fun takesInyonNodeLinks() {
        assertTrue(isPairLink("inyon://pair?m=abc&k=def"))
        assertTrue(isPairLink("  inyon://pair?m=abc\n"))
        // printed by a laptop from before the rename to Inyon
        assertTrue(isPairLink("tether://pair?m=abc&k=def"))
        assertTrue(isPairLink("https://inyon.example/pair?m=abc"))
        assertTrue(isPairLink("https://inyon.example/p?v=1&m=abc"))
    }

    @Test
    fun refusesOtherCodes() {
        assertFalse(isPairLink(null))
        assertFalse(isPairLink(""))
        assertFalse(isPairLink("https://example.com/menu"))
        assertFalse(isPairLink("http://inyon.example/pair?m=abc"))
        assertFalse(isPairLink("WIFI:S:home;T:WPA;P:secret;;"))
        assertFalse(isPairLink("inyon://other?m=abc"))
        assertFalse(isPairLink("see inyon://pair?m=abc"))
    }
}
