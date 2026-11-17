//! pic_yomi desktop app: review, word list, lookup, settings, and global screen lookup (⌥⌘J).
//! Also hosts the local HTTP API used by the browser extension and the phone web app.

mod glossary;
mod lookup_view;
mod native;
mod panel;
mod popup;
mod review;
mod scan;
mod screen;
mod settings;
mod sound;
mod state;
mod ui;
mod words;

use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::component::sidebar::{Sidebar, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, IconName, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use picyomi_core::service::Service;

use crate::state::AppState;

actions!(picyomi, [Quit, ScreenLookup]);

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Review,
    Words,
    Lookup,
    Settings,
}

struct MainView {
    page: Page,
    review: Entity<review::ReviewView>,
    words: Entity<words::WordsView>,
    lookup: Entity<lookup_view::LookupView>,
    settings: Entity<settings::SettingsView>,
}

impl MainView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let review = cx.new(review::ReviewView::new);
        // First run: models still need downloading, so start on Settings.
        let view = Self {
            page: if picyomi_core::models::all_present() { Page::Review } else { Page::Settings },
            review: review.clone(),
            words: cx.new(|cx| words::WordsView::new(window, cx)),
            lookup: cx.new(|cx| lookup_view::LookupView::new(window, cx)),
            settings: cx.new(|cx| settings::SettingsView::new(window, cx)),
        };
        let focus = review.read(cx).focus_handle();
        window.focus(&focus, cx);
        // Follow the system's light/dark appearance, now and when it changes.
        Theme::sync_system_appearance(Some(window), cx);
        Theme::sync_scrollbar_appearance(cx);
        cx.observe_window_appearance(window, |_, window, cx| Theme::sync_system_appearance(Some(window), cx)).detach();
        // Keep the sidebar's due-count badge current as cards are reviewed.
        cx.observe(&review, |_, _, cx| cx.notify()).detach();
        view
    }

    fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        match page {
            Page::Review => {
                self.review.update(cx, |r, cx| r.reload(cx));
                let focus = self.review.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
            Page::Words => self.words.update(cx, |w, cx| w.reload(cx)),
            _ => {}
        }
        cx.notify();
    }
}

/// Below this window width the sidebar shrinks to icons.
const SIDEBAR_COLLAPSE: f32 = 760.;

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = window.viewport_size().width < px(SIDEBAR_COLLAPSE);
        let due = self.review.read(cx).due_count();
        let muted = cx.theme().muted_foreground;
        let item = |label: &'static str, icon: IconName, page: Page, current: Page, cx: &mut Context<Self>| {
            SidebarMenuItem::new(label).icon(icon).active(current == page).on_click(cx.listener(move |this, _, w, cx| this.go(page, w, cx)))
        };
        let p = self.page;
        let review_item = item("复习", IconName::BookOpen, Page::Review, p, cx).when(due > 0, |it| {
            it.suffix(move |_, cx| div().px_1p5().rounded_full().bg(cx.theme().primary).text_color(cx.theme().primary_foreground).text_xs().child(due.to_string()))
        });
        let sidebar = Sidebar::new("sidebar")
            .collapsed(collapsed)
            .when(!collapsed, |s| {
                s.header(
                    SidebarHeader::new().child(
                        v_flex()
                            .child(div().font_weight(FontWeight::BOLD).child("pic_yomi"))
                            .child(div().text_xs().text_color(muted).child("按住 Shift 查词 · ⌥⌘J")),
                    ),
                )
            })
            .child(
                SidebarGroup::new("学习").child(
                    SidebarMenu::new()
                        .child(review_item)
                        .child(item("词单", IconName::Inbox, Page::Words, p, cx))
                        .child(item("查词", IconName::Search, Page::Lookup, p, cx)),
                ),
            )
            .child(SidebarGroup::new("设置").child(SidebarMenu::new().child(item("设置", IconName::Settings, Page::Settings, p, cx))));
        let content: AnyElement = match self.page {
            Page::Review => self.review.clone().into_any_element(),
            Page::Words => self.words.clone().into_any_element(),
            Page::Lookup => self.lookup.clone().into_any_element(),
            Page::Settings => self.settings.clone().into_any_element(),
        };
        h_flex()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(sidebar)
            .child(div().flex_1().min_w_0().h_full().overflow_hidden().child(content))
    }
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let svc = Service::open()?;
    {
        let svc = svc.clone();
        runtime.spawn(async move {
            if let Err(e) = picyomi_core::server::serve(svc, picyomi_core::server::DEFAULT_PORT).await {
                eprintln!("local API not started: {e:#}");
            }
        });
    }
    let rt = runtime.handle().clone();
    // Keep the runtime alive for the whole process.
    let _runtime = Box::leak(Box::new(runtime));

    gpui_kit::application().with_assets(sound::AppAssets).run(move |cx| {
        gpui_kit::init(cx);
        cx.set_global(AppState { svc: svc.clone(), rt: rt.clone() });
        review::bind_keys(cx);
        popup::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ScreenLookup, cx| popup::screen_lookup(cx));
        cx.set_menus([Menu {
            name: "pic_yomi".into(),
            items: vec![MenuItem::action("屏幕查词 ⌥⌘J", ScreenLookup), MenuItem::separator(), MenuItem::action("退出 pic_yomi", Quit)],
            disabled: false,
        }]);

        // The first Vision text request loads its models (seconds on a cold start);
        // do that now so the first ⌥⌘J is fast.
        std::thread::spawn(|| {
            if let Ok(img) = image::load_from_memory(include_bytes!("../assets/vision-warmup.png")) {
                let _ = picyomi_core::vision::recognize_lines(&img);
            }
        });

        // Hold Shift over text anywhere to look words up.
        scan::start(cx);

        // Global hotkey ⌥⌘J. The manager must outlive the app.
        match GlobalHotKeyManager::new() {
            Ok(manager) => {
                let hotkey = HotKey::new(Some(Modifiers::ALT | Modifiers::META), Code::KeyJ);
                if let Err(e) = manager.register(hotkey) {
                    eprintln!("hotkey ⌥⌘J unavailable: {e}");
                }
                Box::leak(Box::new(manager));
                cx.spawn(async move |cx| loop {
                    cx.background_executor().timer(Duration::from_millis(30)).await;
                    while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                        if ev.state == HotKeyState::Pressed {
                            let _ = cx.update(popup::screen_lookup);
                        }
                    }
                })
                .detach();
            }
            Err(e) => eprintln!("global hotkeys unavailable: {e}"),
        }

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1040.), px(720.)), cx)),
            titlebar: Some(TitlebarOptions { title: Some("pic_yomi".into()), ..Default::default() }),
            window_min_size: Some(size(px(520.), px(460.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| MainView::new(window, cx))).expect("open main window");
        cx.activate(true);

        // Development aid: PICYOMI_DEBUG_POPUP=1 shows a sample hover popup, to check its layout.
        if std::env::var_os("PICYOMI_DEBUG_POPUP").is_some() {
            let sentence = "台湾の原発国民投票のTV討論会で、賛成派の黄士修が突然やりますねぇと言い始めた。".to_string();
            if let Ok(resp) = state::svc(cx).lookup(&sentence, 13) {
                popup::show_highlight((200., 200., 700., 120.), vec![[100., 40., 160., 66.]], cx);
                popup::show_hover([300., 240., 360., 266.], sentence, 13, resp, None, cx);
            }
        }
    });
    Ok(())
}
