package app.todoornot

import app.todoornot.core.CacheRecovery
import app.todoornot.core.ChangeListener
import app.todoornot.core.EngineConfig
import app.todoornot.core.FfiException
import app.todoornot.core.RoomKey
import app.todoornot.core.SmartList
import app.todoornot.core.TodoEngine
import app.todoornot.model.TodoModel
import app.todoornot.session.EngineHost
import app.todoornot.session.Host
import app.todoornot.session.PairingFiles
import app.todoornot.session.Session
import app.todoornot.session.SessionStore
import app.todoornot.session.Unpaired
import app.todoornot.widget.WidgetData
import app.todoornot.widget.WidgetEdit
import app.todoornot.widget.WidgetTaskAction
import app.todoornot.widget.widgetSnapshot
import com.sun.net.httpserver.HttpServer
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File
import java.net.InetSocketAddress
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.io.path.createTempDirectory

/** Opening the engine for the saved pairing, what a cache that won't open does to it, and unpairing. */
class EngineHostTest {
    private val approved = Session("https://todo.example.com", "device-1", "tdd_secret-token")
    private val root: File = createTempDirectory("todo-host").toFile()
    private val values = MapValues()
    private val sessions = SessionStore(values, FakeCipher())
    private var keysForgotten = 0
    private var ids = 0

    /** Pairing IDs in order: 000…01, 000…02, … */
    private fun nextId() = "%032x".format(++ids)

    /** The caches engines were opened on, in order. */
    private val opened = mutableListOf<File>()

    @After
    fun tearDown() {
        root.deleteRecursively()
    }

    /**
     * A host over [root], as the app builds it after a (re)start. The fake engine writes its cache file,
     * as the real one would.
     */
    private fun host(
        files: PairingFiles = PairingFiles(root),
        open: (Session, File) -> TodoEngine = { _, db ->
            db.writeText("cache")
            fixtureEngine()
        },
    ) = EngineHost(sessions, files, forgetKey = { keysForgotten++ }, newPairingId = ::nextId) { s, db ->
        opened += db
        open(s, db)
    }

    private fun pairingDirs() = PairingFiles(root).pairings.listFiles()?.map { it.name }?.sorted() ?: emptyList()

    @Test
    fun a_cache_that_wont_open_keeps_the_pairing_and_retrying_opens_it() {
        val engine = fixtureEngine()
        var fail = true
        val host = host { _, _ -> if (fail) throw FfiException.Storage("database disk image is malformed") else engine }
        host.paired(approved)

        val cant = host.state.value as Host.CantOpen
        assertEquals(approved.server, cant.session.server)
        assertTrue(cant.problem.contains("Nothing unsent has been deleted"))
        // The pairing (and so the token) is still saved: nothing pushes the owner to pair again.
        assertEquals(cant.session, sessions.load())

        fail = false
        host.retry()
        val ready = host.state.value as Host.Ready
        assertSame(engine, ready.engine)
        assertNull(ready.recovered)
        assertEquals(cant.session, sessions.load())
        assertEquals(0, keysForgotten)
    }

    @Test
    fun a_401_survives_an_offline_restart_without_showing_widget_rows_or_accepting_writes() = runTest {
        fun realHost() = EngineHost(SessionStore(values, FakeCipher()), PairingFiles(root), forgetKey = {}, newPairingId = ::nextId) { s, db ->
            TodoEngine.open(EngineConfig(s.server, s.token, s.deviceId, db.absolutePath, true))
        }
        val server = HttpServer.create(InetSocketAddress("127.0.0.1", 0), 0)
        server.createContext("/") { exchange -> exchange.use { it.sendResponseHeaders(401, -1) } }
        server.start()
        val session = approved.copy(server = "http://127.0.0.1:${server.address.port}")
        val room = RoomKey.Smart(SmartList.ALL)
        val first = realHost().apply { paired(session) }
        val before = first.state.value as Host.Ready
        val task = before.engine.use { engine ->
            try {
                val id = engine.addTask(room, "Keep this unsent draft", FIXTURE_NOW)
                assertTrue(widgetSnapshot(first, room, FIXTURE_NOW).data is WidgetData.Room)
                val recorded = CountDownLatch(1)
                engine.setListener(object : ChangeListener {
                    override fun onChange() {
                        if (first.status(before).revoked) recorded.countDown()
                    }
                })
                engine.start()
                assertTrue("the 401 must be recorded before restart", recorded.await(8, TimeUnit.SECONDS))
                assertEquals(WidgetData.SignedOut, widgetSnapshot(first, room, FIXTURE_NOW).data)
                id
            } finally {
                engine.stop()
                server.stop(0)
            }
        }

        val restarted = realHost().apply { load() }
        val ready = restarted.state.value as Host.Ready
        val click = WidgetTaskAction(before.session.pairingId, WidgetEdit.Complete(task))
        ready.engine.use { engine ->
            assertEquals(before.session.pairingId, ready.session.pairingId)
            assertNull("the new engine has no network response", engine.status().problem)
            val pending = engine.status().pending
            assertFalse(engine.task(task, FIXTURE_NOW)!!.done)
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, now = { FIXTURE_NOW }, syncStatus = { restarted.status(ready) })
            assertTrue("the main app also shows Pair again", model.ui.value.signedOut)
            assertEquals(WidgetData.SignedOut, widgetSnapshot(restarted, room, FIXTURE_NOW).data)
            assertFalse(click.apply(restarted, model, FIXTURE_NOW))
            assertEquals(pending, engine.status().pending)
            assertFalse(engine.task(task, FIXTURE_NOW)!!.done)
        }

        assertEquals(Unpaired.Clean, restarted.unpair {})
        restarted.paired(session)
        val replacement = restarted.state.value as Host.Ready
        replacement.engine.use { engine ->
            assertNotEquals(ready.session.pairingId, replacement.session.pairingId)
            assertTrue(widgetSnapshot(restarted, room, FIXTURE_NOW).data is WidgetData.Room)
            val model = TodoModel(engine, backgroundScope, Dispatchers.Unconfined, now = { FIXTURE_NOW })
            assertFalse("an action retained from the old pairing must be refused", click.apply(restarted, model, FIXTURE_NOW))
        }
    }

    @Test
    fun each_pairing_opens_its_own_cache() {
        val host = host()
        host.paired(approved)
        assertEquals(Unpaired.Clean, host.unpair { it.close() })
        host.paired(approved.copy(deviceId = "device-2"))
        assertEquals(2, opened.size)
        assertNotEquals(opened[0], opened[1])
        assertEquals(PairingFiles(root).cache("%032x".format(2)), opened[1])
        assertFalse("the first pairing's cache is gone", opened[0].exists())
    }

    @Test
    fun unpairing_forgets_the_pairing_its_key_and_its_cache() {
        val host = host()
        host.paired(approved)
        val db = opened.single()
        assertTrue(db.exists())
        var closed = 0
        assertEquals(Unpaired.Clean, host.unpair { closed++ })
        assertEquals(1, closed)
        assertEquals(Host.SignedOut, host.state.value)
        assertNull(sessions.load())
        assertEquals(1, keysForgotten)
        assertFalse(db.exists())
        assertEquals(emptyList<String>(), pairingDirs())
    }

    @Test
    fun the_cant_open_state_can_be_unpaired_too() {
        val host = host { _, _ -> throw FfiException.Storage("disk full") }
        host.paired(approved)
        assertTrue(host.state.value is Host.CantOpen)
        assertEquals(Unpaired.Clean, host.unpair { error("no engine is open") })
        assertNull(sessions.load())
    }

    @Test
    fun if_the_intent_cant_be_recorded_nothing_is_removed() {
        val host = host()
        host.paired(approved)
        val id = sessions.load()!!.pairingId
        // A directory where the record goes: it can't be written.
        File(PairingFiles(root).pairings, "$id.signed-out").mkdirs()
        File(PairingFiles(root).pairings, "$id.signed-out/x").writeText("x")
        val result = host.unpair { error("must not close") }
        assertTrue(result is Unpaired.StillPaired)
        assertTrue(host.state.value is Host.Ready)
        assertEquals(id, sessions.load()?.pairingId)
        assertEquals(0, keysForgotten)
        assertTrue(opened.single().exists())
    }

    /** Killed after the record, before the pairing was forgotten: the next start finishes the unpair. */
    @Test
    fun an_unpair_cut_short_after_its_record_is_finished_at_the_next_start() {
        host().paired(approved)
        val id = sessions.load()!!.pairingId
        PairingFiles(root).markSignedOut(id)
        // The process dies here. Next start:
        val restarted = host()
        restarted.load()
        assertEquals(Host.SignedOut, restarted.state.value)
        assertNull(sessions.load())
        assertEquals(1, keysForgotten)
        assertEquals(emptyList<String>(), pairingDirs())
        assertEquals("the unpaired cache was never opened again", 1, opened.size)
    }

    /** Killed after the pairing was forgotten, before the data was removed: the next start removes it. */
    @Test
    fun an_unpair_cut_short_before_removal_is_swept_at_the_next_start() {
        host().paired(approved)
        val id = sessions.load()!!.pairingId
        PairingFiles(root).markSignedOut(id)
        sessions.clear()
        // The process dies here, the old cache still on disk.
        assertTrue(PairingFiles(root).cache(id).exists())
        host().load()
        assertEquals(emptyList<String>(), pairingDirs())
    }

    /** Removal fails (a file still open): unpaired anyway, and a new pairing never sees the old cache. */
    @Test
    fun data_that_couldnt_be_removed_is_never_opened_by_the_next_pairing() {
        var stuck = true
        val sticky = PairingFiles(root) { f -> if (stuck && f.name == "todo.sqlite") false else f.delete() }
        val host = host(files = sticky)
        host.paired(approved)
        val old = opened.single()
        assertEquals(Unpaired.DataLeft, host.unpair { it.close() })
        assertNull(sessions.load())
        assertTrue(old.exists())

        host.paired(approved.copy(deviceId = "device-2"))
        assertNotEquals(old, opened.last())
        assertTrue("still stuck, still marked", old.exists())

        stuck = false
        host(files = sticky).load()
        assertFalse(old.exists())
        assertEquals(listOf("%032x".format(2)), pairingDirs())
    }

    /** Never released, so not migrated: a cache from before pairing directories is left as it is. */
    @Test
    fun a_cache_from_before_pairing_directories_is_left_alone() {
        sessions.save(approved) // No pairing ID: the old layout.
        val old = File(root, "todo.sqlite").apply { writeText("old cache") }
        val host = host()
        host.load()
        assertTrue(host.state.value is Host.Ready)
        assertEquals("%032x".format(1), sessions.load()?.pairingId)
        assertNotEquals(old, opened.single())
        assertEquals("old cache", old.readText())
    }

    /** `pairings/` replaced by a link: nothing is opened, written or removed through it. */
    @Test
    fun a_linked_pairings_directory_is_never_used() {
        host().paired(approved)
        val id = sessions.load()!!.pairingId
        val pairings = PairingFiles(root).pairings
        val outside = createTempDirectory("todo-outside").toFile()
        try {
            val precious = File(outside, "$id/todo.sqlite").apply { parentFile?.mkdirs(); writeText("not ours") }
            pairings.deleteRecursively()
            assumeTrue("symlinks can't be made here", runCatching { Files.createSymbolicLink(pairings.toPath(), outside.toPath()) }.isSuccess)

            val restarted = host()
            restarted.load()
            assertEquals(EngineHost.FOLDER_PROBLEM, (restarted.state.value as Host.CantOpen).problem)
            assertEquals("only the first, real open", 1, opened.size)

            assertTrue(restarted.unpair { error("no engine is open") } is Unpaired.StillPaired)
            assertEquals(id, sessions.load()?.pairingId)
            assertEquals(0, keysForgotten)
            assertEquals("not ours", precious.readText())
            assertEquals(listOf(id), outside.list()?.toList())
        } finally {
            Files.deleteIfExists(pairings.toPath())
            outside.deleteRecursively()
        }
    }

    @Test
    fun a_real_engine_on_an_unreadable_cache_shows_cant_open_and_keeps_the_file() {
        val host = EngineHost(sessions, PairingFiles(root), forgetKey = {}, newPairingId = ::nextId) { s, db ->
            // Not a file SQLite can open: opening fails whatever the recovery does.
            db.mkdirs()
            TodoEngine.open(EngineConfig(s.server, s.token, s.deviceId, db.absolutePath, false))
        }
        host.paired(approved)
        val cant = host.state.value as Host.CantOpen
        assertEquals(cant.session, sessions.load())
        assertTrue(PairingFiles(root).cache(cant.session.pairingId).exists())
    }

    @Test
    fun the_recovery_notice_says_what_was_kept() {
        assertNull(EngineHost.recoveredNotice(null))
        assertEquals(
            "Recovered from a damaged copy: all 3 unsent changes kept.",
            EngineHost.recoveredNotice(CacheRecovery(3u, 0u)),
        )
        assertEquals(
            "Recovered from a damaged copy: 2 unsent changes kept, 1 couldn’t be read.",
            EngineHost.recoveredNotice(CacheRecovery(2u, 1u)),
        )
    }
}
