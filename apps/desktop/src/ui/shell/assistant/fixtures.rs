//! Network-free presentation fixtures for the hidden screenshot runner.

use super::*;
use todo_client::assistant::{
    Edit, Effect, ListAccess, ListRef, ModelSelection, ProposalItem, ProposalPreview, Step,
    StepKind, StepStatus, StepSummary, TaskRef, ToolKind,
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
        self.ai.open = name != "ai-pending";
        self.settle_detail();
        if name == "ai-unconfigured" {
            self.ai.cloud.status = Some(todo_client::assistant::relay::Status {
                chatgpt: true,
                private_models: true,
                ..Default::default()
            });
            cx.notify();
            return;
        }
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
            "ai-markdown" => {
                let titles = [
                    "Renew passport appointment",
                    "Pack the weekend bag",
                    "Return the library books",
                    "Check the tire pressure",
                    "Back up the photos",
                    "File the receipts",
                    "Clean out the inbox",
                ];
                let mut turn = draft(
                    5,
                    "Help me plan my weekend.",
                    "## Weekend plan\n\nStart with **Renew passport appointment**, then *Pack the weekend bag*.\n\n\
                     - Return the library books\n- Check the tire pressure\n\n\
                     Later, Back up the photos, File the receipts, and Clean out the inbox.",
                    None,
                    None,
                );
                turn.task_links = titles
                    .into_iter()
                    .filter_map(|title| {
                        let task = state.tasks.values().find(|task| task.title == title)?;
                        let list = state.lists.get(&task.list)?;
                        Some(TaskRef {
                            id: task.id,
                            title: task.title.clone(),
                            due: task.due,
                            list: ListRef {
                                id: list.id,
                                name: list.name.clone(),
                            },
                        })
                    })
                    .collect();
                turn.summary.tasks_read = turn.task_links.len();
                (vec![turn], Composer::Ready)
            }
            "ai-proposal" | "ai-pending" => {
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
            "http://127.0.0.1:11434/v1"
        };
        self.ai.profile.url = address.into();
        self.ai.profile.model = "local-model".into();
        let capabilities = todo_client::assistant::local::LocalProvider::new(address, "")
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
        if name == "ai-typing" {
            self.ai.composer.update(cx, |input, cx| {
                input.set_text("What can I finish today?", cx)
            });
        }
        self.ai.connection = match name {
            "ai-error" | "ai-settings-unreachable" => ConnectionHealth::Unreachable,
            "ai-blocked" | "ai-chatgpt-signed-out" => ConnectionHealth::Unknown,
            _ => ConnectionHealth::Connected,
        };
        if matches!(
            name,
            "ai-settings"
                | "ai-settings-testing"
                | "ai-settings-unreachable"
                | "ai-chatgpt-signed-out"
                | "ai-chatgpt-device-code"
                | "ai-chatgpt-signed-in"
                | "ai-chatgpt-model-picker"
                | "ai-chatgpt-account"
                | "ai-chatgpt-consent"
                | "ai-private-relay"
        ) {
            self.ai.profile.key = Zeroizing::new("fixture-key".into());
            if let Some(collection) = state.lists[&errands.id].collection {
                self.ai
                    .profile
                    .access
                    .set_collection(&state, collection, ListAccess::PrivateOnly);
            }
            self.ai
                .profile
                .access
                .lists
                .insert(errands.id, ListAccess::Hidden);
            if let Some(collection) = state.lists[&reading.id].collection {
                self.ai
                    .profile
                    .access
                    .set_collection(&state, collection, ListAccess::AllModels);
            }
            if name.starts_with("ai-chatgpt-") {
                self.ai.profile.cloud = true;
                self.ai.cloud.fixture_enabled = true;
                let signed_in = !matches!(name, "ai-chatgpt-signed-out" | "ai-chatgpt-device-code");
                if name == "ai-chatgpt-device-code" {
                    self.ai.cloud.code = Some("EXAMPLE-1".into());
                }
                if signed_in {
                    self.ai.profile.model = "fixture-model".into();
                    self.ai.profile.chatgpt_account = Some(Uuid::from_u128(1));
                    self.ai.cloud.fixture = Some(chatgpt::AccountView {
                        id: Uuid::from_u128(1),
                        label: "you@example.com".into(),
                        consented_lists: None,
                    });
                }
                let mut accounts: Vec<_> = self
                    .ai
                    .cloud
                    .fixture
                    .iter()
                    .map(|account| todo_client::assistant::relay::Account {
                        id: account.id,
                        label: account.label.clone(),
                        approved: false,
                        approved_lists: BTreeSet::new(),
                    })
                    .collect();
                if name == "ai-chatgpt-signed-in" {
                    let added = todo_client::assistant::relay::Account {
                        id: Uuid::from_u128(2),
                        label: "second@example.com".into(),
                        approved: false,
                        approved_lists: BTreeSet::new(),
                    };
                    self.ai.cloud.signed_in = Some((added.id, added.label.clone()));
                    accounts.push(added);
                }
                self.ai.cloud.status = Some(todo_client::assistant::relay::Status {
                    chatgpt: true,
                    chatgpt_device_code: name != "ai-chatgpt-account",
                    private_models: true,
                    accounts,
                    access: self.ai.profile.access.clone(),
                    ..Default::default()
                });
                if let Some(view) = &mut self.ai.fixture {
                    if !signed_in {
                        view.composer = Composer::ChooseModel;
                        view.model = None;
                    } else if let Some(model) = &mut view.model {
                        model.model = "gpt-sample".into();
                        model.capabilities.data_class =
                            todo_client::assistant::provider::DataClass::Cloud;
                        model.capabilities.billing =
                            todo_client::assistant::provider::BillingClass::Subscription;
                    }
                }
                if name == "ai-chatgpt-consent" {
                    let access = &mut self.ai.profile.access;
                    let mut lists: BTreeSet<_> = state
                        .lists
                        .keys()
                        .copied()
                        .filter(|&id| access.list(&state, id) == ListAccess::AllModels)
                        .collect();
                    if let Some(account) = &mut self.ai.cloud.fixture {
                        account.consented_lists = Some(lists.clone());
                        access.lists.insert(errands.id, ListAccess::AllModels);
                        lists.insert(errands.id);
                        self.ai.cloud.consent = Some(super::cloud::Consent::new(
                            account.clone(),
                            "Summarize this week".into(),
                            (Some(state), lists),
                        ));
                    }
                } else {
                    self.open_ai_settings(window, cx);
                    if name == "ai-chatgpt-model-picker"
                        && let Some(setup) = &mut self.ai.settings
                    {
                        setup.pick_model = true;
                        setup.models = [
                            ("fixture-model", "GPT Sample"),
                            ("fixture-mini", "GPT Sample Mini"),
                            ("fixture-nano", "GPT Sample Nano"),
                            ("fixture-fast", "GPT Sample Fast"),
                            ("fixture-reasoning", "GPT Sample Reasoning"),
                            ("fixture-coding", "GPT Sample Coding"),
                            ("fixture-long", "GPT Sample Long Context"),
                            ("fixture-preview", "GPT Sample Preview"),
                        ]
                        .into_iter()
                        .map(|(id, label)| Model {
                            id: id.into(),
                            label: label.into(),
                        })
                        .collect();
                    }
                }
                cx.notify();
                return;
            }
            if name == "ai-private-relay" {
                self.ai.profile.private_relay = true;
                self.ai.profile.model = "fixture-model".into();
                self.ai.cloud.status = Some(todo_client::assistant::relay::Status {
                    private_models: true,
                    access: self.ai.profile.access.clone(),
                    ..Default::default()
                });
                self.open_ai_settings(window, cx);
                return;
            }
            self.open_ai_settings(window, cx);
            if let Some(setup) = self.ai.settings.as_mut() {
                setup.check = match name {
                    "ai-settings-testing" => Check::Running("Checking connection…"),
                    "ai-settings-unreachable" => {
                        Check::Unreachable(problem(ProviderError::Unavailable))
                    }
                    _ => Check::Catalog {
                        count: 4,
                        elapsed: Duration::from_millis(38),
                    },
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
