//! The task's attachment controls. Uploads belong to the task they started on, not to whichever
//! detail happens to be open when the transfer finishes.

use super::{Ctx, ctx, icon};
use crate::backend::{self, UploadError};
use crate::model::{AttachmentView, Detail};
use crate::platform;
use leptos::html;
use leptos::prelude::*;
use std::collections::BTreeMap;
use todo_client::{failure, intent};
use todo_core::ids::TaskId;
use todo_design::Icon;
use web_sys::{DragEvent, FileList};

const ADD_FILE: &str = "add-attachment";

#[derive(Clone, Copy, PartialEq, Eq)]
enum UploadState {
    Pending,
    Failed,
}

#[derive(Clone, Copy)]
pub(super) struct Uploads(RwSignal<BTreeMap<TaskId, UploadState>>);

impl Uploads {
    pub(super) fn new() -> Self {
        Self(RwSignal::new(BTreeMap::new()))
    }

    fn busy(self, task: TaskId) -> bool {
        self.0
            .with(|states| states.get(&task) == Some(&UploadState::Pending))
    }

    fn start(self, c: Ctx, task: TaskId, files: Option<FileList>) {
        let Some(files) = files else { return };
        if c.read_only() || self.busy(task) || files.length() == 0 {
            return;
        }
        // Take the Files before the input is cleared or the drop event's data store closes.
        let files: Vec<_> = (0..files.length()).filter_map(|i| files.get(i)).collect();
        self.0.update(|states| {
            states.insert(task, UploadState::Pending);
        });
        leptos::task::spawn_local(async move {
            let mut failed = false;
            // A drop can contain several files. Send them in order without competing for upload slots.
            for file in files {
                match backend::upload_attachment(task, file).await {
                    Ok(()) => {}
                    Err(UploadError::Failed) => failed = true,
                    Err(UploadError::SignedOut) => {
                        platform::sign_in(&platform::path());
                        failed = true;
                        break;
                    }
                }
            }
            if !c.live {
                c.refresh();
            }
            self.0.update(|states| {
                if failed {
                    states.insert(task, UploadState::Failed);
                } else {
                    states.remove(&task);
                }
            });
        });
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
        if let Some(files) = ev
            .data_transfer()
            .and_then(|data| data.files())
            .filter(|f| f.length() > 0)
        {
            ev.prevent_default();
            if let Some(task) = c.detail.with_untracked(|d| d.as_ref().map(|d| d.id)) {
                self.start(c, task, Some(files));
            }
        }
    }
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
                            uploads.start(c, task, input.files());
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
                    {move || if busy() { "Uploading…" } else if !c.read_only() { "or drop files on this task" } else { "" }}
                </span>
            </div>
            <Show when=move || uploads.0.with(|states| states.get(&task) == Some(&UploadState::Failed))>
                <p class="prop-error" role="alert">{failure::UPLOAD_ATTACHMENT_FAILED}</p>
            </Show>
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
