//! The web client: a Leptos app compiled to wasm32, served by `todo-server` at `/app`.
//! See `docs/web.md`.
//!
//! The view-model ([`model`]) and the landscape markup ([`landscape`]) are plain Rust and tested on the
//! host with `cargo test`; everything that touches the browser is compiled for wasm32 only.
// On the host only the tests use the view-model.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

mod changes;
#[cfg(any(test, feature = "fixtures"))]
mod fixtures;
mod landscape;
mod model;
mod reminder_alerts;
mod reminder_time;

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod backend;
#[cfg(target_arch = "wasm32")]
mod platform;

#[cfg(target_arch = "wasm32")]
fn main() {
    wasm_bindgen_futures::spawn_local(start());
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("todo-web runs in a browser: build it with `node apps/web/scripts/build.mjs`.");
}

#[cfg(target_arch = "wasm32")]
async fn start() {
    #[cfg(feature = "fixtures")]
    {
        // Invented data, nothing sent anywhere. Only the clock tests use the browser's clock.
        let query = platform::query();
        let dst = query.contains("fixture=dst");
        let now = if dst
            || query.contains("fixture=browser-clock")
            || query.contains("fixture=reminder-alerts")
        {
            platform::now()
        } else {
            fixtures::now()
        };
        let state = if dst {
            fixtures::fallback_state()
        } else if query.contains("fixture=blank") {
            todo_core::State::new()
        } else if query.contains("fixture=finished-no-tomorrow") {
            fixtures::finished_without_tomorrow()
        } else if query.contains("fixture=finished") {
            fixtures::finished_state()
        } else if query.contains("fixture=agent-rows") {
            fixtures::agent_rows_state()
        } else if query.contains("fixture=trash") {
            fixtures::trash_state()
        } else if query.contains("fixture=task-activity") {
            fixtures::task_activity_state()
        } else if query.contains("fixture=workflow") || query.contains("fixture=timeline") {
            fixtures::workflow_state(query.contains("fixture=timeline"))
        } else if query.contains("fixture=detail-wide") {
            fixtures::wide_detail_state()
        } else if query.contains("fixture=notes-markdown") || query.contains("fixture=notes-code") {
            fixtures::notes_markdown_state(query.contains("fixture=notes-code"))
        } else {
            fixtures::state()
        };
        backend::open_fixtures(state, now);
        app::mount(now, false, None);
    }
    #[cfg(not(feature = "fixtures"))]
    {
        let Some(csrf) = platform::csrf() else {
            fail(&backend::StartError::NoCsrf.to_string());
            return;
        };
        let blocked: std::rc::Rc<dyn Fn(bool)> = std::rc::Rc::new(|waiting| {
            platform::boot_notice(waiting.then_some("Close other To-do tabs to finish updating"));
        });
        match backend::open_live(csrf, blocked).await {
            Ok(opened) => {
                wasm_bindgen_futures::spawn_local(opened.flusher.run());
                platform::on_leave(backend::unsaved, backend::flush_now);
                let r = &opened.recovery;
                let notice = r.has_damaged().then(|| {
                    let n = r.damaged.len() + r.damaged_before;
                    format!(
                        "{n} unsent change{} couldn’t be read. {} kept in this browser, set aside",
                        if n == 1 { "" } else { "s" },
                        if n == 1 { "It is" } else { "They are" },
                    )
                });
                app::mount(platform::now(), true, notice);
                wasm_bindgen_futures::spawn_local(backend::run());
            }
            Err(e @ backend::StartError::Storage(_)) => fail_storage(&e.to_string()),
            Err(e) => fail(&e.to_string()),
        }
    }
}

/// Local storage failed: say so, and offer to reset it (after a confirmation, since it deletes
/// changes this browser has not sent yet). Nothing is deleted without that.
#[cfg(all(target_arch = "wasm32", not(feature = "fixtures")))]
fn fail_storage(message: &str) {
    use leptos::prelude::*;
    let message = message.to_owned();
    let reset = move |_| {
        let ok = web_sys::window().is_some_and(|w| {
            w.confirm_with_message(
                "Reset local data? This deletes this browser’s copy of your tasks, including \
                 changes it has not sent to the server yet. Everything the server has comes back.",
            )
            .unwrap_or(false)
        });
        if ok {
            wasm_bindgen_futures::spawn_local(async {
                if let Err(e) = backend::reset_local_data().await {
                    tracing::warn!("reset failed: {e}");
                }
                if let Some(w) = web_sys::window() {
                    let _ = w.location().reload();
                }
            });
        }
    };
    leptos::mount::mount_to_body(move || {
        view! {
            <main class="fatal">
                <h1 class="room-title">"To-do"</h1>
                <p>{format!("The app couldn’t open its local data: {message}.")}</p>
                <p>"Changes made in this browser and not yet sent are still stored. Closing other tabs of the app and trying again often helps."</p>
                <p class="fatal-actions">
                    <a href="/app">"Try again"</a>
                    <button class="danger" on:click=reset>"Reset local data"</button>
                </p>
            </main>
        }
    });
}

#[cfg(all(target_arch = "wasm32", not(feature = "fixtures")))]
fn fail(message: &str) {
    use leptos::prelude::*;
    let message = message.to_owned();
    leptos::mount::mount_to_body(move || {
        view! {
            <main class="fatal">
                <h1 class="room-title">"To-do"</h1>
                <p>{format!("The app couldn’t start: {message}.")}</p>
                <p><a href="/app">"Try again"</a></p>
            </main>
        }
    });
}
