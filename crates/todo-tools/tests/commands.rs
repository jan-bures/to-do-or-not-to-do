#![allow(clippy::unwrap_used)]

use serde_json::{Map, Value, json};
use todo_core::command::Command;
use todo_core::ids::{ListId, TaskId};
use todo_tools::{assistant, descriptor};
use uuid::Uuid;

fn args(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn rest(value: Value) -> Command {
    serde_json::from_value(value).unwrap()
}

const LIST: ListId = ListId::from_uuid(Uuid::from_u128(1));
const TASK: TaskId = TaskId::from_uuid(Uuid::from_u128(2));

#[test]
fn create_task_arguments_decode_to_the_same_core_command_as_rest() {
    let c = descriptor::CREATE_TASK
        .decode(args(json!({
            "title":"Draft", "priority":"high", "start_date":"2030-06-01", "due":"2030-06-03",
            "status":"backlog", "recurrence":{"rule":{"freq":"weekly", "days":["mon","fri"]}}
        })))
        .unwrap()
        .into_command(TASK, LIST)
        .unwrap();
    assert_eq!(
        c,
        rest(json!({
            "cmd":"create_task", "id":TASK, "list":LIST, "title":"Draft", "priority":"high",
            "start_date":"2030-06-01", "due":"2030-06-03", "status":"backlog",
            "recurrence":{"rule":{"freq":"weekly", "interval":1, "days":["mon","fri"]}}
        }))
    );

    let c = assistant::CREATE_TASK
        .decode(args(json!({"title":"Draft", "due":"2030-06-03"})))
        .unwrap()
        .into_command(TASK, LIST)
        .unwrap();
    assert_eq!(
        c,
        rest(
            json!({"cmd":"create_task", "id":TASK, "list":LIST, "title":"Draft", "due":"2030-06-03"})
        )
    );
}

#[test]
fn update_preserves_omission_and_clear_in_both_tool_surfaces() {
    for (input, expected) in [
        (
            json!({"task_id":TASK,"title":"Changed"}),
            json!({"title":"Changed"}),
        ),
        (
            json!({"task_id":TASK,"priority":"none","due":null}),
            json!({"priority":"none","due":null}),
        ),
    ] {
        let expected = rest(json!({"cmd":"update_task","id":TASK,"patch":expected}));
        assert_eq!(
            assistant::UPDATE_TASK
                .decode(args(input.clone()))
                .unwrap()
                .into_command()
                .unwrap(),
            expected
        );
        assert_eq!(
            descriptor::UPDATE_TASK
                .decode(args(input))
                .unwrap()
                .into_command()
                .unwrap(),
            expected
        );
    }
    let c = descriptor::UPDATE_TASK
        .decode(args(json!({
            "task_id":TASK,"status":"in_progress","recurrence":null,"start_date":null
        })))
        .unwrap()
        .into_command()
        .unwrap();
    assert_eq!(
        c,
        rest(json!({
            "cmd":"update_task","id":TASK,"patch":{"status":"in_progress","recurrence":null,"start_date":null}
        }))
    );
}

#[test]
fn assistant_schema_and_parser_refuse_transport_metadata_and_unsupported_fields() {
    let create_schema = assistant::CREATE_TASK.input_schema();
    let mcp_schema = descriptor::CREATE_TASK.input_schema();
    assert_eq!(assistant::CREATE_TASK.name(), descriptor::CREATE_TASK.name);
    assert_eq!(
        create_schema["properties"]["priority"],
        mcp_schema["properties"]["priority"]
    );
    assert!(create_schema["$defs"].get("PriorityArg").is_some());
    assert_eq!(create_schema["$defs"].as_object().unwrap().len(), 1);
    assert_eq!(create_schema["required"], json!(["title"]));
    for field in [
        "command_id",
        "notes",
        "recurrence",
        "start_date",
        "status",
        "position",
    ] {
        let mut create = args(json!({"title":"Draft"}));
        let mut update = args(json!({"task_id":TASK,"title":"Changed"}));
        create.insert(field.into(), Value::Null);
        update.insert(field.into(), Value::Null);
        assert!(create_schema["properties"].get(field).is_none());
        assert!(
            assistant::UPDATE_TASK.input_schema()["properties"]
                .get(field)
                .is_none()
        );
        assert_eq!(
            assistant::CREATE_TASK.decode(create).unwrap_err(),
            assistant::ArgumentError::UnsupportedField
        );
        assert_eq!(
            assistant::UPDATE_TASK.decode(update).unwrap_err(),
            assistant::ArgumentError::UnsupportedField
        );
    }
    assert_eq!(
        assistant::CREATE_TASK.decode(Map::new()).unwrap_err(),
        assistant::ArgumentError::InvalidArguments
    );
    assert!(
        assistant::GET_TASK
            .decode(args(json!({"task_id":TASK,"include_deleted":false})))
            .is_err()
    );
    assert!(
        assistant::LIST_LISTS
            .decode(args(json!({"include_deleted":false})))
            .is_err()
    );
    assert!(
        assistant::COMPLETE_TASK
            .decode(args(json!({"task_id":TASK,"command_id":null})))
            .is_err()
    );

    for (input, expected) in [
        (
            json!({"task_id":TASK}),
            rest(json!({"cmd":"complete_task","id":TASK})),
        ),
        (
            json!({"task_id":TASK,"completed":false}),
            rest(json!({"cmd":"uncomplete_task","id":TASK})),
        ),
    ] {
        assert_eq!(
            assistant::COMPLETE_TASK
                .decode(args(input.clone()))
                .unwrap()
                .into_command(),
            expected
        );
        assert_eq!(
            descriptor::COMPLETE_TASK
                .decode(args(input))
                .unwrap()
                .into_command(),
            expected
        );
    }
}
