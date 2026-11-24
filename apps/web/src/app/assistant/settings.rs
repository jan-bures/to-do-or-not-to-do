//! Browser-local configuration and the shared collection/individual visibility rules.

use super::{BrowserProvider, CHAT, class_label, ctx, html, icon, platform};
use crate::app::glyph;
use crate::model;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use todo_client::assistant::{AccessRules, ListAccess, Selection, provider::DataClass};
use todo_core::{
    State,
    ids::{CollectionId, ListId},
};
use todo_design::{Glyph, Icon};

pub(super) const KEY: &str = "todo.assistant.settings";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Profile {
    address: String,
    model: String,
    consent: bool,
    access: AccessRules,
}

impl Profile {
    pub(super) fn load() -> Self {
        platform::local_storage()
            .and_then(|storage| storage.get_item(KEY).ok().flatten())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> bool {
        serde_json::to_string(self).ok().is_some_and(|text| {
            platform::local_storage().is_some_and(|storage| storage.set_item(KEY, &text).is_ok())
        })
    }

    fn provider(&self) -> Option<BrowserProvider> {
        BrowserProvider::new(&self.address)
    }

    fn valid_model(&self) -> bool {
        let model = self.model.trim();
        !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
    }

    pub(super) fn apply(&self, old: Option<&Self>) {
        if old == Some(self) {
            return;
        }
        let expected = self.clone();
        let selection = self
            .provider()
            .filter(|provider| {
                self.valid_model() && (provider.class == DataClass::Local || self.consent)
            })
            .map(|provider| Selection {
                provider: Arc::new(provider.bind(move || Profile::load() == expected)),
                model: self.model.clone(),
                allow_billed: self.consent,
                billed_list_scope: None,
                access: self.access.clone(),
            });
        CHAT.with(|chat| chat.configure(selection));
    }
}

impl super::Ui {
    pub(in crate::app) fn reload_profile(self) -> bool {
        let loaded = Profile::load();
        let old = self.profile.get_value();
        if loaded == old {
            return false;
        }
        self.use_profile(loaded, &old);
        true
    }

    fn use_profile(self, profile: Profile, old: &Profile) {
        profile.apply(Some(old));
        if &profile != old {
            self.error.set(None);
            self.draft.set(String::new());
            self.approvals.set(Default::default());
            self.profile.set_value(profile);
            self.refresh();
        }
    }
}

#[component]
pub(in super::super) fn Settings() -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    let dialog = NodeRef::<html::Dialog>::new();
    let body = NodeRef::<html::Div>::new();
    let draft = RwSignal::new(Profile::default());
    let baseline = StoredValue::new(Profile::default());
    let return_focus = StoredValue::new("assistant-trigger");
    let error = RwSignal::new(None::<&'static str>);
    let live_cloud = Memo::new(move |_| {
        draft.with(|p| p.provider().is_some_and(|p| p.class == DataClass::Cloud))
    });
    Effect::new(move |_| {
        if let (Some(from), Some(el)) = (ai.settings.get(), dialog.get())
            && !el.open()
        {
            return_focus.set_value(from);
            ai.reload_profile();
            let profile = ai.profile.get_value();
            draft.set(profile.clone());
            baseline.set_value(profile);
            error.set(None);
            if el.show_modal().is_err() {
                ai.settings.set(None);
            } else if let Some(body) = body.get_untracked() {
                body.set_scroll_top(0);
            }
        }
    });
    let save = move |disconnect: bool| {
        let mut next = draft.get_untracked();
        next.address = next.address.trim().into();
        next.model = next.model.trim().into();
        if disconnect {
            next.address.clear();
            next.model.clear();
            next.consent = false;
        } else if !next.address.is_empty() || !next.model.is_empty() {
            let Some(provider) = next.provider() else {
                error.set(Some("Enter an HTTPS model server URL, or HTTP at 127.0.0.1. It must not be this app’s origin or contain credentials, a query, or a fragment."));
                return;
            };
            if !next.valid_model() {
                error.set(Some("Enter a valid model name."));
                return;
            }
            if provider.class == DataClass::Cloud && !next.consent {
                error.set(Some(
                    "Confirm what can be sent to this destination before saving.",
                ));
                return;
            }
        }
        let old = Profile::load();
        if old != baseline.get_value() {
            ai.reload_profile();
            error.set(Some("Settings changed in another tab. Close this dialog and reopen it to review the new settings."));
        } else if next.save() {
            ai.use_profile(next, &old);
            if let Some(el) = dialog.get_untracked() {
                el.close();
            }
        } else {
            error.set(Some("This browser couldn’t save the assistant settings."));
        }
    };
    view! {
        <dialog id="assistant-settings-dialog" class="room-dialog ai-settings" aria-labelledby="ai-settings-title" node_ref=dialog
            on:keydown=move |ev| platform::contain_tab("assistant-settings-dialog", &ev)
            on:close=move |_| {
                ai.settings.set(None);
                let focus = return_focus.get_value();
                if !platform::focus(focus) && ai.open.get_untracked() { ai.focus_composer(); }
            }>
            <header class="ai-settings-header">
                <div><h2 id="ai-settings-title">"AI assistant"</h2><p>"Choose a model and which lists it can see."</p></div>
                <button type="button" class="icon-button" aria-label="Close AI settings" on:click=move |_| {
                    if let Some(el) = dialog.get_untracked() { el.close(); }
                }>{icon(Icon::X)}</button>
            </header>
            <form on:submit=move |ev| { ev.prevent_default(); save(false); }>
                <div class="ai-settings-body" node_ref=body>
                    <section class="ai-provider" aria-label="Your own server">
                        <div class="ai-provider-heading">
                            {icon(Icon::Settings)}<h3>"Your own server"</h3>
                            <span>{move || draft.with(|p| p.provider().map(|p| class_label(p.class)))}</span>
                        </div>
                        <p>"The browser connects directly to an OpenAI-compatible server. It must allow this site in CORS."</p>
                        <Show when=move || draft.with(|p| p.provider().is_some_and(|p| p.class == DataClass::Local))>
                            <p>"Use a locally running model, not a server that forwards requests to another provider."</p>
                        </Show>
                        <label for="ai-base-url">"Base URL"</label>
                        <input id="ai-base-url" type="url" spellcheck="false" autocomplete="off" autofocus
                            placeholder="https://todo.example.com/v1" prop:value=move || draft.with(|p| p.address.clone())
                            on:input=move |ev| draft.update(|p| { p.address = event_target_value(&ev); p.consent = false; })/>
                        <label for="ai-model">"Model"</label>
                        <input id="ai-model" type="text" maxlength="256" autocomplete="off" spellcheck="false"
                            placeholder="Model name" prop:value=move || draft.with(|p| p.model.clone())
                            on:input=move |ev| draft.update(|p| { p.model = event_target_value(&ev); p.consent = false; })/>
                        <Show when=move || live_cloud.get()>
                            <label class="ai-consent">
                                <input type="checkbox" prop:checked=move || draft.with(|p| p.consent)
                                    on:change=move |ev| draft.update(|p| p.consent = event_target_checked(&ev))/>
                                <span>"Send prompts and data from All models lists to this server. Provider charges may apply."</span>
                            </label>
                        </Show>
                    </section>
                    <section class="ai-access" aria-labelledby="ai-access-title">
                        <h3 id="ai-access-title">"Lists the assistant can see"</h3>
                        <p>"Hidden lists are never sent. Private only lists go only to a model at 127.0.0.1. Lists inherit their collection’s rule unless overridden."</p>
                        {move || c.state.with(|state| {
                            let mut rows = Vec::new();
                            for group in model::collections(state) {
                                rows.push(view! { <AccessRow target=Target::Collection(group.id) name=group.name.clone() draft/> });
                                for list in model::lists_in(state, Some(group.id)) {
                                    rows.push(view! { <AccessRow target=Target::List(list.id) name=list.name.clone() draft nested=true/> });
                                }
                            }
                            for list in model::lists_in(state, None) {
                                rows.push(view! { <AccessRow target=Target::List(list.id) name=list.name.clone() draft/> });
                            }
                            rows
                        })}
                    </section>
                </div>
                <footer class="ai-settings-footer">
                    <Show when=move || error.with(Option::is_some)><p role="alert" class="action-error">{move || error.get()}</p></Show>
                    <div>
                        <Show when=move || ai.view.with(|v| v.model.is_some())>
                            <button type="button" class="ai-link" on:click=move |_| save(true)>"Disconnect model"</button>
                        </Show>
                        <button type="button" class="button" on:click=move |_| {
                            if let Some(el) = dialog.get_untracked() { el.close(); }
                        }>"Cancel"</button>
                        <button type="submit" class="button primary">"Save"</button>
                    </div>
                </footer>
            </form>
        </dialog>
    }
}

#[derive(Clone, Copy)]
enum Target {
    List(ListId),
    Collection(CollectionId),
}

fn key(access: Option<ListAccess>) -> &'static str {
    match access {
        None => "",
        Some(ListAccess::Hidden) => "hidden",
        Some(ListAccess::PrivateOnly) => "private",
        Some(ListAccess::AllModels) => "all",
    }
}

impl Target {
    fn value(self, state: &State, rules: &AccessRules) -> &'static str {
        match self {
            Self::Collection(id) => key(rules.collection(state, id)),
            Self::List(id) => key(Some(rules.list(state, id))),
        }
    }
}

#[component]
fn AccessRow(
    target: Target,
    name: String,
    draft: RwSignal<Profile>,
    #[prop(optional)] nested: bool,
) -> impl IntoView {
    let c = ctx();
    let value = Memo::new(move |_| c.state.with(|s| draft.with(|p| target.value(s, &p.access))));
    view! {
        <label class="ai-access-row" class:nested=nested>
            <span title=name.clone()>{glyph(match target { Target::List(_) => Glyph::Document, Target::Collection(_) => Glyph::Layers })}{name.clone()}</span>
            <select prop:value=move || value.get() on:change=move |ev| {
                let access = match event_target_value(&ev).as_str() {
                    "hidden" => ListAccess::Hidden,
                    "private" => ListAccess::PrivateOnly,
                    "all" => ListAccess::AllModels,
                    _ => return,
                };
                draft.update(|profile| match target {
                    Target::List(id) => { profile.access.lists.insert(id, access); }
                    Target::Collection(id) => c.state.with_untracked(|state| profile.access.set_collection(state, id, access)),
                });
            }>
                <option value="" disabled selected=move || value.get().is_empty()>"Mixed"</option>
                <option value="hidden" selected=move || value.get() == "hidden">"Hidden"</option>
                <option value="private" selected=move || value.get() == "private">"Private only"</option>
                <option value="all" selected=move || value.get() == "all">"All models"</option>
            </select>
        </label>
    }
}
