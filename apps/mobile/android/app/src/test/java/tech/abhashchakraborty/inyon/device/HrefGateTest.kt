package tech.abhashchakraborty.inyon.device

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class HrefGateTest {
    private val mine = "a".repeat(64)

    @Test
    fun opensThePathOfItsOwnNotification() {
        assertEquals("/m/box/p/3", HrefGate.accept(Notifications.ACTION_OPEN_HREF, "/m/box/p/3", mine, mine))
    }

    @Test
    fun anotherAppsIntentOpensNothing() {
        // no nonce, a guessed one, or another action: what any app on the phone can send
        assertNull(HrefGate.accept(Notifications.ACTION_OPEN_HREF, "/machines?hub=inyon://hub?u=x", null, mine))
        assertNull(HrefGate.accept(Notifications.ACTION_OPEN_HREF, "/machines", "b".repeat(64), mine))
        assertNull(HrefGate.accept("android.intent.action.MAIN", "/machines", mine, mine))
        assertNull(HrefGate.accept(null, "/machines", mine, mine))
    }

    @Test
    fun aPathOutsideTheAppOpensNothing() {
        for (href in listOf("//evil.example/signin", "/\\evil.example", "https://evil.example/", "javascript:alert(1)", "")) {
            assertNull(href, HrefGate.accept(Notifications.ACTION_OPEN_HREF, href, mine, mine))
        }
    }
}
