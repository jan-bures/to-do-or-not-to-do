package app.todoornot.session

/** Personal to this installation, independent of the paired device or server. */
enum class ThumbHand(val label: String, internal val value: String) {
    RIGHT("Right-handed", "right"),
    LEFT("Left-handed", "left");

    fun mirrors(wide: Boolean): Boolean = this == LEFT && !wide
}

internal class ThumbHandPreference(private val values: KeyValues) {
    fun load(): ThumbHand = ThumbHand.entries.firstOrNull { it.value == values.get(KEY) } ?: ThumbHand.RIGHT

    fun save(hand: ThumbHand): Boolean = values.put(mapOf(KEY to hand.value))

    private companion object { const val KEY = "thumb_zone_hand" }
}
