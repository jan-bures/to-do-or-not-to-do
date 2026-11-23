//! Network-free presentation fixtures for the hidden screenshot runner.

use super::*;
use todo_client::assistant::{
    Edit, Effect, ListRef, ModelSelection, ProposalItem, ProposalPreview, Step, StepKind,
    StepStatus, StepSummary, TaskRef, ToolKind,
};
use todo_core::{command::Placement, event::Change, model::WorkflowStatus, time::Date};

impl ShellView {
    pub(crate) fn preview_assistant(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !name.starts_with("ai-") {
            return;
        }
        self.ai.open = true;
        self.settle_detail();
        let state = self.store.view();
        let target = state
            .tasks
            .values()
            .find(|task| task.title == "Pay quarterly VAT filing");
        let sample_list = |name: &str| {
            state
                .lists
                .values()
                .find(|list| list.name == name)
                .map(|list| ListRef {
                    id: list.id,
                    name: list.name.clone(),
                })
        };
        let (Some(target), Some(finance), Some(errands), Some(reading), Some(home)) = (
            target,
            sample_list("Finance"),
            sample_list("Errands"),
            sample_list("Reading list"),
            sample_list("Home renovation"),
        ) else {
            return;
        };
        let day = self.store.now().today();
        let monday = "2026-11-23".parse::<Date>().unwrap_or(day);
        let edit = TaskRef {
            id: target.id,
            title: target.title.clone(),
            list: finance,
            due: target.due,
        };
        let make = |index, title: &str, list: &ListRef, due, priority| ProposalItem {
            index,
            effects: vec![Effect::Created {
                task: TaskRef {
                    id: TaskId::from_uuid(Uuid::from_u128(20_000 + index as u128)),
                    title: title.into(),
                    list: list.clone(),
                    due,
                },
                priority,
                status: WorkflowStatus::Todo,
                placement: Placement::First,
            }],
        };
        let done = |kind, count, ms| Step {
            kind: StepKind::Tool(kind),
            status: StepStatus::Done {
                count,
                elapsed: Duration::from_millis(ms),
            },
        };
        let card =
            |id, status, items: Vec<ProposalItem>, available: Vec<ProposalItem>| ProposalCard {
                status,
                preview: ProposalPreview {
                    id: Uuid::from_u128(id),
                    digest: [0; 32],
                    items,
                },
                available,
            };
        let draft = |id, prompt: &str, prose: &str, proposal, error| TurnView {
            id: Uuid::from_u128(id),
            prompt: prompt.into(),
            prose: prose.into(),
            steps: vec![done(ToolKind::Lists, 4, 16), done(ToolKind::Search, 2, 31)],
            summary: StepSummary {
                lists_checked: 4,
                tasks_read: 2,
                private_lists: 1,
            },
            task_links: Vec::new(),
            proposal,
            error,
            usage: None,
            text_only: false,
            stale_snapshot: false,
        };
        let (turns, composer) = match name {
            "ai-proposal" => {
                let item = ProposalItem {
                    index: 0,
                    effects: vec![Effect::Edited {
                        task: edit,
                        changes: vec![Edit::Due(Change {
                            old: target.due,
                            new: Some(monday),
                        })],
                    }],
                };
                (
                    vec![
                        draft(
                            1,
                            "Add oat milk to my errands.",
                            "",
                            Some(card(
                                101,
                                ProposalStatus::Submitted,
                                vec![make(0, "Pick up oat milk", &errands, None, Priority::None)],
                                vec![],
                            )),
                            None,
                        ),
                        draft(
                            2,
                            "Move the quarterly filing to Monday instead.",
                            "I found the quarterly filing in Finance. Here’s the change I’d make. It won’t take effect until you approve it.",
                            Some(card(
                                102,
                                ProposalStatus::Pending,
                                vec![item.clone()],
                                vec![item],
                            )),
                            None,
                        ),
                    ],
                    Composer::Review {
                        id: Uuid::from_u128(102),
                        selected: 1,
                        total: 1,
                    },
                )
            }
            "ai-batch" => {
                let items = vec![
                    make(
                        0,
                        "Pick up oat milk and lemons",
                        &errands,
                        Some(day),
                        Priority::None,
                    ),
                    make(
                        1,
                        "Pick up dry cleaning",
                        &errands,
                        Some(day),
                        Priority::Normal,
                    ),
                    make(
                        2,
                        "Return the library books",
                        &reading,
                        Some(monday),
                        Priority::Low,
                    ),
                    make(3, "Buy potting soil", &home, Some(monday), Priority::None),
                    make(4, "Organize the hall closet", &home, None, Priority::None),
                ];
                let turn = draft(
                    3,
                    "Add five tasks for the weekend: groceries, dry cleaning, library books, potting soil, and the hall closet.",
                    "",
                    Some(card(
                        103,
                        ProposalStatus::Pending,
                        items[..4].to_vec(),
                        items,
                    )),
                    None,
                );
                self.ai.step_details.insert(turn.id);
                self.ai.review.begin(turn.id);
                self.ai.review.expanded.insert(0);
                (
                    vec![turn],
                    Composer::Review {
                        id: Uuid::from_u128(103),
                        selected: 4,
                        total: 5,
                    },
                )
            }
            "ai-error" => (
                vec![draft(
                    4,
                    "What’s due this week?",
                    "",
                    None,
                    Some(AssistantError::Provider(ProviderError::Unavailable)),
                )],
                Composer::Ready,
            ),
            _ => (Vec::new(), Composer::Ready),
        };
        let address = if name == "ai-blocked" {
            "https://mac.tailnet.example/v1"
        } else {
            "http://100.64.0.1:11434/v1"
        };
        self.ai.profile.url = address.into();
        self.ai.profile.model = "local-model".into();
        self.ai.profile.enabled = true;
        let capabilities = LocalProvider::new(address, "")
            .map(|p| {
                p.with_support("local-model".into(), Support::StreamingTools)
                    .capabilities("local-model")
            })
            .ok();
        let model = capabilities.map(|capabilities| ModelSelection {
            model: "local-model".into(),
            capabilities,
        });
        self.ai.fixture = Some(AssistantView {
            generation: 0,
            composer,
            model,
            turns,
        });
        if name == "ai-settings" {
            self.ai.profile.key = Zeroizing::new("fixture-key".into());
            self.ai.profile.lists.insert(errands.id, Access::Private);
            self.ai.profile.lists.insert(reading.id, Access::All);
            self.open_ai_settings(window, cx);
            if let Some(setup) = self.ai.settings.as_mut() {
                setup.check = Check::Catalog {
                    count: 4,
                    elapsed: Duration::from_millis(38),
                };
                setup.models = ["local-model", "another-local-model"]
                    .into_iter()
                    .map(|name| Model {
                        id: name.into(),
                        label: name.into(),
                    })
                    .collect();
            }
        }
        cx.notify();
    }
}
