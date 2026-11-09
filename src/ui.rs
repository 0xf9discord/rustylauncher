use crate::{
    auth::{self, Session},
    launcher::{self, Event, Version},
    loaders::{self, LoaderKind, LoaderVersion},
    settings::{self, Settings},
};
use gpui::{div, prelude::*, px, rgb, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const BG: u32 = 0x121212;
const PANEL: u32 = 0x181818;
const LINE: u32 = 0x2a2a2a;
const TEXT: u32 = 0xffffff;
const MUTED: u32 = 0xb3b3b3;
const GREEN: u32 = 0x1ed760;
const ERROR: u32 = 0xf3727f;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Play,
    Versions,
    Settings,
    Logs,
}

#[derive(Clone)]
enum Action {
    Navigate(Page),
    Select(String),
    Loader(LoaderKind),
    LoaderVersion(String),
    Mods,
    InstallerLog,
    Refresh,
    Snapshots,
    Install,
    Play,
    Login,
    Logout,
    Memory(u32),
    EditSettings,
    ReloadSettings,
    Folder,
    Log,
    Cancel,
    CopyCode,
    Browser,
}

struct Launcher {
    logo: Arc<Image>,
    settings: Settings,
    saved_settings: Settings,
    settings_error: Option<String>,
    page: Page,
    versions: Vec<Version>,
    loader_choices: Vec<LoaderVersion>,
    session: Option<Session>,
    tx: async_channel::Sender<Event>,
    cancel: Arc<AtomicBool>,
    busy: bool,
    cancellable: bool,
    running: bool,
    progress: f32,
    status: String,
    error: bool,
    device: Option<(String, String)>,
    logs: Vec<String>,
    focus: FocusHandle,
}

fn label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text.into())
}

impl Launcher {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this: &mut Self, cx| {
                        this.receive(event);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let loaded = Settings::load();
        let (settings, settings_error) = match loaded {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(format!("설정 오류: {e:#}"))),
        };
        let initial = settings_error
            .clone()
            .unwrap_or("버전 목록을 불러오는 중".to_owned());
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut this = Self {
            logo: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../assets/logo.png").to_vec(),
            )),
            saved_settings: settings.clone(),
            settings,
            settings_error: settings_error.clone(),
            page: Page::Play,
            versions: vec![],
            loader_choices: vec![],
            session: None,
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            cancellable: false,
            running: false,
            progress: 0.,
            status: initial,
            error: settings_error.is_some(),
            device: None,
            logs: settings_error.into_iter().collect(),
            focus,
        };
        this.refresh();
        this
    }

    fn status(&mut self, text: String, error: bool) {
        self.status = text.clone();
        self.error = error;
        self.logs.push(text);
        if self.logs.len() > 120 {
            self.logs.remove(0);
        }
    }

    fn receive(&mut self, event: Event) {
        match event {
            Event::Loaders(game, kind, choices) => {
                if game != self.settings.selected_version || kind != self.settings.loader {
                    return;
                }
                self.busy = false;
                if !choices.iter().any(|v| v.id == self.settings.loader_version) {
                    self.settings.loader_version =
                        choices.first().map(|v| v.id.clone()).unwrap_or_default();
                }
                self.loader_choices = choices;
                self.save();
                if self.settings_error.is_some() {
                    return;
                }
                if self.loader_choices.is_empty() {
                    self.status(format!("{game}용 {} 배포 버전이 없습니다. 다른 게임 버전이나 로더를 선택하세요.",kind.name()),true);
                } else {
                    self.status(
                        format!(
                            "{} 호환 로더 {}개를 불러왔습니다",
                            kind.name(),
                            self.loader_choices.len()
                        ),
                        false,
                    );
                }
            }
            Event::Versions(versions, message) => {
                self.versions = versions;
                if !self
                    .versions
                    .iter()
                    .any(|v| v.id == self.settings.selected_version)
                {
                    self.settings.selected_version = self
                        .versions
                        .iter()
                        .find(|v| v.kind == "release")
                        .map(|v| v.id.clone())
                        .unwrap_or_default();
                }
                self.busy = false;
                if let Some(error) = self.settings_error.clone() {
                    self.status(error, true);
                } else {
                    self.save();
                    if self.settings_error.is_none() {
                        self.status(message, false);
                    }
                }
                if self.settings_error.is_none() && self.settings.loader != LoaderKind::Vanilla {
                    self.load_loader_versions();
                }
            }
            Event::Progress(message, p) => {
                self.progress = p;
                self.status = message;
            }
            Event::Installed(id) => {
                self.busy = false;
                self.progress = 1.;
                self.status(
                    format!("{id} 설치 완료 · 플레이할 준비가 되었습니다"),
                    false,
                );
            }
            Event::SignedIn(session) => {
                self.status(format!("{}님, 환영합니다", session.name), false);
                self.session = Some(session);
                self.device = None;
                self.busy = false;
            }
            Event::DeviceCode(code, url) => {
                self.device = Some((code, url));
                self.status(
                    "브라우저에서 코드를 입력해 로그인을 완료하세요".to_owned(),
                    false,
                );
            }
            Event::Started => {
                self.busy = false;
                self.running = true;
                self.status("Minecraft 실행 중".to_owned(), false);
            }
            Event::Exited(message) => {
                self.running = false;
                self.status(message, false);
            }
            Event::Error(message) => {
                self.busy = false;
                self.running = false;
                self.device = None;
                self.status(message, true);
            }
        }
    }

    fn save(&mut self) {
        match self.settings.save_changes(&self.saved_settings) {
            Ok(merged) => {
                self.saved_settings = merged.clone();
                self.settings = merged;
                self.settings_error = None;
            }
            Err(e) => {
                let message = format!("설정 저장 실패: {e:#}");
                self.settings_error = Some(message.clone());
                self.status(message, true);
            }
        }
    }

    fn profile_installed(&self, game: &str) -> bool {
        loaders::is_installed(game, self.settings.loader, &self.settings.loader_version)
    }

    fn load_loader_versions(&mut self) {
        self.loader_choices.clear();
        if self.settings.loader == LoaderKind::Vanilla || self.settings.selected_version.is_empty()
        {
            return;
        }
        let game = self.settings.selected_version.clone();
        let kind = self.settings.loader;
        self.work(
            "호환되는 로더 버전을 불러오는 중",
            false,
            move |tx, _| {
                let choices = loaders::versions(&game, kind)?;
                tx.send_blocking(Event::Loaders(game, kind, choices))?;
                Ok(())
            },
        );
    }

    fn refresh(&mut self) {
        if self.busy || self.running {
            return;
        }
        self.busy = true;
        self.cancellable = false;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let event = match launcher::versions() {
                Ok((v, m)) => Event::Versions(v, m),
                Err(e) => Event::Error(format!("{e:#}")),
            };
            tx.send_blocking(event).ok();
        });
    }

    fn work(
        &mut self,
        message: &str,
        cancellable: bool,
        f: impl FnOnce(async_channel::Sender<Event>, Arc<AtomicBool>) -> anyhow::Result<()>
        + Send
        + 'static,
    ) {
        self.busy = true;
        self.cancellable = cancellable;
        self.progress = 0.;
        self.device = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.status(message.to_owned(), false);
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            if let Err(e) = f(tx.clone(), cancel) {
                tx.send_blocking(Event::Error(format!("{e:#}"))).ok();
            }
        });
    }

    fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Navigate(page) => self.page = page,
            Action::Cancel => {
                self.cancel.store(true, Ordering::Relaxed);
                self.status(
                    "취소 요청 중 · 진행 중인 연결이 끝나면 중지합니다".to_owned(),
                    false,
                );
            }
            Action::CopyCode => {
                if let Some((code, _)) = &self.device {
                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
                }
            }
            Action::Browser => {
                if let Some((_, url)) = &self.device {
                    cx.open_url(url);
                }
            }
            Action::Folder => {
                let result = loaders::instance_dir(&self.settings).and_then(|path| {
                    std::fs::create_dir_all(&path)?;
                    settings::open_path(path)
                });
                if let Err(e) = result {
                    self.status(e.to_string(), true);
                }
            }
            Action::Mods => {
                let result = loaders::instance_dir(&self.settings).and_then(|p| {
                    let mods = p.join("mods");
                    std::fs::create_dir_all(&mods)?;
                    settings::open_path(mods)
                });
                if let Err(e) = result {
                    self.status(e.to_string(), true);
                }
            }
            Action::InstallerLog => {
                let path = settings::data_dir().join("installer.log");
                if path.exists() {
                    if let Err(e) = settings::open_path(path) {
                        self.status(e.to_string(), true);
                    }
                } else {
                    self.status(
                        "설치 로그가 없습니다. Forge 또는 NeoForge 설치 후 확인하세요.".to_owned(),
                        false,
                    );
                }
            }
            Action::Log => {
                let path = settings::data_dir().join("game.log");
                if path.exists() {
                    if let Err(e) = settings::open_path(path) {
                        self.status(e.to_string(), true);
                    }
                } else {
                    self.status(
                        "아직 게임 로그가 없습니다. 게임을 실행한 뒤 확인하세요.".to_owned(),
                        false,
                    );
                }
            }
            Action::EditSettings => {
                if let Err(e) = settings::edit_settings() {
                    self.status(e.to_string(), true);
                }
            }
            _ if self.busy || self.running => {}
            Action::Select(id) => {
                self.settings.selected_version = id;
                self.save();
                self.load_loader_versions();
            }
            Action::Loader(kind) => {
                self.settings.loader = kind;
                self.settings.loader_version.clear();
                self.save();
                self.load_loader_versions();
            }
            Action::LoaderVersion(version) => {
                if self.loader_choices.iter().any(|v| v.id == version) {
                    self.settings.loader_version = version;
                    self.save();
                }
            }
            Action::Refresh => self.refresh(),
            Action::Snapshots => {
                self.settings.show_snapshots = !self.settings.show_snapshots;
                self.save();
            }
            Action::Memory(memory) => {
                self.settings.memory_mb = memory;
                self.save();
            }
            Action::ReloadSettings => match Settings::load() {
                Ok(s) => {
                    self.saved_settings = s.clone();
                    self.settings = s;
                    self.settings_error = None;
                    self.status("설정을 다시 불러왔습니다".to_owned(), false);
                    self.load_loader_versions();
                }
                Err(e) => self.status(format!("{e:#}"), true),
            },
            Action::Logout => {
                self.session = None;
                self.status("로그아웃했습니다".to_owned(), false);
            }
            Action::Login => {
                // Pick up app IDs edited while this window was open.
                let id = Settings::load().and_then(|latest| {
                    self.settings.microsoft_client_id = latest.microsoft_client_id.clone();
                    self.saved_settings.microsoft_client_id = latest.microsoft_client_id;
                    auth::validate_client_id(&self.settings.microsoft_client_id)
                });
                match id {
                    Err(e) => {
                        self.page = Page::Settings;
                        self.status(format!("{e:#}"), true);
                    }
                    Ok(id) => {
                        self.work(
                            "Microsoft 로그인 코드를 요청하는 중",
                            true,
                            move |tx, cancel| {
                                let session = auth::login(&id, &tx, cancel)?;
                                tx.send_blocking(Event::SignedIn(session))?;
                                Ok(())
                            },
                        );
                    }
                }
            }
            Action::Install => {
                if let Some(v) = self
                    .versions
                    .iter()
                    .find(|v| v.id == self.settings.selected_version)
                    .cloned()
                {
                    let selected = self.settings.clone();
                    if selected.loader != LoaderKind::Vanilla
                        && !self
                            .loader_choices
                            .iter()
                            .any(|v| v.id == selected.loader_version)
                    {
                        self.status("호환되는 로더 버전을 먼저 선택하세요".to_owned(), true);
                        return;
                    }
                    self.work("설치를 준비하는 중", true, move |tx, cancel| {
                        loaders::install(
                            &v,
                            selected.loader,
                            &selected.loader_version,
                            &selected,
                            &tx,
                            cancel,
                        )?;
                        tx.send_blocking(Event::Installed(format!(
                            "{} {} {}",
                            v.id,
                            selected.loader.name(),
                            selected.loader_version
                        )))?;
                        Ok(())
                    });
                } else {
                    self.status("설치할 버전을 먼저 선택하세요".to_owned(), true);
                }
            }
            Action::Play => {
                if !self.profile_installed(&self.settings.selected_version) {
                    self.act(Action::Install, cx);
                } else if self
                    .session
                    .as_ref()
                    .is_none_or(|s| s.expires <= std::time::Instant::now())
                {
                    self.act(Action::Login, cx);
                } else if let Some(session) = self.session.clone() {
                    let settings = self.settings.clone();
                    let id = loaders::profile_id(
                        &settings.selected_version,
                        settings.loader,
                        &settings.loader_version,
                    );
                    self.work(
                        "Java를 확인하고 게임을 시작하는 중",
                        false,
                        move |tx, _| launcher::launch(&id, &settings, &session, &tx),
                    );
                }
            }
        }
        cx.notify();
    }

    fn button(
        &self,
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        action: Action,
        primary: bool,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let keyboard_action = action.clone();
        div()
            .id(id)
            .tab_index(0)
            .flex()
            .items_center()
            .justify_center()
            .px_4()
            .py_3()
            .rounded_full()
            .border_1()
            .border_color(rgb(if primary { GREEN } else { 0x1f1f1f }))
            .bg(rgb(if primary { GREEN } else { 0x1f1f1f }))
            .text_color(rgb(if primary { 0x000000 } else { TEXT }))
            .text_size(px(14.))
            .font_weight(FontWeight::BOLD)
            .when(disabled, |s| s.opacity(0.4))
            .when(!disabled, |s| {
                s.cursor_pointer()
                    .hover(|s| s.bg(rgb(if primary { 0x3be477 } else { 0x333333 })))
            })
            .focus(|s| s.border_color(rgb(TEXT)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !disabled {
                    this.act(action.clone(), cx);
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if !disabled && (event.keystroke.key == "enter" || event.keystroke.key == "space") {
                    this.act(keyboard_action.clone(), cx);
                    cx.stop_propagation();
                }
            }))
            .child(text.into())
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(208.))
            .flex_shrink_0()
            .h_full()
            .rounded_lg()
            .bg(rgb(BG))
            .p_4()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_3()
                    .mb_5()
                    .child(img(self.logo.clone()).size(px(44.)).flex_shrink_0())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("rusty"),
                            )
                            .child(label("Minecraft Launcher").text_size(px(10.))),
                    ),
            )
            .children(
                [
                    (Page::Play, "홈"),
                    (Page::Versions, "버전 라이브러리"),
                    (Page::Settings, "설정"),
                    (Page::Logs, "활동 로그"),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (page, title))| {
                    self.button(("nav", i), title, Action::Navigate(page), false, false, cx)
                        .mb_1()
                        .justify_start()
                        .h(px(48.))
                        .bg(rgb(BG))
                        .border_color(rgb(BG))
                        .text_color(rgb(if self.page == page { TEXT } else { MUTED }))
                        .font_weight(if self.page == page {
                            FontWeight::BOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .when(self.page == page, |s| {
                            s.bg(rgb(0x272727)).border_color(rgb(0x272727))
                        })
                }),
            )
            .child(
                div()
                    .mt_6()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child("나만의 블록 세계"),
                    )
                    .child(label("버전을 고르고,\n새 세계로 떠나세요."))
                    .child(
                        self.button(
                            "sidebar-library",
                            "버전 둘러보기",
                            Action::Navigate(Page::Versions),
                            false,
                            false,
                            cx,
                        )
                        .bg(rgb(0xeeeeee))
                        .border_color(rgb(0xeeeeee))
                        .text_color(rgb(0x121212))
                        .hover(|s| s.bg(rgb(TEXT))),
                    ),
            )
            .child(div().flex_1())
            .child(self.button(
                "folder-side",
                "게임 폴더 열기 ↗",
                Action::Folder,
                false,
                false,
                cx,
            ))
            .child(
                label("JAVA EDITION · RUST + GPUI")
                    .text_size(px(10.))
                    .mt_4()
                    .px_2(),
            )
    }

    fn landscape() -> Div {
        let heights = [
            50., 70., 90., 65., 110., 135., 110., 150., 175., 140., 115., 155., 190., 160., 200.,
            180.,
        ];
        div()
            .absolute()
            .right_0()
            .bottom_0()
            .w(px(400.))
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .right(px(62.))
                    .top(px(36.))
                    .size(px(46.))
                    .bg(rgb(0xc6e5a2)),
            )
            .children(heights.into_iter().enumerate().map(|(i, h)| {
                div()
                    .absolute()
                    .left(px(i as f32 * 27.))
                    .bottom_0()
                    .w(px(28.))
                    .h(px(h))
                    .bg(rgb(if i % 3 == 0 { 0x304f32 } else { 0x28452d }))
                    .border_t_4()
                    .border_color(rgb(0x64884b))
            }))
            .children(
                [40., 64., 45., 82., 60., 100., 80., 65., 110., 85.]
                    .into_iter()
                    .enumerate()
                    .map(|(i, h)| {
                        div()
                            .absolute()
                            .left(px(i as f32 * 43.))
                            .bottom_0()
                            .w(px(44.))
                            .h(px(h))
                            .bg(rgb(if i % 2 == 0 { 0x3c6038 } else { 0x355631 }))
                            .border_t_4()
                            .border_color(rgb(0x8aaf62))
                    }),
            )
    }

    fn loader_tabs(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(LoaderKind::ALL.into_iter().enumerate().map(|(i, kind)| {
                self.button(
                    ("loader-kind", i),
                    kind.name(),
                    Action::Loader(kind),
                    self.settings.loader == kind,
                    self.busy || self.running,
                    cx,
                )
                .py_2()
            }))
    }

    fn loader_panel(&self, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().gap_3()
            .child(self.loader_tabs(cx))
            .when(self.settings.loader!=LoaderKind::Vanilla,|s|s.child(
                div().p_4().rounded_lg().bg(rgb(PANEL)).flex().flex_col().gap_3()
                    .child(div().flex().items_center().justify_between()
                        .child(div().font_weight(FontWeight::BOLD).child(format!("{} 로더 버전",self.settings.loader.name())))
                        .child(self.button("mods-folder","모드 폴더 ↗",Action::Mods,false,false,cx).py_2()))
                    .child(label(if self.loader_choices.is_empty() { "호환 로더가 없습니다. 다른 게임 버전을 선택하거나 목록을 다시 불러오세요." } else { "안정 버전을 우선 표시합니다. 설치할 로더 버전을 선택하세요." }))
                    .child(div().id("loader-options").max_h(px(132.)).overflow_y_scroll().child(
                        div().flex().flex_wrap().gap_2().children(self.loader_choices.iter().enumerate().map(|(i,v)| {
                            let display=v.id.strip_prefix(&format!("{}-",self.settings.selected_version)).unwrap_or(&v.id);
                            self.button(("loader-version",i),format!("{display}{}",if v.stable { "" } else { " (시험판)" }),Action::LoaderVersion(v.id.clone()),self.settings.loader_version==v.id,self.busy||self.running,cx).py_2()
                        }))))
                    .child(self.button("installer-log","공식 설치기 로그 열기 ↗",Action::InstallerLog,false,false,cx).w(px(220.)).py_2())
            ))
    }

    fn play_page(&self, cx: &mut Context<Self>) -> Div {
        let id = &self.settings.selected_version;
        let installed = self.profile_installed(id);
        let recent: Vec<_> = self
            .versions
            .iter()
            .filter(|v| self.profile_installed(&v.id))
            .take(3)
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .relative()
                    .overflow_hidden()
                    .h(px(196.))
                    .flex_shrink_0()
                    .rounded_lg()
                    .bg(rgb(0x1f1f1f))
                    .child(Self::landscape())
                    .child(
                        div()
                            .relative()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                label("MINECRAFT JAVA EDITION")
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD),
                            )
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("다음 모험을 시작하세요."),
                            )
                            .child(label("선택한 버전으로, 나만의 속도로."))
                            .child(
                                self.button(
                                    "hero-library",
                                    "버전 둘러보기",
                                    Action::Navigate(Page::Versions),
                                    false,
                                    false,
                                    cx,
                                )
                                .mt_3()
                                .w(px(156.))
                                .bg(rgb(0xeeeeee))
                                .border_color(rgb(0xeeeeee))
                                .text_color(rgb(BG))
                                .hover(|s| s.bg(rgb(TEXT))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .child("이어서 플레이"),
                    )
                    .child(
                        self.button(
                            "choose-version",
                            "버전 / 로더 변경",
                            Action::Navigate(Page::Versions),
                            false,
                            false,
                            cx,
                        )
                        .py_2(),
                    ),
            )
            .child(
                div()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .p_4()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .size(px(64.))
                            .rounded_md()
                            .bg(rgb(0x252525))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img(self.logo.clone()).size(px(56.))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(if id.is_empty() {
                                        "버전 목록을 불러오는 중".to_owned()
                                    } else {
                                        format!("Minecraft {id}")
                                    }),
                            )
                            .child(label(format!(
                                "{} · {}",
                                self.settings.loader.name(),
                                if installed {
                                    "설치됨 · 하단 플레이 버튼으로 시작"
                                } else {
                                    "하단 다운로드 버튼으로 설치"
                                }
                            ))),
                    )
                    .child(
                        div()
                            .rounded_full()
                            .px_3()
                            .py_1()
                            .bg(rgb(0x272727))
                            .text_size(px(10.))
                            .font_weight(FontWeight::BOLD)
                            .child("JAVA"),
                    ),
            )
            .child(
                div().flex().gap_3().children(
                    [
                        (
                            "메모리",
                            format!("{} GB", self.settings.memory_mb as f32 / 1024.),
                        ),
                        (
                            "런타임",
                            if self.settings.java_path.is_empty() {
                                "Java 자동 탐색".to_owned()
                            } else {
                                "Java 직접 지정".to_owned()
                            },
                        ),
                        (
                            "계정",
                            self.session
                                .as_ref()
                                .map(|s| s.name.clone())
                                .unwrap_or("로그인 필요".to_owned()),
                        ),
                    ]
                    .into_iter()
                    .map(|(name, value)| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .p_4()
                            .rounded_lg()
                            .bg(rgb(PANEL))
                            .child(label(name))
                            .child(
                                div()
                                    .mt_2()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::BOLD)
                                    .truncate()
                                    .child(value),
                            )
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .child("내 라이브러리"),
                    )
                    .child(label(format!(
                        "{}개 설치됨",
                        self.versions
                            .iter()
                            .filter(|v| self.profile_installed(&v.id))
                            .count()
                    ))),
            )
            .when(recent.is_empty(), |s| {
                s.child(div().p_4().rounded_lg().bg(rgb(PANEL)).child(label(
                    "아직 설치한 버전이 없습니다. 버전 라이브러리에서 첫 버전을 골라보세요.",
                )))
            })
            .children(recent.into_iter().enumerate().map(|(i, v)| {
                div()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(img(self.logo.clone()).size(px(36.)))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Minecraft {}", v.id)),
                    )
                    .child(label(self.settings.loader.name()).text_size(px(10.)))
                    .child(
                        self.button(
                            ("recent", i),
                            if &v.id == id { "선택됨" } else { "선택" },
                            Action::Select(v.id.clone()),
                            false,
                            self.busy || self.running,
                            cx,
                        )
                        .py_2(),
                    )
            }))
    }

    fn play_bar(&self, cx: &mut Context<Self>) -> Div {
        let id = &self.settings.selected_version;
        let installed = self.profile_installed(id);
        let needs_login = self
            .session
            .as_ref()
            .is_none_or(|s| s.expires <= std::time::Instant::now());
        let action_label = if self.running {
            "게임 실행 중"
        } else if self.busy {
            "준비 중"
        } else if !installed {
            "설치하기"
        } else if needs_login {
            "로그인하고 플레이"
        } else {
            "플레이"
        };
        div()
            .flex_shrink_0()
            .rounded_lg()
            .bg(rgb(PANEL))
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_5()
            .child(
                div()
                    .w(px(200.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(img(self.logo.clone()).size(px(48.)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(px(14.))
                                    .truncate()
                                    .child(if id.is_empty() {
                                        "버전 선택".to_owned()
                                    } else {
                                        format!("Minecraft {id}")
                                    }),
                            )
                            .child(
                                label(format!(
                                    "{} · {}",
                                    self.settings.loader.name(),
                                    if installed {
                                        "설치됨"
                                    } else {
                                        "설치 필요"
                                    }
                                ))
                                .text_size(px(11.)),
                            ),
                    ),
            )
            .child(
                self.button(
                    "global-play",
                    if self.busy {
                        "…"
                    } else if installed {
                        "▶"
                    } else {
                        "↓"
                    },
                    Action::Play,
                    true,
                    self.busy || self.running || id.is_empty(),
                    cx,
                )
                .size(px(52.))
                .p_0()
                .text_size(px(22.))
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(13.))
                            .child(action_label),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(if self.error { ERROR } else { MUTED }))
                            .line_clamp(2)
                            .child(self.status.clone()),
                    )
                    .when(self.busy, |s| {
                        s.child(
                            div()
                                .w_full()
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgb(0x4d4d4d))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .w(relative(self.progress.clamp(0., 1.)))
                                        .bg(rgb(GREEN)),
                                ),
                        )
                    }),
            )
            .when(self.busy && self.cancellable, |s| {
                s.child(self.button("cancel", "취소", Action::Cancel, false, false, cx))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .child(
                        self.button(
                            "bar-settings",
                            format!("{} GB RAM", self.settings.memory_mb as f32 / 1024.),
                            Action::Navigate(Page::Settings),
                            false,
                            false,
                            cx,
                        )
                        .py_2(),
                    )
                    .child(label("RUSTY  /  JAVA EDITION").text_size(px(9.))),
            )
    }

    fn versions_page(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.loader_panel(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(
                        "공식 배포 버전 · 선택한 버전으로 바로 설치할 수 있습니다",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "snapshots",
                                if self.settings.show_snapshots {
                                    "스냅샷 포함 ✓"
                                } else {
                                    "정식 버전만"
                                },
                                Action::Snapshots,
                                false,
                                self.busy || self.running,
                                cx,
                            ))
                            .child(self.button(
                                "refresh",
                                "새로고침",
                                Action::Refresh,
                                false,
                                self.busy || self.running,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .child(
                        div()
                            .child(label("선택한 버전"))
                            .child(self.settings.selected_version.clone()),
                    )
                    .child(self.button(
                        "install-version",
                        "설치 / 파일 검사",
                        Action::Install,
                        true,
                        self.busy || self.running || self.settings.selected_version.is_empty(),
                        cx,
                    )),
            )
            .children(
                self.versions
                    .iter()
                    .filter(|v| self.settings.show_snapshots || v.kind == "release")
                    .enumerate()
                    .map(|(i, v)| {
                        let selected = v.id == self.settings.selected_version;
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .border_b_1()
                            .border_color(rgb(LINE))
                            .pb_3()
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(rgb(if selected { GREEN } else { MUTED })),
                            )
                            .child(div().flex_1().child(v.id.clone()))
                            .child(
                                label(if v.kind == "release" {
                                    "RELEASE"
                                } else {
                                    "SNAPSHOT"
                                })
                                .w(px(80.)),
                            )
                            .child(
                                label(if self.profile_installed(&v.id) {
                                    "설치됨"
                                } else {
                                    "다운로드 가능"
                                })
                                .w(px(90.)),
                            )
                            .child(self.button(
                                ("version", i),
                                if selected { "선택됨 ✓" } else { "선택" },
                                Action::Select(v.id.clone()),
                                false,
                                self.busy || self.running,
                                cx,
                            ))
                    }),
            )
    }

    fn settings_page(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .child(div().text_size(px(18.)).child("메모리"))
                    .child(label("게임에 할당할 최대 RAM을 선택하세요.").mt_2())
                    .child(
                        div().flex().gap_2().mt_4().children(
                            [2048, 4096, 6144, 8192, 12288, 16384]
                                .into_iter()
                                .enumerate()
                                .map(|(i, mb)| {
                                    self.button(
                                        ("memory", i),
                                        format!("{} GB", mb / 1024),
                                        Action::Memory(mb),
                                        self.settings.memory_mb == mb,
                                        self.busy || self.running,
                                        cx,
                                    )
                                }),
                        ),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(LINE)))
            .child(div().text_size(px(18.)).child("Java & Microsoft 로그인"))
            .child(label(
                "Java는 JAVA_HOME, PATH와 일반적인 설치 폴더에서 자동으로 찾습니다.",
            ))
            .child(label(
                "직접 지정하려면 설정 파일의 java_path에 java.exe 경로를 입력하세요.",
            ))
            .child(
                div()
                    .p_5()
                    .rounded_lg()
                    .bg(rgb(PANEL))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(rgb(TEXT)).child(
                        if self.settings.microsoft_client_id.is_empty() {
                            "Microsoft 앱 ID 설정이 필요합니다"
                        } else {
                            "Microsoft 앱 ID가 설정되어 있습니다"
                        },
                    ))
                    .child(label(
                        "본인의 Microsoft 앱을 등록하고 장치 코드 로그인을 활성화한 뒤,",
                    ))
                    .child(label(
                        "microsoft_client_id를 입력하세요. Minecraft API 접근 권한도 필요합니다.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(self.button(
                                "edit-settings",
                                "설정 파일 편집 ↗",
                                Action::EditSettings,
                                false,
                                self.busy || self.running,
                                cx,
                            ))
                            .child(self.button(
                                "reload-settings",
                                "설정 다시 불러오기",
                                Action::ReloadSettings,
                                false,
                                self.busy || self.running,
                                cx,
                            )),
                    ),
            )
            .child(label(
                "인증 토큰은 메모리에만 보관합니다. 런처를 다시 열면 로그인이 필요합니다.",
            ))
            .child(div().text_size(px(18.)).mt_3().child("저장 위치"))
            .child(label(
                loaders::instance_dir(&self.settings)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ))
            .child(
                self.button(
                    "open-data",
                    "게임 폴더 열기 ↗",
                    Action::Folder,
                    false,
                    false,
                    cx,
                )
                .w(px(190.)),
            )
    }
}

impl Render for Launcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locked = self.busy || self.running;
        let title = match self.page {
            Page::Play => "홈",
            Page::Versions => "버전 라이브러리",
            Page::Settings => "설정",
            Page::Logs => "활동 로그",
        };
        let content = match self.page {
            Page::Play => self.play_page(cx),
            Page::Versions => self.versions_page(cx),
            Page::Settings => self.settings_page(cx),
            Page::Logs => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    self.button(
                        "game-log",
                        "게임 로그 열기 ↗",
                        Action::Log,
                        false,
                        false,
                        cx,
                    )
                    .w(px(190.)),
                )
                .when(self.logs.is_empty(), |s| {
                    s.child(label("아직 기록된 활동이 없습니다."))
                })
                .children(self.logs.iter().rev().map(|s| {
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(rgb(PANEL))
                        .text_size(px(13.))
                        .child(s.clone())
                })),
        };
        div()
            .id("root")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .bg(rgb(0x000000))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .text_size(px(14.))
            .on_key_down(|event, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev();
                    } else {
                        window.focus_next();
                    }
                    cx.stop_propagation();
                }
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap_2()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .rounded_lg()
                            .overflow_hidden()
                            .bg(rgb(BG))
                            .child(
                                div()
                                    .h(px(72.))
                                    .flex_shrink_0()
                                    .px_6()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(24.))
                                            .font_weight(FontWeight::BOLD)
                                            .child(title),
                                    )
                                    .child(
                                        self.button(
                                            "account",
                                            self.session
                                                .as_ref()
                                                .map(|s| format!("{} · 로그아웃", s.name))
                                                .unwrap_or("Microsoft 로그인 ↗".to_owned()),
                                            if self.session.is_some() {
                                                Action::Logout
                                            } else {
                                                Action::Login
                                            },
                                            false,
                                            locked,
                                            cx,
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .id(("content", self.page as usize))
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .px_6()
                                    .pb_6()
                                    .pt_2()
                                    .child(content),
                            )
                            .when_some(self.device.clone(), |s, (code, _)| {
                                s.child(
                                    div()
                                        .px_6()
                                        .py_4()
                                        .bg(rgb(0x252525))
                                        .flex()
                                        .items_center()
                                        .gap_4()
                                        .child(
                                            div()
                                                .flex_1()
                                                .child(label("MICROSOFT 로그인 코드"))
                                                .child(
                                                    div()
                                                        .text_size(px(22.))
                                                        .font_weight(FontWeight::BOLD)
                                                        .child(code),
                                                ),
                                        )
                                        .child(self.button(
                                            "copy-code",
                                            "코드 복사",
                                            Action::CopyCode,
                                            false,
                                            false,
                                            cx,
                                        ))
                                        .child(self.button(
                                            "login-browser",
                                            "브라우저 열기 ↗",
                                            Action::Browser,
                                            true,
                                            false,
                                            cx,
                                        )),
                                )
                            }),
                    ),
            )
            .child(self.play_bar(cx))
    }
}

pub fn run() {
    Application::new().run(|cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1160.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(940.), px(680.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Rusty · Minecraft Launcher".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Launcher::new(window, cx)),
        )
        .expect("런처 창을 열 수 없습니다");
        cx.activate(true);
    });
}
