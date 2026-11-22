package app.todoornot.model

import app.todoornot.core.ColorTarget
import app.todoornot.core.RepeatChoice

/** Edits from a picker carry the entity that was shown, never the current selection. */
sealed interface OrganizeEdit {
    data class Repeat(val task: String, val choice: RepeatChoice) : OrganizeEdit
    data class ToggleTag(val task: String, val id: String) : OrganizeEdit
    data class TagNamed(val task: String, val name: String) : OrganizeEdit
    data class RenameTag(val id: String, val name: String) : OrganizeEdit
    data class Color(val target: ColorTarget, val hue: String) : OrganizeEdit
    data class AddLink(val task: String, val address: String) : OrganizeEdit
    data class RemoveLink(val id: String) : OrganizeEdit
    data class NewCollection(val name: String) : OrganizeEdit
    data class MoveList(val id: String, val collection: String?) : OrganizeEdit
}
