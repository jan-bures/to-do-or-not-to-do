package app.todoornot

import android.app.Application
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.widget.WidgetChoicesStore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(application = Application::class, sdk = [35])
class WidgetChoicesStoreTest {
    @Test
    fun instances_remember_their_own_targets_and_removing_one_keeps_the_other() {
        val context = RuntimeEnvironment.getApplication()
        val store = WidgetChoicesStore(context)
        val list = RoomKey.List("test-list")
        val smart = RoomKey.Smart(SmartList.MY_DAY)
        val collection = RoomKey.Collection("test-collection")
        assertNull(store.get(20))
        assertTrue(store.put(20, list))
        assertTrue(store.put(21, smart))
        assertEquals(list, WidgetChoicesStore(context).get(20))
        assertEquals(smart, WidgetChoicesStore(context).get(21))
        assertTrue(store.put(20, collection))
        assertEquals(collection, store.get(20))
        store.remove(21)
        assertNull(store.get(21))
        assertEquals(collection, store.get(20))
    }
}
