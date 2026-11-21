//! The web client: a Leptos app compiled to wasm32, served by `todo-server` at `/app`.
//! See `docs/web.md`.
//!
//! The view-model ([`model`]) and the landscape markup ([`landscape`]) are plain Rust and tested on the
//! host with `cargo test`; everything that touches the browser is compiled for wasm32 only.
// On the host only the tests use the view-model.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

#[cfg(any(test, feature = "fixtures"))]
mod fixtures;
mod landscape;
mod model;

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
        // The screenshot gallery: invented data, a fixed clock, nothing sent anywhere.
        let state = if platform::query().contains("fixture=finished") {
            fixtures::finished_state()
        } else {
            fixtures::state()
        };
        backend::open_fixtures(state, fixtures::now());
        app::mount(fixtures::now(), false);
    }
    #[cfg(not(feature = "fixtures"))]
    {
        let Some(csrf) = platform::csrf() else {
            fail(&backend::StartError::NoCsrf.to_string());
            return;
        };
        match backend::open_live(csrf).await {
            Ok(flusher) => {
                wasm_bindgen_futures::spawn_local(flusher.run());
                app::mount(platform::now(), true);
                wasm_bindgen_futures::spawn_local(backend::run());
            }
            Err(e) => fail(&e.to_string()),
        }
    }
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
