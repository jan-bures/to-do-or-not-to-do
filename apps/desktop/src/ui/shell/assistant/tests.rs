use super::*;
use gpui::{TestApp, TestAppWindow};

fn shell(app: &mut TestApp) -> TestAppWindow<ShellView> {
    app.update(|cx| {
        cx.bind_keys(crate::ui::input::key_bindings());
        cx.bind_keys(crate::ui::text_area::key_bindings());
        cx.bind_keys(super::super::key_bindings());
    });
    app.open_window(|window, cx| {
        let fixture = crate::fixtures::my_day();
        let mut shell = ShellView::new(
            Rc::new(Store::fixture(&fixture)),
            fixture.ui,
            Theme::Light,
            None,
            window,
            cx,
        );
        shell.still();
        window.focus(&shell.list_focus, cx);
        shell
    })
}

fn press(window: &mut TestAppWindow<ShellView>, key: &'static str) {
    window.draw();
    window.simulate_keystroke(key);
    window.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).expect("fixture key"),
    });
}

#[test]
fn assistant_dates_follow_the_current_display_preference() {
    let mut app = TestApp::new();
    let window = shell(&mut app);
    window.read(|shell, _| {
        let date = Some("2026-11-23".parse().expect("fixture date"));
        assert_eq!(shell.ai_date(date), "Mon 23.11.");
        shell
            .store
            .set_date_format(todo_client::date_format::DateFormat::Iso)
            .expect("set format");
        assert_eq!(shell.ai_date(date), "Mon 2026-11-23");
        assert_eq!(shell.ai_date(None), "No date");
    });
}

#[test]
fn shortcut_opens_an_unconfigured_chat_without_erasing_task_selection() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    let selected = window.read(|shell, _| shell.ui.task);
    press(&mut window, "ctrl-j");
    window.update(|shell, window, cx| {
        assert!(shell.ai.open);
        assert!(shell.ai.panel.is_focused(window));
        assert_eq!(shell.ui.task, selected);
        assert_eq!(shell.ai.view().composer, Composer::ChooseModel);
        shell.open_ai_settings(window, cx);
        assert!(shell.ai.modal());
    });
    press(&mut window, "ctrl-j");
    window.read(|shell, _| {
        assert!(
            shell.ai.modal(),
            "the settings dialog traps the shell shortcut"
        )
    });
    press(&mut window, "escape");
    window.update(|shell, window, _| {
        assert!(!shell.ai.modal());
        assert!(shell.ai.panel.is_focused(window));
    });
    press(&mut window, "ctrl-j");
    window.update(|shell, window, _| {
        assert!(!shell.ai.open);
        assert!(shell.list_focus.is_focused(window));
        assert_eq!(shell.ui.task, selected);
    });
}

#[test]
fn ready_composer_accepts_multiline_drafts_and_escape_clears_before_closing() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    window.update(|shell, window, cx| {
        shell.preview_assistant("ai-empty", window, cx);
        shell.focus_ai(window, cx);
    });
    window.draw();
    window.simulate_input("first line");
    press(&mut window, "shift-enter");
    window.simulate_input("second line");
    window.read(|shell, cx| {
        assert_eq!(
            shell.ai.composer.read(cx).content.as_ref(),
            "first line\nsecond line"
        )
    });
    press(&mut window, "escape");
    window.read(|shell, cx| {
        assert!(shell.ai.open);
        assert!(shell.ai.composer.read(cx).content.is_empty());
    });
    press(&mut window, "escape");
    window.read(|shell, _| assert!(!shell.ai.open));
}

#[test]
fn connection_health_tracks_active_probes_and_network_failures() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    window
        .update(|shell, _, _| assert_eq!(shell.ai.connection_health(), ConnectionHealth::Unknown));
    window.update(|shell, window, cx| {
        shell.preview_assistant("ai-settings-testing", window, cx);
        assert_eq!(shell.ai.connection_health(), ConnectionHealth::Testing);
        shell.close_ai_settings(window, cx);
        assert_eq!(shell.ai.connection_health(), ConnectionHealth::Connected);
        shell.preview_assistant("ai-error", window, cx);
        shell.ai.connection = ConnectionHealth::Connected;
        shell.observe_ai_error(cx);
        assert_eq!(shell.ai.connection_health(), ConnectionHealth::Unreachable);
        assert!(!unreachable(ProviderError::Unauthorized));
    });
}

#[test]
fn changing_destination_invalidates_probe_scope_and_saved_key_without_contacting_it() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    window.update(|shell, window, cx| {
        shell.preview_assistant("ai-settings", window, cx);
        let setup = shell.ai.settings.as_mut().expect("fixture settings");
        setup.draft.attested = true;
        let revision = setup.revision;
        assert!(!setup.draft.key.is_empty() && !setup.draft.access.lists.is_empty());
        assert!(!setup.draft.access.collections.is_empty());
        shell.ai.url.update(cx, |input, cx| {
            input.set_text("https://other.example.com/v1", cx)
        });
        shell.ai_setting_changed(Field::Url, cx);
        let setup = shell.ai.settings.as_ref().expect("edited settings");
        assert_ne!(setup.revision, revision);
        assert!(setup.draft.key.is_empty() && setup.draft.access == Default::default());
        assert!(!setup.draft.attested);
        assert_eq!(
            setup.draft.endpoint().expect("HTTPS endpoint").destination,
            Destination::NotPrivate
        );
        assert!(setup.draft.support.is_none() && setup.models.is_empty());
        assert!(shell.ai.key.read(cx).content.is_empty());
        shell.close_ai_settings(window, cx);
        assert!(!shell.ai.modal());
        assert!(
            !shell.ai.profile.key.is_empty(),
            "cancel keeps the saved profile unchanged"
        );
    });
}

#[test]
fn review_each_navigates_without_changing_the_frozen_preview() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    window.update(|shell, window, cx| shell.preview_assistant("ai-batch", window, cx));
    window.draw();
    window.update(|shell, window, cx| {
        window.focus(&shell.ai.focus_handle("assistant-review-each", cx), cx)
    });
    press(&mut window, "enter");
    window.read(|shell, _| assert_eq!(shell.ai.review.each, Some(0)));
    window.update(|shell, window, cx| {
        window.focus(&shell.ai.focus_handle("assistant-review-next", cx), cx)
    });
    press(&mut window, "enter");
    window.read(|shell, _| {
        assert_eq!(shell.ai.review.each, Some(1));
        assert!(matches!(
            shell.ai.view().composer,
            Composer::Review {
                selected: 4,
                total: 5,
                ..
            }
        ));
    });
}

#[test]
fn public_https_is_blocked_until_the_owner_attests_in_settings() {
    let mut app = TestApp::new();
    let mut window = shell(&mut app);
    window.update(|shell, window, cx| {
        shell.preview_assistant("ai-blocked", window, cx);
        assert!(shell.ai.blocked());
        assert!(
            shell
                .ai_destination()
                .expect("status destination")
                .starts_with("Not private")
        );
        shell.focus_ai(window, cx);
        assert!(shell.ai.panel.is_focused(window));
        shell.open_ai_settings(window, cx);
    });
    window.draw();
    window.update(|shell, window, cx| {
        window.focus(&shell.ai.focus_handle("assistant-attestation", cx), cx)
    });
    press(&mut window, "enter");
    window.read(|shell, _| {
        let setup = shell.ai.settings.as_ref().expect("settings draft");
        assert!(setup.draft.attested);
        assert_eq!(
            setup
                .draft
                .endpoint()
                .expect("attested endpoint")
                .destination,
            Destination::YourServer
        );
        assert!(
            shell.ai.blocked(),
            "a draft does not change the active destination until saved"
        );
    });
}

#[test]
fn errors_are_typed_and_use_owner_facing_words() {
    assert!(problem(ProviderError::Unauthorized).starts_with("API key required or invalid"));
    assert!(problem(ProviderError::Unavailable).contains("may be asleep"));
    assert!(problem(ProviderError::ModelMissing).contains("selected model"));
    assert!(
        problem(ProviderError::TailnetDisconnected)
            .contains("Tailscale isn't connected on this PC")
    );
}
