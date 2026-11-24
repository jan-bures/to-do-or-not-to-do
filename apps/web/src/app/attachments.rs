//! The task's attachment controls. Uploads belong to the task they started on, not to whichever
//! detail happens to be open when the transfer finishes.

use super::{Ctx, ctx, icon};
use crate::backend::{self, UploadError};
use crate::model::{AttachmentView, Detail};
use crate::platform;
use leptos::html;
use leptos::prelude::*;
use std::borrow::Cow;
use std::collections::BTreeMap;
use todo_client::intent;
use todo_core::ids::TaskId;
use todo_design::Icon;
use web_sys::{DataTransfer, DragEvent, File, FileList};

const ADD_FILE: &str = "add-attachment";

#[derive(Clone, PartialEq, Eq)]
enum UploadState {
    Waiting,
    Uploading,
    Notice(Cow<'static, str>),
}

#[derive(Clone, Copy)]
pub(super) struct Uploads(RwSignal<BTreeMap<TaskId, UploadState>>);

impl Uploads {
    pub(super) fn new(c: Ctx) -> Self {
        let uploads = Self(RwSignal::new(BTreeMap::new()));
        c.drafts.watch(move || {
            uploads.0.with_untracked(|states| {
                states
                    .values()
                    .any(|state| matches!(state, UploadState::Waiting | UploadState::Uploading))
            })
        });
        let open = Memo::new(move |_| c.detail.with(|d| d.as_ref().map(|d| d.id)));
        Effect::new(move |_| {
            let id = open.get();
            // Keep background transfers, but don't carry an old alert into a reopened detail.
            uploads.0.update(|states| {
                states.retain(|task, state| {
                    Some(*task) == id || !matches!(state, UploadState::Notice(_))
                })
            });
        });
        uploads
    }

    fn busy(self, task: TaskId) -> bool {
        self.0.with(|states| {
            matches!(
                states.get(&task),
                Some(UploadState::Waiting | UploadState::Uploading)
            )
        })
    }

    fn set(self, task: TaskId, state: UploadState) {
        self.0.update(|states| {
            states.insert(task, state);
        });
    }

    fn start(self, c: Ctx, task: TaskId, files: Vec<File>) {
        if c.read_only() || self.busy(task) || files.is_empty() {
            return;
        }
        let state = if backend::task_confirmed(task) {
            UploadState::Uploading
        } else {
            UploadState::Waiting
        };
        self.set(task, state);
        leptos::task::spawn_local(async move {
            let notice = match self.transfer(task, files).await {
                Ok(notice) => notice,
                Err(UploadError::Failed(message)) => Some(message),
                Err(UploadError::SignedOut) => None,
            };
            // Show the observed attachments in the same update that releases the Add file button.
            c.refresh();
            let open = c
                .detail
                .with_untracked(|d| d.as_ref().is_some_and(|d| d.id == task));
            if let Some(message) = notice.filter(|_| open) {
                self.set(task, UploadState::Notice(message));
            } else {
                self.0.update(|states| {
                    states.remove(&task);
                });
            }
        });
    }

    async fn transfer(
        self,
        task: TaskId,
        files: Vec<File>,
    ) -> Result<Option<Cow<'static, str>>, UploadError> {
        backend::wait_for_task(task).await?;
        self.set(task, UploadState::Uploading);
        let mut accepted = Vec::new();
        let mut notice = None;
        // A drop can contain several files. Send them in order without competing for upload slots.
        for file in files {
            match backend::upload_attachment(task, file).await {
                Ok(id) => accepted.push(id),
                Err(UploadError::Failed(message)) => {
                    notice.get_or_insert(message);
                }
                Err(error @ UploadError::SignedOut) => return Err(error),
            }
        }
        if !accepted.is_empty() {
            self.set(task, UploadState::Waiting);
            if !backend::wait_for_attachments(&accepted).await {
                let lag = "The uploaded files haven’t synced to this view yet.";
                notice = Some(
                    notice.map_or_else(|| lag.into(), |message| format!("{message} {lag}").into()),
                );
            }
        }
        Ok(notice)
    }

    pub(super) fn drag_over(self, c: Ctx, ev: DragEvent) {
        if let Some(data) = ev
            .data_transfer()
            .filter(|data| data.types().includes(&"Files".into(), 0))
        {
            // A file must not replace the app in this tab, even when it can't be uploaded.
            ev.prevent_default();
            let available = !c.read_only()
                && c.detail
                    .with_untracked(|d| d.as_ref().is_some_and(|d| !self.busy(d.id)));
            data.set_drop_effect(if available { "copy" } else { "none" });
        }
    }

    pub(super) fn drop_files(self, c: Ctx, ev: DragEvent) {
        if let Some(data) = ev
            .data_transfer()
            .filter(|data| data.types().includes(&"Files".into(), 0))
        {
            ev.prevent_default();
            if let Some(task) = c.detail.with_untracked(|d| d.as_ref().map(|d| d.id)) {
                self.start(c, task, dropped_files(&data));
            }
        }
    }
}

fn files_from_list(files: Option<FileList>) -> Vec<File> {
    files.map_or_else(Vec::new, |files| {
        (0..files.length()).filter_map(|i| files.get(i)).collect()
    })
}

fn dropped_files(data: &DataTransfer) -> Vec<File> {
    let items = data.items();
    let looks_like_file = |file: &File| file.size() > 0.0 || !file.type_().is_empty();
    if items.length() == 0 {
        return files_from_list(data.files())
            .into_iter()
            .filter(looks_like_file)
            .collect();
    }
    (0..items.length())
        .filter_map(|i| {
            let item = items.get(i).filter(|item| item.kind() == "file")?;
            // On browsers without the entry API, skip the empty, typeless placeholders for folders.
            let entry = item.webkit_get_as_entry().ok().flatten();
            let file = item.get_as_file().ok().flatten()?;
            entry
                .as_ref()
                .map_or_else(|| looks_like_file(&file), |entry| !entry.is_directory())
                .then_some(file)
        })
        .collect()
}

#[component]
pub(super) fn Section(
    task: TaskId,
    detail: Memo<Option<Detail>>,
    uploads: Uploads,
) -> impl IntoView {
    let c = ctx();
    let input = NodeRef::<html::Input>::new();
    let busy = move || uploads.busy(task);
    let files = Memo::new(move |_| {
        detail.with(|d| {
            d.as_ref()
                .map(|d| d.attachments.clone())
                .unwrap_or_default()
        })
    });
    view! {
        <section class="d-section" aria-labelledby="attachments-title">
            <h3 class="caps d-caps" id="attachments-title">"Attachments"</h3>
            <ul class="files" hidden=move || files.with(Vec::is_empty)>
                <For each=move || files.get() key=|a| a.id let:a>
                    <AttachmentRow file=a/>
                </For>
            </ul>
            <div class="file-controls">
                <input type="file" hidden multiple node_ref=input disabled=move || c.read_only() || busy()
                    on:change=move |_| {
                        if let Some(input) = input.get_untracked() {
                            uploads.start(c, task, files_from_list(input.files()));
                            input.set_value("");
                        }
                    }/>
                <button type="button" class="file-action file-add" id=ADD_FILE
                    disabled=move || c.read_only() aria-disabled=move || busy().then_some("true")
                    on:click=move |_| {
                        if !c.read_only() && !busy() && let Some(input) = input.get_untracked() {
                            input.click();
                        }
                    }>
                    {icon(Icon::Plus)}"Add file"
                </button>
                <span class="file-hint" role="status">
                    {move || uploads.0.with(|states| match states.get(&task) {
                        Some(UploadState::Waiting) => "Waiting to sync…",
                        Some(UploadState::Uploading) => "Uploading…",
                        _ if !c.read_only() => "or drop files on this task",
                        _ => "",
                    })}
                </span>
            </div>
            {move || uploads.0.with(|states| match states.get(&task) {
                Some(UploadState::Notice(message)) => Some(view! { <p class="prop-error" role="alert">{message.clone()}</p> }),
                _ => None,
            })}
        </section>
    }
}

#[component]
fn AttachmentRow(file: AttachmentView) -> impl IntoView {
    let c = ctx();
    let AttachmentView { id, name, size } = file;
    let label = name.clone();
    view! {
        <li class="file-row">
            <button class="file" type="button" aria-label=format!("Download “{label}”") title=label.clone()
                on:click=move |_| leptos::task::spawn_local(platform::download(id, name.clone()))>
                {icon(Icon::Paperclip)}
                <span class="file-name">{label.clone()}</span>
                <span class="file-size">{size}</span>
                {icon(Icon::Download)}
            </button>
            <button class="file-action" type="button" aria-label=format!("Remove “{label}”")
                disabled=move || c.read_only() on:click=move |_| {
                    if c.submit_undoable(intent::delete(id), format!("Removed attachment “{label}”"), None) {
                        request_animation_frame(|| platform::refocus_row(ADD_FILE));
                    }
                }>"Remove"</button>
        </li>
    }
}
