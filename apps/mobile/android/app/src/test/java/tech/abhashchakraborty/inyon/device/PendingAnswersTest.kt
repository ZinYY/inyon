package tech.abhashchakraborty.inyon.device

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PendingAnswersTest {
    private fun tap(tag: String, label: String, at: Long) = PendingAnswers.Pending(tag, label, "/m/box/p/3", at)

    @Test
    fun anAnswerTheWebAppConfirmsIsNoLongerWaitedFor() {
        val p = PendingAnswers()
        p.add(tap("box:%3", "Yes", 0))
        assertTrue(p.settle("box:%3"))
        assertTrue(p.isEmpty())
        // a notification for a session with no answer on the way settles nothing
        assertFalse(p.settle("box:%4"))
    }

    @Test
    fun onlyAnswersPastTheLimitExpire() {
        val p = PendingAnswers()
        p.add(tap("a", "Yes", 0))
        p.add(tap("b", "No", 10_000))
        assertEquals(emptyList<String>(), p.expire(14_999, AnswerService.TTL_MS).map { it.tag })
        assertEquals(listOf("a"), p.expire(15_000, AnswerService.TTL_MS).map { it.tag })
        assertEquals("b", p.latest()?.tag)
        assertEquals(listOf("b"), p.expire(25_000, AnswerService.TTL_MS).map { it.tag })
        assertTrue(p.isEmpty())
    }

    @Test
    fun aSecondTapOnTheSameSessionReplacesTheFirst() {
        val p = PendingAnswers()
        p.add(tap("a", "Yes", 0))
        p.add(tap("b", "Yes", 1_000))
        p.add(tap("a", "No", 2_000))
        assertEquals("No", p.latest()?.label)
        // the first tap's clock no longer counts
        assertEquals(listOf("b"), p.expire(16_000, AnswerService.TTL_MS).map { it.tag })
        assertTrue(p.settle("a"))
        assertTrue(p.isEmpty())
    }

    @Test
    fun theWebAppDropsATapBeforeThePhoneSaysItWasntSent() {
        // notifyAnswer.ts drops taps older than 12 s; the phone gives up at 15 s
        assertTrue(AnswerService.TTL_MS > 12_000)
    }
}
