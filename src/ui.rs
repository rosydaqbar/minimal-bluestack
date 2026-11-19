use crate::backend::{
    AndroidApp, Backend, BloatwareStatus, CleanupPlan, Job, Outcome, Snapshot, icon_exists,
};
use gpui::{prelude::*, *};
use std::path::PathBuf;

const BG: u32 = 0x101316;
const PANEL: u32 = 0x171c20;
const LINE: u32 = 0x2a3238;
const TEXT: u32 = 0xe9eeeb;
const MUTED: u32 = 0x8f9e9a;
const ACCENT: u32 = 0xb9e5c8;
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Library,
    Installation,
    Cleanup,
    Activity,
}
pub struct Toolbox {
    backend: Backend,
    snapshot: Snapshot,
    selected: Option<String>,
    page: Page,
    busy: bool,
    status: String,
    last_error: bool,
    history: Vec<String>,
    plan: Option<CleanupPlan>,
    search: String,
    search_focus: FocusHandle,
    show_system: bool,
    registered_only: bool,
    confirm_restore: bool,
}
impl Toolbox {
    fn browse_installation(&mut self, data: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                if data {
                    "Select the folder containing bluestacks.conf"
                } else {
                    "Select the folder containing HD-Player.exe"
                }
                .into(),
            ),
        });
        self.busy = true;
        self.status = "Choose a folder…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = selected.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(Some(paths))) if !paths.is_empty() => {
                        let path = paths[0].clone();
                        let job = if data {
                            Job::SelectData(path, this.snapshot.status.program.clone())
                        } else {
                            Job::SelectInstallation(path)
                        };
                        this.run(job, cx);
                    }
                    Ok(Ok(_)) => this.status = "Folder selection cancelled.".into(),
                    Ok(Err(error)) => {
                        this.status = format!("Could not open folder picker: {error}")
                    }
                    Err(_) => this.status = "Folder selection cancelled.".into(),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub fn new(backend: Backend, cx: &mut Context<Self>) -> Self {
        let mut state = Self {
            backend,
            snapshot: Snapshot::default(),
            selected: None,
            page: Page::Library,
            busy: false,
            status: "Reading local installation…".into(),
            last_error: false,
            history: vec![],
            plan: None,
            search: String::new(),
            search_focus: cx.focus_handle(),
            show_system: false,
            registered_only: false,
            confirm_restore: false,
        };
        state.run(Job::Scan, cx);
        state
    }
    fn run(&mut self, job: Job, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.last_error = false;
        if matches!(
            &job,
            Job::LoadBloatware | Job::Apply(_) | Job::CleanPackage(_, _) | Job::Undo
        ) {
            self.plan = None;
        }
        let installation_changed = matches!(
            &job,
            Job::SelectInstallation(_) | Job::SelectData(_, _) | Job::AutoDetect
        );
        self.status = if matches!(&job, Job::LoadBloatware) {
            "Loading bloatware from Android…".into()
        } else {
            "Working…".into()
        };
        cx.notify();
        let backend = self.backend.clone();
        let work = cx
            .background_executor()
            .spawn(async move { backend.execute(job) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Outcome {
                        snapshot,
                        plan,
                        message,
                    }) => {
                        if installation_changed {
                            this.plan = None;
                        }
                        if let Some(snapshot) = snapshot {
                            this.snapshot = snapshot;
                        }
                        if let Some(plan) = plan {
                            this.plan = Some(plan);
                        }
                        this.status = message;
                        if this.selected.is_none() {
                            this.selected =
                                this.snapshot
                                    .apps
                                    .iter()
                                    .find(|a| a.registered && a.enabled && !a.activity.is_empty())
                                    .or_else(|| {
                                        this.snapshot.apps.iter().find(|a| {
                                            !a.system && a.enabled && !a.activity.is_empty()
                                        })
                                    })
                                    .map(|a| a.package.clone());
                        }
                    }
                    Err(error) => {
                        this.last_error = true;
                        this.status = format!("{}", error);
                    }
                }
                this.history.insert(0, this.status.clone());
                this.history.truncate(60);
                cx.notify();
            });
        })
        .detach();
    }
    fn button(
        &self,
        cx: &mut Context<Self>,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        primary: bool,
        enabled: bool,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_4()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if primary { ACCENT } else { LINE }))
            .bg(rgb(if primary { ACCENT } else { PANEL }))
            .text_color(rgb(if primary { BG } else { TEXT }))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .opacity(if enabled { 1. } else { 0.35 })
            .flex_shrink_0()
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|style| style.bg(rgb(if primary { 0xcdf2d8 } else { 0x26312d })))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    action(this, window, cx)
                }
            }))
            .child(label.into())
            .into_any_element()
    }
    fn tag(label: &str, color: u32) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(LINE))
            .text_size(px(10.))
            .text_color(rgb(color))
            .child(label.to_owned())
            .into_any_element()
    }
    fn app_icon(app: &AndroidApp, size: f32) -> AnyElement {
        if icon_exists(app) {
            img(PathBuf::from(&app.icon))
                .w(px(size))
                .h(px(size))
                .flex_shrink_0()
                .rounded_lg()
                .into_any_element()
        } else {
            div()
                .w(px(size))
                .h(px(size))
                .rounded_lg()
                .bg(rgb(0x29382e))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(ACCENT))
                .text_size(px(size * 0.4))
                .child(app.name.chars().next().unwrap_or('A').to_string())
                .into_any_element()
        }
    }
    fn nav(&self, cx: &mut Context<Self>, label: &str, index: usize, page: Page) -> AnyElement {
        let selected = self.page == page;
        div()
            .id(("nav", index))
            .px_4()
            .py_3()
            .rounded_md()
            .bg(rgb(if selected { 0x26352d } else { BG }))
            .text_color(rgb(if selected { ACCENT } else { MUTED }))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                cx.notify();
            }))
            .child(label.to_owned())
            .into_any_element()
    }
    fn library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search.to_lowercase();
        let rows: Vec<_> = self
            .snapshot
            .apps
            .iter()
            .filter(|a| self.show_system || !a.system)
            .filter(|a| self.show_system || !a.activity.is_empty())
            .filter(|a| !self.registered_only || a.registered)
            .filter(|a| {
                query.is_empty()
                    || a.name.to_lowercase().contains(&query)
                    || a.package.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        let selected = self
            .snapshot
            .apps
            .iter()
            .find(|a| Some(&a.package) == self.selected.as_ref())
            .cloned();
        let mut cards = Vec::new();
        for app in &rows {
            let package = app.package.clone();
            let selected = Some(&app.package) == self.selected.as_ref();
            cards.push(
                div()
                    .id(SharedString::from(app.package.clone()))
                    .w(px(220.))
                    .flex_grow()
                    .min_w(px(0.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if selected { 0x719d7f } else { LINE }))
                    .bg(rgb(if selected { 0x203027 } else { PANEL }))
                    .hover(|style| style.bg(rgb(0x25302b)))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(package.clone());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(Self::app_icon(app, 52.))
                            .child(Self::tag(
                                if !app.enabled {
                                    "Disabled"
                                } else if app.registered {
                                    "Registered"
                                } else if app.system {
                                    "System app"
                                } else {
                                    "App"
                                },
                                if app.enabled { ACCENT } else { MUTED },
                            )),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .w_full()
                            .whitespace_normal()
                            .child(app.name.clone()),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .w_full()
                            .whitespace_normal()
                            .child(app.package.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(if selected { ACCENT } else { MUTED }))
                            .child(if selected {
                                "Selected · details on the right"
                            } else {
                                "Select to view actions"
                            }),
                    )
                    .into_any_element(),
            );
        }
        let search = div()
            .id("search")
            .track_focus(&self.search_focus)
            .px_4()
            .py_3()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(LINE))
            .rounded_md()
            .cursor_text()
            .on_click(cx.listener(|this, _, window, _| {
                this.search_focus.focus(window);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "backspace" {
                    this.search.pop();
                } else if event.keystroke.key == "escape" {
                    this.search.clear();
                } else if !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt {
                    if let Some(text) = &event.keystroke.key_char {
                        this.search.push_str(text);
                    }
                }
                cx.notify();
            }))
            .text_size(px(13.))
            .text_color(rgb(if self.search.is_empty() { MUTED } else { TEXT }))
            .child(if self.search.is_empty() {
                "Search by name or package…".to_owned()
            } else {
                self.search.clone()
            });
        let filter = self.button(
            cx,
            "filter-system",
            if self.show_system {
                "System apps: shown"
            } else {
                "System apps: hidden"
            },
            false,
            true,
            |this, _, cx| {
                this.show_system = !this.show_system;
                cx.notify();
            },
        );
        let shortcut_filter = self.button(
            cx,
            "filter-registered",
            if self.registered_only {
                "Registered apps"
            } else {
                "All apps"
            },
            false,
            true,
            |this, _, cx| {
                this.registered_only = !this.registered_only;
                cx.notify();
            },
        );
        let list = div()
            .flex_1()
            .min_w(px(300.))
            .flex()
            .flex_col()
            .gap_3()
            .child(search)
            .child(div().flex().gap_2().child(filter).child(shortcut_filter))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} apps · {}",
                        rows.len(),
                        self.snapshot.source.clone()
                    )),
            )
            .child(
                div()
                    .id("apps-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap_3()
                    .children(cards)
                    .when(rows.is_empty(), |d| {
                        d.child(div().p_6().text_color(rgb(MUTED)).child(
                            "No apps match this filter. Connect Android to scan all installed apps.",
                        ))
                    }),
            );
        let details = if let Some(app) = selected {
            let shortcut = app.clone();
            let launch = app.clone();
            let allowed = !self.busy && !app.activity.is_empty() && app.enabled;
            div()
                    .id("app-details-scroll")
                    .overflow_y_scroll()
                    .w(px(276.))
                    .flex_none()
                    .p_6()
                    .bg(rgb(PANEL))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(LINE))
                    .child(div().flex().flex_col().gap_4()
                    .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("Selected app"))
                    .child(Self::app_icon(&app, 72.))
                    .child(
                        div()
                            .text_size(px(23.))
                            .font_weight(FontWeight::BOLD)
                            .child(app.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(app.package.clone()),
                    )
                    .child(div().h(px(1.)).bg(rgb(LINE)))
                    .when(!allowed || !self.snapshot.status.ready, |d| d.child(div().text_size(px(12.)).text_color(rgb(0xe8c987)).child(
                        if !app.enabled { "This app is disabled in Android." }
                        else if app.activity.is_empty() { "This app has no launcher activity." }
                        else { "Open Installation and install the core to use game windows." })))
                    .child(self.button(
                        cx,
                        "make-shortcut",
                        "Create shortcut",
                        false,
                        allowed,
                        move |this, _, cx| this.run(Job::Register(shortcut.clone()), cx),
                    ))
                    .child(self.button(
                        cx,
                        "launch-game",
                        "Open window",
                        true,
                        allowed && self.snapshot.status.ready,
                        move |this, _, cx| this.run(Job::Launch(launch.clone()), cx),
                    ))
                    .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                        "Desktop and window icons match the app. Each shortcut opens one game.",
                    ))
                    .child(
                        div()
                            .mt_4()
                            .text_size(px(10.))
                            .text_color(rgb(ACCENT))
                            .child("Individual desktop window"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("Android stops after the last game closes while the BlueStacks launcher is hidden."),
                    ))
        } else {
            div()
                .w(px(276.))
                .p_6()
                .bg(rgb(PANEL))
                .rounded_lg()
                .text_color(rgb(MUTED))
                .child("Select an app to view its details.")
                .id("app-details-empty")
        };
        div()
            .flex()
            .gap_6()
            .size_full()
            .child(list)
            .child(details)
            .into_any_element()
    }
    fn cleanup(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let load = self.button(
            cx,
            "load-bloatware",
            "Load bloatware",
            true,
            !self.busy,
            |this, _, cx| this.run(Job::LoadBloatware, cx),
        );
        let undo = self.button(
            cx,
            "restore-cleanup",
            "Undo last cleanup",
            false,
            !self.busy,
            |this, _, cx| this.run(Job::Undo, cx),
        );
        let mut body=div().flex().flex_col().gap_5().child(div().text_size(px(28.)).font_weight(FontWeight::BOLD).child("Bloatware"))
            .child(div().text_color(rgb(MUTED)).text_size(px(14.)).child("Load optional packages and ad components. Read each reason before cleanup. Undo restores recorded changes."))
            .child(div().flex().gap_3().child(load).child(undo))
            .child(div().p_5().bg(rgb(PANEL)).rounded_lg().border_1().border_color(rgb(LINE)).child("Game data, Play Store, Google sign-in, and WebView are preserved."));
        if let Some(plan) = self.plan.clone() {
            let pending = plan
                .items
                .iter()
                .filter(|item| item.status == BloatwareStatus::Pending)
                .count();
            let cleared = plan
                .items
                .iter()
                .filter(|item| item.status == BloatwareStatus::Cleared)
                .count();
            let blocked = plan.items.len() - pending - cleared;
            let mut rows = Vec::new();
            let mut protected_rows = Vec::new();
            for item in &plan.items {
                let (label, color) = match item.status {
                    BloatwareStatus::Cleared => ("✓ Cleared", ACCENT),
                    BloatwareStatus::Pending => ("– Pending", 0xe8c987),
                    BloatwareStatus::Blocked if item.package.contains('/') => {
                        ("× Needs root", 0xf2a29c)
                    }
                    BloatwareStatus::Blocked => ("× Protected", 0xf2a29c),
                    BloatwareStatus::Failed => ("× Failed", 0xf2a29c),
                };
                let mut row = div()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(LINE))
                    .flex()
                    .gap_4()
                    .items_start()
                    .child(
                        div()
                            .w(px(96.))
                            .flex_shrink_0()
                            .child(Self::tag(label, color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(item.name.clone()),
                            )
                            .when(item.name != item.package, |details| {
                                details.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(MUTED))
                                        .child(item.package.clone()),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(item.reason.clone()),
                            ),
                    );
                if item.can_clean {
                    let id = plan.id.clone();
                    let package = item.package.clone();
                    row = row.child(self.button(
                        cx,
                        SharedString::from(format!("clean-{}", item.package)),
                        if item.status == BloatwareStatus::Failed {
                            "Retry"
                        } else {
                            "Clean"
                        },
                        false,
                        !self.busy,
                        move |this, _, cx| {
                            this.run(Job::CleanPackage(id.clone(), package.clone()), cx)
                        },
                    ));
                }
                if item.status == BloatwareStatus::Blocked {
                    protected_rows.push(row);
                } else {
                    rows.push(row);
                }
            }
            let id = plan.id;
            body = body
                .child(div().text_size(px(13.)).child(format!("✓ {cleared} cleared   ·   – {pending} pending   ·   × {blocked} protected / failed")))
                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("Cleared means disabled or removed for Android user 0. App data is retained. Undo restores recorded changes."))
                .child(div().flex().flex_col().gap_3().children(rows))
                .child(self.button(
                    cx,
                    "apply-cleanup",
                    format!("Clean pending ({pending})"),
                    true,
                    !self.busy && pending > 0,
                    move |this, _, cx| this.run(Job::Apply(id.clone()), cx),
                ));
            if !protected_rows.is_empty() {
                body = body.child(div().mt_4().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Protected / unavailable components"))
                    .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("These entries explain dependencies that are kept. Shared packages can still have their optional ad components cleaned individually above."))
                    .child(div().flex().flex_col().gap_3().children(protected_rows));
            }
        } else {
            body = body.child(div().p_5().text_size(px(13.)).text_color(rgb(MUTED)).child("Load bloatware to see ✓ cleared, – pending, and × protected or failed entries with reasons. Android will start in the background if needed."));
        }
        let restore = self.button(
            cx,
            "restore-core",
            if self.confirm_restore {
                "Confirm core restore · UAC"
            } else {
                "Restore core · UAC"
            },
            false,
            !self.busy && !self.snapshot.status.running,
            |this, _, cx| {
                if this.confirm_restore {
                    this.confirm_restore = false;
                    this.run(Job::RestoreCore, cx);
                } else {
                    this.confirm_restore = true;
                    cx.notify();
                }
            },
        );
        body=body.child(div().mt_6().p_5().rounded_lg().border_1().border_color(rgb(LINE)).flex().flex_col().gap_3()
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Native core recovery"))
            .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child("Restore the BlueStacks files backed up before core setup. Close all emulators first. Reinstall the core to use separate windows again after restoring."))
            .child(restore));
        div()
            .id("cleanup-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }
}
include!("ui_cards.rs");
