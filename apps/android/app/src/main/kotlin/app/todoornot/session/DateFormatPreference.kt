package app.todoornot.session

import android.content.Context

/** A display preference, kept with the other local appearance settings. */
internal class DateFormatPreference(private val values: KeyValues) {
    constructor(context: Context) : this(PrefsKeyValues(context, "appearance"))

    fun load(): String? = values.get(KEY)
    fun save(key: String): Boolean = values.put(mapOf(KEY to key))

    private companion object { const val KEY = "date_format" }
}
