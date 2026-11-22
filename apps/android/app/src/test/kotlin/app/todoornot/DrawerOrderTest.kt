package app.todoornot

import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.ui.activityAt
import org.junit.Assert.assertEquals
import org.junit.Test

/** The drawer keeps the order every client shares: Activity right after Agent activity. */
class DrawerOrderTest {
    private val smart = fixtureEngine().home(FIXTURE_NOW).smart

    private fun keys() = smart.map { (it.key as RoomKey.Smart).list }

    @Test
    fun activity_follows_agent_activity() {
        assertEquals(
            listOf(SmartList.MY_DAY, SmartList.IMPORTANT, SmartList.PLANNED, SmartList.ALL, SmartList.COMPLETED, SmartList.AGENT_ACTIVITY),
            keys(),
        )
        assertEquals(6, activityAt(smart))
        // Wherever Agent activity is, Activity comes next.
        val moved = smart.toMutableList().apply { add(0, removeAt(5)) }
        assertEquals(1, activityAt(moved))
    }

    @Test
    fun without_agent_activity_activity_closes_the_smart_lists() {
        val without = smart.filter { (it.key as RoomKey.Smart).list != SmartList.AGENT_ACTIVITY }
        assertEquals(without.size, activityAt(without))
        assertEquals(0, activityAt(emptyList()))
    }
}
