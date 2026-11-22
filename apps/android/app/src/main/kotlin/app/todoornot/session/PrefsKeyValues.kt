package app.todoornot.session

import android.content.Context
import android.content.SharedPreferences

/** [KeyValues] over the app's private SharedPreferences (excluded from backups). */
class PrefsKeyValues(context: Context, name: String = "session") : KeyValues {
    private val prefs: SharedPreferences = context.getSharedPreferences(name, Context.MODE_PRIVATE)

    override fun get(key: String): String? = prefs.getString(key, null)

    override fun put(values: Map<String, String?>): Boolean {
        val e = prefs.edit()
        values.forEach { (k, v) -> if (v == null) e.remove(k) else e.putString(k, v) }
        // commit, not apply: callers can distinguish an in-memory edit from a durable write.
        return e.commit()
    }
}
