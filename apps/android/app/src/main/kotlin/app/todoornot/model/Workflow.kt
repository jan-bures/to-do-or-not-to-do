package app.todoornot.model

import app.todoornot.core.BoardView
import app.todoornot.core.Home
import app.todoornot.core.ListFeatures
import app.todoornot.core.ListView
import app.todoornot.core.OpenStage
import app.todoornot.core.RoomKey
import app.todoornot.core.RoomView
import app.todoornot.core.TaskRow
import app.todoornot.core.WorkflowStage

internal val ChecklistFeatures = ListFeatures(false, false, ListView.LIST)

internal fun Home.featuresFor(list: RoomKey): ListFeatures? =
    groups.firstNotNullOfOrNull { group -> group.lists.firstOrNull { it.key == list }?.features }

/** A synced Timeline default is preserved, but this Android release opens it as List. */
internal fun androidView(features: ListFeatures?, override: ListView?): ListView =
    if (features?.workflow == true && (override ?: features.defaultView) == ListView.BOARD) ListView.BOARD else ListView.LIST

internal val WorkflowStage.label: String get() = when (this) {
    WorkflowStage.BACKLOG -> "Backlog"
    WorkflowStage.TODO -> "To do"
    WorkflowStage.IN_PROGRESS -> "In progress"
    WorkflowStage.DONE -> "Done"
}

internal fun WorkflowStage.openStage(): OpenStage? = when (this) {
    WorkflowStage.BACKLOG -> OpenStage.BACKLOG
    WorkflowStage.TODO -> OpenStage.TODO
    WorkflowStage.IN_PROGRESS -> OpenStage.IN_PROGRESS
    WorkflowStage.DONE -> null
}

internal fun OpenStage.workflowStage(): WorkflowStage = when (this) {
    OpenStage.BACKLOG -> WorkflowStage.BACKLOG
    OpenStage.TODO -> WorkflowStage.TODO
    OpenStage.IN_PROGRESS -> WorkflowStage.IN_PROGRESS
}

internal fun TaskRow.visibleStage(): WorkflowStage? = workflowStage?.takeIf {
    chip == null || it == WorkflowStage.BACKLOG || it == WorkflowStage.IN_PROGRESS
}

/** Join shared grouping with the filtered, motion-placed rows. A completion held open by the
 * choreography still belongs in its origin column, not also in Done. */
internal fun BoardView.visibleCards(room: RoomView, reopening: Set<String> = emptySet()): Map<WorkflowStage, List<TaskRow>> {
    val open = room.tasks.groupBy { it.workflowStage }
    val recent = columns.firstOrNull { it.stage == WorkflowStage.DONE }?.taskIds.orEmpty().toHashSet()
    return columns.associate { column ->
        column.stage to if (column.stage == WorkflowStage.DONE) room.completed.filter { it.id in recent || it.id in reopening }
        else open[column.stage].orEmpty()
    }
}
