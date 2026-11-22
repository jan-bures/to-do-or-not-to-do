package app.todoornot.reminders

/** What reminders may need from the phone. */
enum class ReminderNeed {
    /** Permission to post notifications (Android 13 and later ask for it). */
    NOTIFICATIONS,

    /** Exact alarms ("Alarms & reminders", Android 12 and later), so a reminder isn't late. */
    EXACT_ALARMS,
}

/**
 * Asks for what reminders need, at a sensible moment: right after the owner sets a reminder, never at
 * launch. Each is asked once; a "Not now" is kept. The main activity does it; the gallery has [None].
 */
interface ReminderAccess {
    /** What to ask for now, or null. */
    fun toAsk(): ReminderNeed?

    /** The owner agreed: Android's own question (notifications), or its settings page (exact alarms). */
    fun allow(need: ReminderNeed)

    /** The owner said not now: it isn't asked again. */
    fun decline(need: ReminderNeed)

    object None : ReminderAccess {
        override fun toAsk(): ReminderNeed? = null
        override fun allow(need: ReminderNeed) = Unit
        override fun decline(need: ReminderNeed) = Unit
    }

    companion object {
        /**
         * Which to ask for: notifications first (only Android 13 and later can ask, at [sdk] 33), then exact
         * alarms (Android 12 and later, at [sdk] 31), each only if missing and not [asked] before.
         */
        fun next(sdk: Int, notifications: Boolean, exact: Boolean, asked: Set<ReminderNeed>): ReminderNeed? = when {
            sdk >= 33 && !notifications && ReminderNeed.NOTIFICATIONS !in asked -> ReminderNeed.NOTIFICATIONS
            sdk >= 31 && !exact && ReminderNeed.EXACT_ALARMS !in asked -> ReminderNeed.EXACT_ALARMS
            else -> null
        }
    }
}
