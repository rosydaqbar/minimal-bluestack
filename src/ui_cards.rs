// Card layout shares the existing actions and backend in ui.rs.
impl Toolbox {
    fn card() -> Div {
        div()
            .p_5()
            .rounded_lg()
            .border_1()
            .border_color(rgb(LINE))
            .bg(rgb(PANEL))
            .flex()
            .flex_col()
            .gap_3()
    }
    fn field(label: &str, value: &str) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .child(value.replace('\\', "\\\u{200b}")),
            )
            .into_any_element()
    }
    fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = &self.snapshot.status;
        let running = host.running;
        let ready = host.ready;
        let session = Self::card()
            .flex_1()
            .min_w(px(0.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child("Android session")
                    .child(Self::tag(
                        if running { "Running" } else { "Offline" },
                        if running { ACCENT } else { MUTED },
                    )),
            )
            .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                if host.launcher_visible {
                    "BlueStacks launcher is visible."
                } else if running {
                    "Android is running in the background."
                } else {
                    "Connect Android to scan installed apps."
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.button(
                        cx,
                        "connect",
                        if running { "Scan apps" } else { "Connect" },
                        true,
                        !self.busy && ready,
                        |this, _, cx| this.run(Job::Connect, cx),
                    ))
                    .child(self.button(
                        cx,
                        "stop",
                        "Stop idle",
                        false,
                        !self.busy && running,
                        |this, _, cx| this.run(Job::Stop, cx),
                    )),
            );
        let core = Self::card()
            .flex_1()
            .min_w(px(0.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child("Native core")
                    .child(Self::tag(
                        if ready { "Ready" } else { "Setup needed" },
                        if ready { ACCENT } else { 0xe8c987 },
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(if ready {
                        "Core files verified."
                    } else {
                        "Check the installation and core files."
                    }),
            )
            .child(self.button(
                cx,
                "view-installation",
                "View setup",
                false,
                true,
                |this, _, cx| {
                    this.page = Page::Installation;
                    cx.notify();
                },
            ));
        let registered = self
            .snapshot
            .apps
            .iter()
            .filter(|app| app.registered)
            .count();
        let library = Self::card()
            .flex_1()
            .min_w(px(0.))
            .child("App library")
            .child(
                div()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!(
                        "{} apps · {} registered",
                        self.snapshot.apps.len(),
                        registered
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "Source: {}",
                        if self.snapshot.source.is_empty() {
                            "Reading local data"
                        } else {
                            &self.snapshot.source
                        }
                    )),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("One instance · up to two game windows"),
            );
        div()
            .flex()
            .gap_4()
            .child(session)
            .child(core)
            .child(library)
            .into_any_element()
    }
    fn installation(&self, cx: &mut Context<Self>) -> AnyElement {
        let host = &self.snapshot.status;
        let backend = self.backend.clone();
        let program = host.program.clone();
        let paths = Self::card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("BlueStacks folders"),
                    )
                    .child(Self::tag(
                        if host.installed {
                            "Detected"
                        } else {
                            "Not found"
                        },
                        if host.installed { ACCENT } else { 0xe8c987 },
                    )),
            )
            .child(Self::field(
                "Program folder",
                if host.program.is_empty() {
                    "Select the folder that contains HD-Player.exe."
                } else {
                    &host.program
                },
            ))
            .child(div().h(px(1.)).bg(rgb(LINE)))
            .child(Self::field(
                "Android data folder",
                if host.data_path.is_empty() {
                    "Select the folder that contains bluestacks.conf."
                } else {
                    &host.data_path
                },
            ))
            .child(Self::field(
                "Detection method",
                if host.detection_source.is_empty() {
                    "Not detected"
                } else {
                    &host.detection_source
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.button(
                        cx,
                        "browse-installation",
                        "Program folder…",
                        false,
                        !self.busy && !host.running,
                        |this, _, cx| this.browse_installation(false, cx),
                    ))
                    .child(self.button(
                        cx,
                        "browse-data",
                        "Data folder…",
                        false,
                        !self.busy && !host.running && host.installed,
                        |this, _, cx| this.browse_installation(true, cx),
                    ))
                    .child(self.button(
                        cx,
                        "detect-installation",
                        "Auto-detect",
                        false,
                        !self.busy && !host.running,
                        |this, _, cx| this.run(Job::AutoDetect, cx),
                    ))
                    .child(self.button(
                        cx,
                        "open-installation",
                        "Open folder",
                        false,
                        host.installed,
                        move |_, _, _| backend.open_install_folder(&program),
                    )),
            )
            .when(host.running, |d| {
                d.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(0xe8c987))
                        .child("Stop Android before changing folders."),
                )
            });
        let core = Self::card()
            .child(
                div()
                    .text_size(px(17.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Core setup"),
            )
            .child(Self::field(
                "BlueStacks version",
                if host.version.is_empty() {
                    "Not available"
                } else {
                    &host.version
                },
            ))
            .child(Self::field("Android instance", &host.instance))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(if host.ready { ACCENT } else { 0xe8c987 }))
                    .child(if host.ready {
                        "Core files are verified and ready.".to_owned()
                    } else {
                        host.problem.clone().unwrap_or_else(|| {
                            "Select your BlueStacks folders, then install the core.".into()
                        })
                    }),
            )
            .child(self.button(
                cx,
                "setup",
                "Install core · UAC",
                !host.ready,
                !self.busy && !host.running && host.installed,
                |this, _, cx| this.run(Job::SetupCore, cx),
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(if host.running {
                        "Stop Android before installing or restoring the core."
                    } else {
                        "Setup saves a backup and asks for Windows administrator approval."
                    }),
            );
        div()
            .id("installation-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(div().flex().flex_col().gap_4().child(paths).child(core))
            .into_any_element()
    }
}
impl Render for Toolbox {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.page {
            Page::Library => "Apps & games",
            Page::Installation => "Installation",
            Page::Cleanup => "Cleanup",
            Page::Activity => "Activity",
        };
        let content = match self.page {
            Page::Library => self.library(cx),
            Page::Installation => self.installation(cx),
            Page::Cleanup => self.cleanup(cx),
            Page::Activity => div()
                .id("history-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when(self.history.is_empty(), |d| {
                            d.child(Self::card().child("No actions yet"))
                        })
                        .children(self.history.iter().enumerate().map(|(index, entry)| {
                            Self::card()
                                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                                    if index == 0 {
                                        "Latest action"
                                    } else {
                                        "Previous action"
                                    },
                                ))
                                .child(div().text_size(px(13.)).child(entry.clone()))
                        })),
                )
                .into_any_element(),
        };
        let overview = self.overview(cx);
        let launcher_visible = self.snapshot.status.launcher_visible;
        let launcher = self.button(
            cx,
            "open-bluestacks",
            if launcher_visible {
                "Hide BlueStacks"
            } else {
                "Open BlueStacks"
            },
            false,
            !self.busy && self.snapshot.status.installed,
            move |this, _, cx| {
                this.run(
                    if launcher_visible {
                        Job::HideLauncher
                    } else {
                        Job::OpenLauncher
                    },
                    cx,
                )
            },
        );
        let refresh = self.button(cx, "scan", "Refresh", false, !self.busy, |this, _, cx| {
            this.run(Job::Scan, cx)
        });
        let folder = self.backend.clone();
        div()
            .size_full()
            .flex()
            .font_family("Segoe UI")
            .text_color(rgb(TEXT))
            .bg(rgb(BG))
            .child(
                div()
                    .w(px(184.))
                    .flex_none()
                    .p_4()
                    .border_r_1()
                    .border_color(rgb(LINE))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .px_3()
                            .pt_3()
                            .pb_6()
                            .child(
                                div()
                                    .text_size(px(26.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("minimal"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(ACCENT))
                                    .child("BlueStack"),
                            ),
                    )
                    .child(self.nav(cx, "Apps & games", 0, Page::Library))
                    .child(self.nav(cx, "Installation", 1, Page::Installation))
                    .child(self.nav(cx, "Cleanup", 2, Page::Cleanup))
                    .child(self.nav(cx, "Activity", 3, Page::Activity))
                    .child(div().flex_1())
                    .child(self.button(
                        cx,
                        "open-folder",
                        "Runtime folder",
                        false,
                        true,
                        move |_, _, _| folder.open_folder(),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_6()
                            .py_5()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap_4()
                            .child(
                                div().min_w(px(0.)).child(
                                    div()
                                        .text_size(px(24.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(title),
                                ),
                            )
                            .child(div().flex().gap_2().child(launcher).child(refresh)),
                    )
                    .child(div().px_6().pb_5().child(overview))
                    .child(div().px_6().pb_5().flex_1().min_h(px(0.)).child(content))
                    .child(
                        div()
                            .id("status-scroll")
                            .px_6()
                            .py_3()
                            .max_h(px(96.))
                            .overflow_y_scroll()
                            .border_t_1()
                            .border_color(rgb(LINE))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(if self.last_error { 0xf2a29c } else { MUTED }))
                                    .child(if self.busy {
                                        "In progress"
                                    } else if self.last_error {
                                        "Action failed"
                                    } else {
                                        "Latest result"
                                    }),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(12.))
                                    .text_color(rgb(if self.busy { ACCENT } else { TEXT }))
                                    .child(self.status.clone()),
                            ),
                    ),
            )
    }
}
