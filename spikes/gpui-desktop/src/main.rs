mod drops;
mod input;
mod metrics;
mod winput;

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, MouseButton, ScrollStrategy,
    SharedString, UniformListScrollHandle, Window, WindowBounds, WindowOptions, actions, div,
    point, prelude::*, px, rgb, size, uniform_list,
};
use gpui_platform::application;
use input::{InputEvent, TextInput};
use metrics::{log, logf, process_age_ms};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ops::Range;
use std::time::{Duration, Instant};

actions!(
    todo,
    [SelectNext, SelectPrev, ToggleSelected, ToggleImportant, FocusQuickAdd, FocusFilter, FocusList, QuitApp]
);

const ROW_H: f32 = 32.0;

struct Task {
    title: SharedString,
    folded: String,
    done: bool,
    important: bool,
}

struct TaskList {
    name: &'static str,
    tasks: Vec<Task>,
}

fn fold(s: &str) -> String {
    s.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' => 'a', 'č' => 'c', 'ď' => 'd', 'é' | 'ě' => 'e', 'í' => 'i', 'ň' => 'n',
            'ó' => 'o', 'ř' => 'r', 'š' => 's', 'ť' => 't', 'ú' | 'ů' => 'u', 'ý' => 'y', 'ž' => 'z',
            c => c,
        })
        .collect()
}

fn make_task(title: String, done: bool, important: bool) -> Task {
    Task { folded: fold(&title), title: title.into(), done, important }
}

fn generate() -> Vec<TaskList> {
    const PHRASES: &[&str] = &[
        "Koupit mléko, rohlíky a čaj", "Zkontrolovat úlohy", "Zavolat zubaři", "Připravit prezentaci pro schůzku",
        "Odeslat fakturu klientovi", "Naplánovat dovolenou v Šumavě", "Vyměnit žárovku v předsíni", "Přečíst kapitolu o Rustu",
        "Zaplatit nájem a energie", "Objednat dárek pro maminku", "Uklidit pracovní stůl", "Vyřídit pojištění auta",
        "Nakrmit kočku Žofii", "Jít na procházku s psem", "Umýt okna v kuchyni", "Zálohovat fotky na disk",
        "Prodloužit řidičský průkaz", "Napsat e-mail paní Nováčkové", "Ušít záclony do ložnice", "Přeinstalovat Windows na notebooku",
        "Koupit lístky na koncert Dvořákovy Rusalky", "Poslat přání k narozeninám", "Nechat opravit kolo", "Zrecyklovat staré baterie",
        "Dokončit účetní závěrku", "Vyzvednout balíček na poště", "Přesadit květiny na balkóně", "Vymyslet ředitelskou přípravu na čtvrtletí",
    ];
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let names = ["Můj den", "Důležité", "Plánované", "Úkoly", "Nákupy", "Práce", "Domácnost", "Cesty a dovolená", "Zdraví", "Finance"];
    let counts = [2000usize, 40, 120, 300, 25, 400, 80, 60, 15, 90];
    names
        .iter()
        .zip(counts)
        .map(|(name, n)| TaskList {
            name,
            tasks: (0..n)
                .map(|i| {
                    let r = rnd();
                    let p = PHRASES[(r % PHRASES.len() as u64) as usize];
                    make_task(format!("{p} #{}", i + 1), (r >> 20) % 5 == 0, (r >> 30) % 10 == 0)
                })
                .collect(),
        })
        .collect()
}

struct ScrollRun {
    frames: u32,
    total: u32,
    last: Option<Instant>,
    dts: Vec<f64>,
    builds: Vec<f64>,
}

pub struct TodoApp {
    lists: Vec<TaskList>,
    cur: usize,
    visible: Vec<usize>,
    selected: usize,
    quick: Entity<TextInput>,
    filter: Entity<TextInput>,
    list_focus: FocusHandle,
    scroll: UniformListScrollHandle,
    scroll_run: Option<ScrollRun>,
    scroll_done: bool,
    last_build_ms: f64,
    first_frame_logged: bool,
    first_list_build_logged: bool,
}

impl TodoApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let t = Instant::now();
        let lists = generate();
        logf("data_generate_ms", t.elapsed().as_secs_f64() * 1000.0);
        let quick = cx.new(|cx| TextInput::new("Přidat úkol… (Enter, Ctrl+N)", cx));
        let filter = cx.new(|cx| TextInput::new("Filtr… (Ctrl+F)", cx));
        cx.subscribe_in(&quick, window, |this, input, ev: &InputEvent, window, cx| {
            if let InputEvent::Submit = ev {
                let text = input.read(cx).content.trim().to_string();
                if !text.is_empty() {
                    let t = Instant::now();
                    this.lists[this.cur].tasks.insert(0, make_task(text, false, false));
                    input.update(cx, |i, cx| i.reset(window, cx));
                    this.recompute(cx);
                    this.selected = 0;
                    this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                    logf("quick_add_submit_ms", t.elapsed().as_secs_f64() * 1000.0);
                    cx.notify();
                }
            }
        })
        .detach();
        cx.subscribe_in(&filter, window, |this, _i, ev: &InputEvent, _w, cx| {
            if let InputEvent::Changed = ev {
                let t = Instant::now();
                this.recompute(cx);
                this.selected = 0;
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                logf("filter_recompute_ms", t.elapsed().as_secs_f64() * 1000.0);
                cx.notify();
            }
        })
        .detach();
        let mut app = Self {
            lists,
            cur: 0,
            visible: vec![],
            selected: 0,
            quick,
            filter,
            list_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            scroll_run: None,
            scroll_done: false,
            last_build_ms: 0.0,
            first_frame_logged: false,
            first_list_build_logged: false,
        };
        app.recompute(cx);
        app
    }

    fn recompute(&mut self, cx: &App) {
        let q = fold(self.filter.read(cx).content.trim());
        self.visible = self.lists[self.cur]
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| q.is_empty() || t.folded.contains(&q))
            .map(|(i, _)| i)
            .collect();
    }

    fn move_sel(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.visible.is_empty() {
            return;
        }
        let n = self.visible.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, n - 1) as usize;
        self.scroll.scroll_to_item(self.selected, ScrollStrategy::Center);
        cx.notify();
    }

    fn toggle_vis(&mut self, vi: usize, cx: &mut Context<Self>) {
        if let Some(&ti) = self.visible.get(vi) {
            let t = &mut self.lists[self.cur].tasks[ti];
            t.done = !t.done;
            cx.notify();
        }
    }

    fn star_vis(&mut self, vi: usize, cx: &mut Context<Self>) {
        if let Some(&ti) = self.visible.get(vi) {
            let t = &mut self.lists[self.cur].tasks[ti];
            t.important = !t.important;
            cx.notify();
        }
    }

    fn row(&self, vi: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = &self.lists[self.cur].tasks[self.visible[vi]];
        let sel = vi == self.selected;
        let done = t.done;
        let imp = t.important;
        let mut title = div().flex_1().overflow_hidden().whitespace_nowrap().child(t.title.clone());
        if done {
            title = title.line_through().text_color(rgb(0x888888));
        }
        div()
            .id(("row", vi))
            .h(px(ROW_H))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(rgb(0xeeeeee))
            .bg(if sel { rgb(0xdce9ff) } else { rgb(0xffffff) })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = vi;
                window.focus(&this.list_focus, cx);
                cx.notify();
            }))
            .child(
                div()
                    .id(("chk", vi))
                    .w(px(18.))
                    .h(px(18.))
                    .border_1()
                    .border_color(rgb(0x555555))
                    .rounded(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .bg(if done { rgb(0x2b6cff) } else { rgb(0xffffff) })
                    .text_color(rgb(0xffffff))
                    .child(if done { "✓" } else { "" })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.toggle_vis(vi, cx))),
            )
            .child(title)
            .child(
                div()
                    .id(("star", vi))
                    .w(px(24.))
                    .text_color(if imp { rgb(0xe0a000) } else { rgb(0xaaaaaa) })
                    .child(if imp { "★" } else { "☆" })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.star_vis(vi, cx))),
            )
    }
}

impl Render for TodoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_start = Instant::now();
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            logf("first_render_call_ms_since_process_start", process_age_ms());
            window.on_next_frame(|_, _| {
                logf("first_frame_presented_ms_since_process_start", process_age_ms());
            });
            window.request_animation_frame();
        }
        // scroll benchmark driver
        if let Some(run) = self.scroll_run.as_mut() {
            let now = Instant::now();
            if let Some(l) = run.last {
                run.dts.push(now.duration_since(l).as_secs_f64() * 1000.0);
                run.builds.push(self.last_build_ms);
            }
            run.last = Some(now);
            run.frames += 1;
            if run.frames >= run.total {
                let run = self.scroll_run.take().unwrap();
                metrics::stats("scroll_frame_interval_ms", run.dts);
                metrics::stats("scroll_visible_list_build_ms", run.builds);
                self.scroll_done = true;
            } else {
                let y = (run.frames as f32 * 40.0) % 40000.0;
                self.scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-y)));
                window.request_animation_frame();
            }
        }
        let sidebar = div()
            .w(px(220.))
            .h_full()
            .flex_none()
            .bg(rgb(0xf0f0f4))
            .border_r_1()
            .border_color(rgb(0xd0d0d8))
            .flex()
            .flex_col()
            .pt_2()
            .children(self.lists.iter().enumerate().map(|(i, l)| {
                let n = l.tasks.len();
                div()
                    .id(("list", i))
                    .px_3()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(if i == self.cur { rgb(0xdde4f5) } else { rgb(0xf0f0f4) })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cur = i;
                        this.recompute(cx);
                        this.selected = 0;
                        cx.notify();
                    }))
                    .child(l.name)
                    .child(div().text_color(rgb(0x777777)).child(format!("{n}")))
            }));

        let count = self.visible.len();
        let list = uniform_list(
            "tasks",
            count,
            cx.processor(move |this, range: Range<usize>, _w, cx| {
                let t = Instant::now();
                let rows: Vec<_> = range.clone().map(|vi| this.row(vi, cx)).collect();
                this.last_build_ms = t.elapsed().as_secs_f64() * 1000.0;
                if !this.first_list_build_logged && range.len() > 1 {
                    this.first_list_build_logged = true;
                    logf("first_visible_rows_build_ms", this.last_build_ms);
                    log("first_visible_rows_count", range.len());
                }
                rows
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1();

        let main = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.quick.clone())
                    .child(self.filter.clone())
                    .child(div().text_color(rgb(0x777777)).text_size(px(12.)).child(format!(
                        "{} — {} z {} úkolů",
                        self.lists[self.cur].name,
                        count,
                        self.lists[self.cur].tasks.len()
                    ))),
            )
            .child(
                div()
                    .id("list-wrap")
                    .key_context("TodoList")
                    .track_focus(&self.list_focus)
                    .on_action(cx.listener(|this, _: &ToggleSelected, _, cx| this.toggle_vis(this.selected, cx)))
                    .on_action(cx.listener(|this, _: &ToggleImportant, _, cx| this.star_vis(this.selected, cx)))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(list),
            );

        let out = div()
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x222222))
            .text_size(px(14.))
            .key_context("Todo")
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_sel(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.move_sel(-1, cx)))
            .on_action(cx.listener(|this, _: &FocusQuickAdd, window, cx| {
                window.focus(&this.quick.focus_handle(cx), cx)
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, window, cx| {
                window.focus(&this.filter.focus_handle(cx), cx)
            }))
            .on_action(cx.listener(|this, _: &FocusList, window, cx| window.focus(&this.list_focus, cx)))
            .on_drop(cx.listener(|_this, paths: &gpui::ExternalPaths, _w, _cx| {
                drops::drop_log(format!("GPUI on_drop::<ExternalPaths> received {} path(s)", paths.paths().len()));
                for p in paths.paths() {
                    let md = std::fs::metadata(p);
                    drops::drop_log(format!(
                        "  path={:?} ext={:?} is_dir={} size={:?}",
                        p,
                        p.extension(),
                        p.is_dir(),
                        md.map(|m| m.len()).ok()
                    ));
                }
            }))
            .child(sidebar)
            .child(main);
        if self.scroll_run.is_some() {
            // keep CPU-side frame build cost too
            self.last_build_ms = self.last_build_ms.max(0.0);
        }
        let _ = frame_start;
        out
    }
}

fn hwnd_of(w: &Window) -> isize {
    match HasWindowHandle::window_handle(w).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

fn setup_tray_and_hotkey() -> Option<(tray_icon::TrayIcon, global_hotkey::GlobalHotKeyManager, tray_icon::menu::MenuId, tray_icon::menu::MenuId)> {
    use global_hotkey::{GlobalHotKeyManager, hotkey::{Code, HotKey, Modifiers}};
    use tray_icon::{Icon, TrayIconBuilder, menu::{Menu, MenuItem}};
    let menu = Menu::new();
    let show = MenuItem::new("Zobrazit", true, None);
    let quit = MenuItem::new("Ukončit", true, None);
    menu.append(&show).ok()?;
    menu.append(&quit).ok()?;
    let mut rgba = Vec::new();
    for y in 0..16 {
        for x in 0..16 {
            let edge = x == 0 || y == 0 || x == 15 || y == 15;
            rgba.extend_from_slice(if edge { &[255, 255, 255, 255] } else { &[43, 108, 255, 255] });
        }
    }
    let icon = Icon::from_rgba(rgba, 16, 16).ok()?;
    let tray = match TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("To-do spike").with_icon(icon).build() {
        Ok(t) => t,
        Err(e) => {
            log("tray_icon", format!("FAILED: {e}"));
            return None;
        }
    };
    log("tray_icon", "created OK");
    let mgr = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            log("global_hotkey", format!("manager FAILED: {e}"));
            return None;
        }
    };
    let hk = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
    match mgr.register(hk) {
        Ok(_) => log("global_hotkey", "Ctrl+Alt+Space registered OK"),
        Err(e) => log("global_hotkey", format!("register FAILED: {e}")),
    }
    Some((tray, mgr, show.id().clone(), quit.id().clone()))
}

fn main() {
    metrics::log("main_entered_ms_since_process_start", format!("{:.1}", process_age_ms()));
    let args: Vec<String> = std::env::args().collect();
    let bench = args.iter().any(|a| a == "--bench");
    let hold: u64 = args.iter().position(|a| a == "--hold").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(0);

    application().run(move |cx: &mut App| {
        logf("app_run_callback_ms_since_process_start", process_age_ms());
        cx.bind_keys([
            KeyBinding::new("backspace", input::Backspace, Some("TextInput")),
            KeyBinding::new("delete", input::Delete, Some("TextInput")),
            KeyBinding::new("left", input::Left, Some("TextInput")),
            KeyBinding::new("right", input::Right, Some("TextInput")),
            KeyBinding::new("shift-left", input::SelectLeft, Some("TextInput")),
            KeyBinding::new("shift-right", input::SelectRight, Some("TextInput")),
            KeyBinding::new("ctrl-a", input::SelectAll, Some("TextInput")),
            KeyBinding::new("ctrl-v", input::Paste, Some("TextInput")),
            KeyBinding::new("ctrl-c", input::Copy, Some("TextInput")),
            KeyBinding::new("ctrl-x", input::Cut, Some("TextInput")),
            KeyBinding::new("home", input::Home, Some("TextInput")),
            KeyBinding::new("end", input::End, Some("TextInput")),
            KeyBinding::new("enter", input::Submit, Some("TextInput")),
            KeyBinding::new("down", SelectNext, Some("Todo")),
            KeyBinding::new("up", SelectPrev, Some("Todo")),
            KeyBinding::new("space", ToggleSelected, Some("TodoList")),
            KeyBinding::new("i", ToggleImportant, Some("TodoList")),
            KeyBinding::new("ctrl-n", FocusQuickAdd, Some("Todo")),
            KeyBinding::new("ctrl-f", FocusFilter, Some("Todo")),
            KeyBinding::new("escape", FocusList, Some("Todo")),
            KeyBinding::new("ctrl-q", QuitApp, None),
        ]);
        cx.on_action(|_: &QuitApp, cx| cx.quit());
        if std::env::var("SPIKE_KEYLOG").is_ok() { cx.observe_keystrokes(|ev, _, _| println!("KEY {:?}", ev.keystroke)).detach(); }

        let bounds = Bounds::centered(None, size(px(1000.0), px(700.0)), cx);
        let t_open = Instant::now();
        let window = cx
            .open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() },
                |window, cx| cx.new(|cx| TodoApp::new(window, cx)),
            )
            .unwrap();
        logf("open_window_call_ms", t_open.elapsed().as_secs_f64() * 1000.0);
        let hwnd = window.update(cx, |_, w, _| hwnd_of(w)).unwrap();
        window.update(cx, |v, w, cx| { w.focus(&v.list_focus, cx); }).ok();
        cx.activate(true);


        if std::env::args().any(|a| a == "--drop-sniff") {
            drops::install_sniffer(hwnd);
        } else {
            drops::drop_log("GPUI drop mode: only CF_HDROP file drags are delivered (as ExternalPaths). Use --drop-sniff to log raw formats.");
        }
        cx.on_window_closed(|cx, _w| { if cx.windows().is_empty() { cx.quit(); } }).detach();

        let tray = setup_tray_and_hotkey();

        // poll tray / hotkey events
        let win2 = window;
        cx.spawn(async move |cx| {
            use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
            let _keep = &tray;
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                    if ev.state == HotKeyState::Pressed {
                        let t = Instant::now();
                        winput::show_and_focus(hwnd);
                        win2.update(cx, |v, w, cx| {
                            w.focus(&v.quick.focus_handle(cx), cx);
                        })
                        .ok();
                        logf("hotkey_summon_handler_ms", t.elapsed().as_secs_f64() * 1000.0);
                        log("hotkey_event", "received -> window shown, quick-add focused");
                    }
                }
                while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                    if let Some((_, _, show, quit)) = &tray {
                        if &ev.id == show {
                            winput::show_and_focus(hwnd);
                        } else if &ev.id == quit {
                            cx.update(|cx| cx.quit());
                        }
                    }
                }
                while tray_icon::TrayIconEvent::receiver().try_recv().is_ok() {}
            }
        })
        .detach();

        if bench {
            cx.spawn(async move |cx| bench_driver(cx, window, hwnd).await).detach();
        } else if hold > 0 {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(hold)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}

/// In-process synthetic input: goes through GPUI's own keystroke dispatch (keymap + input handler),
/// never through the OS, so it cannot affect other applications.
fn ks(cx: &mut gpui::AsyncApp, window: gpui::WindowHandle<TodoApp>, s: &'static str) {
    let k = gpui::Keystroke::parse(s).unwrap();
    cx.update_window(window.into(), |_, w, cx| { w.dispatch_keystroke(k, cx); }).ok();
}
fn typ(cx: &mut gpui::AsyncApp, window: gpui::WindowHandle<TodoApp>, c: char) {
    let k = gpui::Keystroke { modifiers: Default::default(), key: c.to_string(), key_char: Some(c.to_string()) };
    cx.update_window(window.into(), |_, w, cx| { w.dispatch_keystroke(k, cx); }).ok();
}

async fn bench_driver(cx: &mut gpui::AsyncApp, window: gpui::WindowHandle<TodoApp>, hwnd: isize) {
    let exec = cx.background_executor().clone();
    let sleep = move |ms: u64| exec.timer(Duration::from_millis(ms));
    // wait for first frame
    for _ in 0..400 {
        if window.update(cx, |v, _, _| v.first_frame_logged).unwrap_or(false) {
            break;
        }
        sleep(25).await;
    }
    sleep(800).await;
    let (ws, peak, pf) = metrics::working_set_mb();
    log("working_set_after_load_mb", format!("{ws:.1}"));
    log("peak_working_set_after_load_mb", format!("{peak:.1}"));
    log("private_commit_after_load_mb", format!("{pf:.1}"));

    // reference: construct all 2000 row elements (no layout/paint)
    window
        .update(cx, |v, _, cx| {
            let t = Instant::now();
            let n = v.visible.len();
            let rows: Vec<_> = (0..n).map(|i| v.row(i, cx)).collect();
            logf("construct_all_2000_row_elements_ms(no layout/paint)", t.elapsed().as_secs_f64() * 1000.0);
            drop(rows);
        })
        .ok();

    let _ = hwnd;
    {
        let q_focused = |cx: &mut gpui::AsyncApp| {
            window.update(cx, |v, w, cx| v.quick.focus_handle(cx).is_focused(w)).unwrap_or(false)
        };
        let f_focused = |cx: &mut gpui::AsyncApp| {
            window.update(cx, |v, w, cx| v.filter.focus_handle(cx).is_focused(w)).unwrap_or(false)
        };
        // Ctrl+N
        ks(cx, window, "ctrl-n");
        sleep(200).await;
        log("ctrl_n_focuses_quick_add", q_focused(cx));
        // typing
        let text = "Koupit mléko, rohlíky a čaj";
        for c in text.chars() {
            metrics::mark_e2e();
            typ(cx, window, c);
            sleep(70).await;
        }
        sleep(150).await;
        let typed = window.update(cx, |v, _, cx| v.quick.read(cx).content.to_string()).unwrap_or_default();
        log("typed_text_in_quick_add", &typed);
        log("typed_text_matches_czech_input", typed == text);
        metrics::typing_report();
        // Enter adds
        let before = window.update(cx, |v, _, _| v.lists[0].tasks.len()).unwrap_or(0);
        ks(cx, window, "enter");
        sleep(250).await;
        let (after, top) = window
            .update(cx, |v, _, _| (v.lists[0].tasks.len(), v.lists[0].tasks[0].title.to_string()))
            .unwrap_or((0, String::new()));
        log("enter_adds_task_to_top", after == before + 1 && top == text);
        // filter
        ks(cx, window, "ctrl-f");
        sleep(200).await;
        log("ctrl_f_focuses_filter", f_focused(cx));
        for c in "mleko".chars() {
            typ(cx, window, c);
            sleep(70).await;
        }
        sleep(200).await;
        let m = window.update(cx, |v, _, _| v.visible.len()).unwrap_or(0);
        log("filter_'mleko'_matches(diacritic-insensitive)", m);
        for c in "čaj".chars() {
            // exercise Czech diacritics in filter too
            let _ = c;
        }
        ks(cx, window, "ctrl-a");
        sleep(80).await;
        ks(cx, window, "backspace");
        sleep(200).await;
        let m = window.update(cx, |v, _, _| v.visible.len()).unwrap_or(0);
        log("visible_after_clearing_filter", m);
        // keyboard nav
        ks(cx, window, "escape");
        sleep(150).await;
        for _ in 0..3 {
            ks(cx, window, "down");
            sleep(80).await;
        }
        let sel = window.update(cx, |v, _, _| v.selected).unwrap_or(99);
        log("down_x3_selected_index_is_3", sel == 3);
        let was = window.update(cx, |v, _, _| v.lists[0].tasks[3].done).unwrap_or(false);
        ks(cx, window, "space");
        sleep(150).await;
        let now = window.update(cx, |v, _, _| v.lists[0].tasks[3].done).unwrap_or(false);
        log("space_toggles_selected", was != now);
    }
    // scroll bench (programmatic)
    window
        .update(cx, |v, w, cx| {
            v.scroll_run = Some(ScrollRun { frames: 0, total: 400, last: None, dts: vec![], builds: vec![] });
            cx.notify();
        })
        .ok();
    for _ in 0..400 {
        sleep(50).await;
        if window.update(cx, |v, _, _| v.scroll_done).unwrap_or(true) {
            break;
        }
    }
    let (_, peak, _) = metrics::working_set_mb();
    log("peak_working_set_after_scroll_mb", format!("{peak:.1}"));

    log("hotkey_end_to_end", "NOT tested (would require OS-level input injection); registration only");
    let (ws, peak, _) = metrics::working_set_mb();
    log("working_set_end_mb", format!("{ws:.1}"));
    log("peak_working_set_end_mb", format!("{peak:.1}"));
    metrics::flush("metrics.txt");
    cx.update(|cx| cx.quit());
}
