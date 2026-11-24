//! Browser-local configuration and the shared collection/individual visibility rules.

mod store;

use super::{BrowserProvider, CHAT, ctx, hosted, html, icon, platform, relay};
use crate::app::glyph;
use crate::model;
use leptos::prelude::*;
use std::sync::Arc;
use todo_client::assistant::{AccessRules, ListAccess, Selection, endpoint::bearer_key};
use todo_core::{
    State,
    ids::{CollectionId, ListId},
};
use todo_design::{Glyph, Icon};

use store::login_fingerprint;
pub(super) use store::{Credential, Profile, Scope};

fn valid_model(model: &str) -> bool {
    !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control)
}

pub(super) fn load(scope: Option<&Scope>) -> (Profile, Credential) {
    let Some(scope) = scope.filter(|scope| scope.active()) else {
        return Default::default();
    };
    let profile = Profile::load(scope);
    let epoch = scope.epoch();
    let mut key = scope.remembered(&profile.address, epoch);
    // Keep the revision even when keyless, so revocation still resets/aborts the controller.
    key.address.clone_from(&profile.address);
    key.epoch = epoch;
    key.login = login_fingerprint();
    (profile, key)
}

pub(super) fn configure(
    scope: Option<Scope>,
    profile: &Profile,
    credential: &Credential,
    hosted_status: Option<&relay::Status>,
) -> Option<&'static str> {
    let mut route = None;
    let selection = scope.filter(Scope::active).and_then(|scope| {
        let expected = profile.clone();
        if let Some(hosted) = profile.hosted {
            let status = hosted_status?;
            let account = match hosted {
                super::Hosted::Private if status.private_models => None,
                super::Hosted::ChatGpt(id) => Some(status.accounts.iter().find(|a| a.id == id)?),
                _ => return None,
            };
            if !valid_model(&profile.hosted_model) {
                return None;
            }
            route = Some(hosted.label());
            return Some(Selection {
                provider: Arc::new(
                    BrowserProvider::hosted(hosted).bind(String::new(), move || {
                        scope.active() && Profile::load(&scope) == expected
                    }),
                ),
                model: profile.hosted_model.clone(),
                allow_billed: account.is_some_and(|a| a.approved),
                billed_list_scope: account.map(|a| a.approved_lists.clone()),
                access: status.access.clone(),
            });
        }
        if !valid_model(&profile.model) {
            return None;
        }
        let provider = BrowserProvider::new(&profile.address).ok()?;
        route = Some("This device");
        let epoch = scope.epoch();
        let key = if credential.matches(&profile.address, epoch) {
            credential.value.clone()
        } else {
            String::new()
        };
        let login = login_fingerprint();
        Some(Selection {
            provider: Arc::new(provider.bind(key, move || {
                scope.active()
                    && scope.epoch() == epoch
                    && login_fingerprint() == login
                    && Profile::load(&scope) == expected
            })),
            model: profile.model.clone(),
            allow_billed: false,
            billed_list_scope: None,
            access: profile.access.clone(),
        })
    });
    CHAT.with(|chat| chat.configure(selection));
    route
}

impl super::Ui {
    pub(in crate::app) fn storage_changed(self, key: Option<&str>) {
        if self
            .scope
            .with_value(|scope| scope.as_ref().is_some_and(|s| s.relevant(key)))
        {
            self.reload_profile();
        }
    }

    pub(super) fn reload_profile(self) -> bool {
        if self.signed_out.get_value() {
            return true;
        }
        let scope = self.scope.get_value();
        if scope.as_ref().is_some_and(|s| !s.active()) {
            self.sign_out();
            return true;
        }
        let (loaded, remembered) = load(scope.as_ref());
        let old = self.profile.get_value();
        let key = self.credential.get_value();
        let key = if scope
            .as_ref()
            .is_some_and(|scope| key.matches(&loaded.address, scope.epoch()))
        {
            key
        } else {
            remembered
        };
        if loaded == old && key == self.credential.get_value() {
            return false;
        }
        self.use_profile(loaded, key);
        true
    }

    fn use_profile(self, profile: Profile, credential: Credential) {
        self.route.set(self.relay_status.with_untracked(|status| {
            configure(
                self.scope.get_value(),
                &profile,
                &credential,
                status.as_ref(),
            )
        }));
        self.profile.set_value(profile);
        self.credential.set_value(credential);
        self.error.set(None);
        self.draft.set(String::new());
        self.consent.set(None);
        self.approvals.set(Default::default());
        self.refresh();
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum ProviderTab {
    #[default]
    Device,
    Private,
    ChatGpt,
}

impl ProviderTab {
    pub(super) const ALL: [Self; 3] = [Self::Device, Self::Private, Self::ChatGpt];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Device => "This device",
            Self::Private => "Your private models",
            Self::ChatGpt => "ChatGPT",
        }
    }

    pub(super) fn opener(self) -> &'static str {
        match self {
            Self::Device => "assistant-connect",
            Self::Private => "assistant-connect-private",
            Self::ChatGpt => "assistant-connect-chatgpt",
        }
    }

    pub(super) fn available(self, status: Option<&relay::Status>) -> bool {
        match self {
            Self::Device => true,
            Self::Private => status.is_some_and(|s| s.private_models),
            Self::ChatGpt => status.is_some_and(|s| s.chatgpt),
        }
    }

    fn from_profile(profile: &Profile) -> Self {
        match profile.hosted {
            None => Self::Device,
            Some(super::Hosted::Private) => Self::Private,
            Some(super::Hosted::ChatGpt(_)) => Self::ChatGpt,
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
    let baseline_epoch = StoredValue::new(None);
    let baseline_login = StoredValue::new(None);
    let return_focus = StoredValue::new("assistant-trigger");
    let error = RwSignal::new(None::<String>);
    let key_input = NodeRef::<html::Input>::new();
    let has_key = RwSignal::new(false);
    let remember_key = RwSignal::new(false);
    let local_access = StoredValue::new(AccessRules::default());
    let reviewed_status = RwSignal::new(None::<relay::Status>);
    let saving = RwSignal::new(false);
    let tab = RwSignal::new(ProviderTab::default());
    let signed_in = RwSignal::new(None::<String>);
    let recent_account = StoredValue::new(None);
    let select_tab = move |provider| {
        signed_in.set(None);
        tab.set(provider);
        if provider != ProviderTab::ChatGpt {
            draft.with_untracked(|p| {
                if let Some(super::Hosted::ChatGpt(id)) = p.hosted {
                    recent_account.set_value(Some((id, p.hosted_model.clone())));
                }
            });
        }
        match provider {
            ProviderTab::Device => draft.update(|p| {
                if p.hosted.take().is_some() {
                    p.hosted_model.clear();
                    p.access = local_access.get_value();
                }
            }),
            ProviderTab::Private
                if provider.available(reviewed_status.get_untracked().as_ref()) =>
            {
                hosted::choose(draft, reviewed_status, local_access, super::Hosted::Private);
            }
            ProviderTab::ChatGpt => {
                if let Some((id, model)) = recent_account.get_value()
                    && reviewed_status.with_untracked(|s| {
                        s.as_ref()
                            .is_some_and(|s| s.accounts.iter().any(|a| a.id == id))
                    })
                    && draft.with_untracked(|p| p.hosted != Some(super::Hosted::ChatGpt(id)))
                {
                    hosted::choose(
                        draft,
                        reviewed_status,
                        local_access,
                        super::Hosted::ChatGpt(id),
                    );
                    draft.update(|p| p.hosted_model = model);
                }
            }
            _ => {}
        }
    };
    let route_ready = move || {
        let tab = tab.get();
        draft.with(|p| ProviderTab::from_profile(p) == tab)
            && match tab {
                ProviderTab::Device => true,
                ProviderTab::Private => reviewed_status.with(|s| tab.available(s.as_ref())),
                ProviderTab::ChatGpt => reviewed_status.with(|s| {
                    s.as_ref().is_some_and(|s| {
                        s.accounts
                            .iter()
                            .any(|a| draft.with(|p| p.hosted == Some(super::Hosted::ChatGpt(a.id))))
                    })
                }),
            }
    };
    let clear_input = move || {
        if let Some(input) = key_input.get_untracked() {
            input.set_value("");
        }
    };
    Effect::new(move |_| {
        let (from, Some(el)) = (ai.settings.get(), dialog.get()) else {
            return;
        };
        if let Some(from) = from.filter(|_| !el.open()) {
            return_focus.set_value(from);
            ai.reload_profile();
            let profile = ai.profile.get_value();
            let credential = ai.credential.get_value();
            draft.set(profile.clone());
            baseline.set_value(profile);
            local_access.set_value(draft.with_untracked(|p| p.access.clone()));
            reviewed_status.set(None);
            recent_account.set_value(None);
            let initial = ProviderTab::ALL
                .into_iter()
                .find(|tab| tab.opener() == from)
                .unwrap_or_else(|| draft.with_untracked(ProviderTab::from_profile));
            select_tab(initial);
            leptos::task::spawn_local(async move {
                match ai.refresh_hosted().await {
                    Ok(status) if ai.settings.get_untracked().is_some() => {
                        if draft.with_untracked(|p| p.hosted.is_some()) {
                            draft.update(|p| p.access = status.access.clone());
                        }
                        reviewed_status.set(Some(status));
                        if tab.get_untracked() == ProviderTab::Private {
                            select_tab(ProviderTab::Private);
                        }
                    }
                    Err(_)
                        if ai.settings.get_untracked().is_some()
                            && draft.with_untracked(|p| p.hosted.is_some()) =>
                    {
                        error.set(Some("Couldn’t load list access from your server. Reopen AI settings to try again.".into()));
                    }
                    _ => {}
                }
            });
            baseline_epoch.set_value(
                ai.scope
                    .with_value(|scope| scope.as_ref().and_then(Scope::epoch)),
            );
            baseline_login.set_value(login_fingerprint());
            has_key.set(!credential.value.is_empty());
            remember_key.set(credential.remember);
            clear_input();
            error.set(None);
            if el.show_modal().is_err() {
                ai.settings.set(None);
            } else if let Some(body) = body.get_untracked() {
                body.set_scroll_top(0);
            }
        } else if from.is_none() && el.open() {
            clear_input();
            el.close();
        }
    });
    let save = move |disconnect: bool| {
        if saving.get_untracked() || (!disconnect && !untrack(route_ready)) {
            return;
        }
        let Some(scope) = ai
            .scope
            .get_value()
            .filter(Scope::active)
            .filter(|_| !ai.signed_out.get_value())
        else {
            error.set(Some("Couldn’t save assistant settings for this sign-in. Reload the app or sign in again.".into()));
            return;
        };
        let mut next = draft.get_untracked();
        if next.hosted.is_some() || (disconnect && baseline.with_value(|p| p.hosted.is_some())) {
            let previous = Profile::load(&scope);
            if previous != baseline.get_value() {
                error.set(Some(
                    "Settings changed in another tab. Reopen settings to review the changes."
                        .into(),
                ));
                return;
            }
            if disconnect {
                next.hosted = None;
                next.hosted_model.clear();
                next.address.clear();
                next.model.clear();
                next.access = local_access.get_value();
                let _ = scope.revoke();
                if next.save(&scope) {
                    ai.use_profile(next, Credential::default());
                    if let Some(el) = dialog.get_untracked() {
                        el.close();
                    }
                }
                return;
            }
            // A hosted save must not pick up unvalidated edits from the hidden device card.
            next.address = previous.address;
            next.model = previous.model;
            let Some(reviewed) = reviewed_status.get_untracked() else {
                error.set(Some(
                    "Wait for the server’s assistant settings to load.".into(),
                ));
                return;
            };
            if !valid_model(&next.hosted_model) {
                error.set(Some("Choose a model.".into()));
                return;
            }
            saving.set(true);
            leptos::task::spawn_local(async move {
                let result = if next.access == reviewed.access {
                    Ok(reviewed)
                } else {
                    hosted::save_access(relay::AccessUpdate {
                        access: next.access.clone(),
                        access_version: reviewed.access_version,
                    })
                    .await
                };
                saving.set(false);
                match result {
                    Ok(status) if scope.active() && !ai.signed_out.get_value() => {
                        next.access = local_access.get_value();
                        if !next.save(&scope) {
                            error.set(Some("This browser couldn’t save the model choice.".into()));
                            return;
                        }
                        ai.relay_status.set(Some(status));
                        ai.use_profile(next, ai.credential.get_value());
                        if let Some(el) = dialog.get_untracked() {
                            el.close();
                        }
                    }
                    Ok(_) => {}
                    Err(err) => error.set(Some(err.to_string())),
                }
            });
            return;
        }
        next.address = next.address.trim().into();
        next.model = next.model.trim().into();
        let raw_key = key_input
            .get_untracked()
            .map_or_else(String::new, |input| input.value());
        if disconnect {
            next.address.clear();
            next.model.clear();
        } else if !next.address.is_empty() || !next.model.is_empty() || !raw_key.is_empty() {
            if let Err(problem) = BrowserProvider::new(&next.address) {
                error.set(Some(problem.to_string()));
                return;
            }
            if !valid_model(&next.model) {
                error.set(Some("Enter a valid model name.".into()));
                return;
            }
        }
        let old = Profile::load(&scope);
        let reviewed_epoch = baseline_epoch.get_value();
        let login = baseline_login.get_value();
        if old != baseline.get_value()
            || scope.epoch() != reviewed_epoch
            || login_fingerprint() != login
        {
            ai.reload_profile();
            error.set(Some("Settings changed in another tab. Close this dialog and reopen it to review the new settings.".into()));
            return;
        }
        let previous_key = ai.credential.get_value();
        let value = if disconnect || next.address.is_empty() {
            String::new()
        } else if let Some(key) = bearer_key(&raw_key) {
            if key.is_empty()
                && has_key.get_untracked()
                && previous_key.matches(&next.address, reviewed_epoch)
            {
                previous_key.value.clone()
            } else {
                key.to_owned()
            }
        } else {
            error.set(Some(
                "The API key contains unsupported characters or is too long.".into(),
            ));
            return;
        };
        if !next.save(&scope) {
            error.set(Some(
                "This browser couldn’t save the assistant settings.".into(),
            ));
            return;
        }
        let epoch = if disconnect
            || next.address != old.address
            || (!previous_key.value.is_empty() && previous_key.value != value)
        {
            scope.revoke().or(reviewed_epoch)
        } else {
            reviewed_epoch
        };
        let mut key = Credential {
            address: next.address.clone(),
            epoch,
            login,
            remember: !value.is_empty() && remember_key.get_untracked(),
            value,
        };
        scope.forget();
        let remember_failed = key.remember && !scope.remember(&key);
        if remember_failed {
            key.remember = false;
        }
        has_key.set(!key.value.is_empty());
        ai.use_profile(next.clone(), key);
        clear_input();
        if remember_failed {
            remember_key.set(false);
            baseline.set_value(next);
            baseline_epoch.set_value(scope.epoch());
            error.set(Some(
                "This browser couldn’t remember the key. It’s available until you leave this page."
                    .into(),
            ));
        } else if let Some(el) = dialog.get_untracked() {
            el.close();
        }
    };
    view! {
        <dialog id="assistant-settings-dialog" class="room-dialog ai-settings" aria-labelledby="ai-settings-title" node_ref=dialog
            on:keydown=move |ev| platform::contain_tab("assistant-settings-dialog", &ev)
            on:close=move |_| {
                clear_input();
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
            <form novalidate on:submit=move |ev| { ev.prevent_default(); save(false); }
                on:click:capture=move |_| signed_in.set(None) on:input=move |_| signed_in.set(None)>
                <div class="ai-settings-body" node_ref=body>
                    <div class="ai-provider-picker" role="group" aria-label="Model provider">
                        {ProviderTab::ALL.map(|provider| view! {
                            <button type="button" class="button" aria-pressed=move || (tab.get() == provider).to_string()
                                on:click=move |_| select_tab(provider)>{provider.label()}</button>
                        })}
                    </div>
                    <hosted::HostedCards draft status=reviewed_status error local_access tab signed_in/>
                    <section class="ai-provider" aria-label="This device" data-selected="true" hidden=move || tab.get() != ProviderTab::Device>
                        <div class="ai-provider-heading">
                            {icon(Icon::Settings)}<h3>"This device"</h3>
                        </div>
                        <p>"Connect directly to a model on this device that allows this site in CORS."</p>
                        <div class="ai-provider-fields">
                        <label for="ai-base-url">"Base URL"</label>
                        <input id="ai-base-url" type="url" spellcheck="false" autocomplete="off" autofocus
                            placeholder="http://localhost:11434" prop:value=move || draft.with(|p| p.address.clone())
                            on:input=move |ev| {
                                draft.update(|p| { p.address = event_target_value(&ev); });
                                clear_input();
                                has_key.set(false);
                                remember_key.set(false);
                            }/>
                        <label for="ai-model">"Model"</label>
                        <input id="ai-model" type="text" maxlength="256" autocomplete="off" spellcheck="false"
                            placeholder="Model name" prop:value=move || draft.with(|p| p.model.clone())
                            on:input=move |ev| draft.update(|p| { p.model = event_target_value(&ev); })/>
                        <label for="ai-api-key">"API key"</label>
                        <input id="ai-api-key" type="password" autocomplete="off" spellcheck="false" maxlength="4096"
                            node_ref=key_input placeholder=move || if has_key.get() { "Key saved · enter to replace" } else { "Optional" }/>
                        <div class="ai-credential-controls">
                            <label class="ai-consent">
                                <input type="checkbox" prop:checked=move || remember_key.get()
                                    on:change=move |ev| remember_key.set(event_target_checked(&ev))/>
                                <span>"Remember in this tab"</span>
                            </label>
                            <Show when=move || has_key.get()>
                                <button type="button" class="ai-link" on:click=move |_| {
                                    clear_input(); has_key.set(false); remember_key.set(false);
                                }>"Remove key"</button>
                            </Show>
                        </div>
                        </div>
                    </section>
                    <section class="ai-access" aria-labelledby="ai-access-title">
                        <h3 id="ai-access-title">"Lists the assistant can see"</h3>
                        <p>"Hidden lists are never sent. Private only lists can go to a model on this device or through your server’s private relay. Lists inherit their collection’s rule unless overridden."</p>
                        <Show when=route_ready>
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
                        </Show>
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
                        <button type="submit" class="button primary" disabled=move || saving.get() || !route_ready()>"Save"</button>
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
