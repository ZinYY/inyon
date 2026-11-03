package tech.abhashchakraborty.inyon.device

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class AnswerButtonsTest {
    @Test
    fun approvingNeedsAnUnlockAndTurningDownDoesNot() {
        val approval = listOf(Choice(1, "Yes"), Choice(2, "Yes, and don't ask again for git commands"), Choice(3, "No, and tell Claude what to do differently"))
        assertEquals(listOf(true, true, false), Notifications.answerButtons(approval).map { it.second })
        for (no in listOf("No", "Deny", "  reject", "Don't allow", "do not run", "Cancel", "Skip this one")) assertTrue(no, Notifications.declines(no))
        // "Nothing", "Stopwatch" and "Notes" only start like a refusal
        for (yes in listOf("Allow once", "Nothing else", "Stopwatch mode", "Notes first", "Yes")) assertFalse(yes, Notifications.declines(yes))
    }

    @Test
    fun aQuestionWithMoreThanThreeAnswersGetsNoButtons() {
        val four = (1..4).map { Choice(it, "Option $it") }
        assertTrue(Notifications.answerButtons(four).isEmpty())
        assertTrue(Notifications.answerButtons(emptyList()).isEmpty())
        assertEquals(listOf(1, 2, 3), Notifications.answerButtons(four.take(3)).map { it.first.n })
    }
}
