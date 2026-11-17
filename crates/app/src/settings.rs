//! Settings: dictionaries, Chinese explanations (LLM), review, OCR models, phone pairing.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Disableable, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use picyomi_core::dict::{self, DictInfo};
use picyomi_core::audio::{self, TtsConfig};
use picyomi_core::explain::LlmConfig;
use picyomi_core::models;

use crate::state::{rt, svc};

pub struct SettingsView {
    dicts: Vec<DictInfo>,
    base_url: Entity<InputState>,
    model: Entity<InputState>,
    api_key: Entity<InputState>,
    retention: Entity<InputState>,
    new_per_day: Entity<InputState>,
    azure_key: Entity<InputState>,
    azure_region: Entity<InputState>,
    status: Option<String>,
    busy: bool,
    model_progress: Option<String>,
    pair_url: Option<String>,
    qr: Option<Arc<Image>>,
    tailscale_note: String,
}

fn input(window: &mut Window, cx: &mut Context<SettingsView>, placeholder: &str, value: String) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value))
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let service = svc(cx);
        let llm = LlmConfig::load(&service.db).unwrap_or_default();
        let sched = service.scheduler();
        let tts = TtsConfig::load(&service.db).unwrap_or_default();
        let key_hint = if tts.azure_key.is_empty() { "Azure 语音 key".to_string() } else { "已保存（留空则不改）".to_string() };
        let mut this = Self {
            azure_key: input(window, cx, &key_hint, String::new()),
            azure_region: input(window, cx, "japaneast", tts.azure_region.clone()),
            dicts: dict::list(&service.db).unwrap_or_default(),
            base_url: input(window, cx, "https://api.deepseek.com", llm.base_url.clone()),
            model: input(window, cx, "deepseek-flash", llm.model.clone()),
            api_key: input(window, cx, "API key（留空则不改）", String::new()),
            retention: input(window, cx, "0.9", format!("{}", sched.config.desired_retention)),
            new_per_day: input(window, cx, "20", service.new_cards_per_day().to_string()),
            status: None,
            busy: false,
            model_progress: None,
            pair_url: None,
            qr: None,
            tailscale_note: "正在读取 Tailscale 状态…".into(),
        };
        this.load_pairing(cx);
        this
    }

    fn refresh_dicts(&mut self, cx: &mut Context<Self>) {
        self.dicts = dict::list(&svc(cx).db).unwrap_or_default();
        cx.notify();
    }

    fn import_dicts(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("导入 Yomitan 词典 (.zip)".into()) });
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            this.update(cx, |this, cx| {
                this.busy = true;
                this.status = Some(format!("正在导入 {} 个词典…", paths.len()));
                cx.notify();
            })
            .ok();
            let res = cx
                .background_spawn(async move {
                    let mut done = vec![];
                    for p in paths {
                        let r = dict::import_zip(&service.db, &p, true, |_, _, _| {})?;
                        done.push(format!("{}（{} 词条，{} 频率）", r.title, r.terms, r.meta));
                    }
                    anyhow::Ok(done)
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.status = Some(match res {
                    Ok(done) => format!("已导入：{}", done.join("、")),
                    Err(e) => format!("导入失败：{e:#}"),
                });
                this.refresh_dicts(cx);
            })
            .ok();
        })
        .detach();
    }

    fn save_llm(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        let old = LlmConfig::load(&service.db).unwrap_or_default();
        let key = self.api_key.read(cx).value().trim().to_string();
        let cfg = LlmConfig {
            base_url: self.base_url.read(cx).value().trim().to_string(),
            model: self.model.read(cx).value().trim().to_string(),
            api_key: if key.is_empty() { old.api_key } else { key },
        };
        self.status = Some(match cfg.save(&service.db) {
            Ok(()) => "已保存中文解释设置".into(),
            Err(e) => format!("保存失败：{e:#}"),
        });
        cx.notify();
    }

    fn save_tts(&mut self, voice: Option<&str>, cx: &mut Context<Self>) -> bool {
        let service = svc(cx);
        let mut cfg = TtsConfig::load(&service.db).unwrap_or_default();
        let key = self.azure_key.read(cx).value().trim().to_string();
        if !key.is_empty() {
            cfg.azure_key = key;
        }
        cfg.azure_region = self.azure_region.read(cx).value().trim().to_string();
        if let Some(v) = voice {
            cfg.voice = v.to_string();
        }
        let res = cfg.save(&service.db);
        self.status = Some(match &res {
            Ok(()) => "已保存发音设置".into(),
            Err(e) => format!("保存失败：{e:#}"),
        });
        cx.notify();
        res.is_ok()
    }

    fn try_voice(&mut self, voice: Option<&str>, cx: &mut Context<Self>) {
        if self.save_tts(voice, cx) {
            crate::sound::play_sentence(VOICE_SAMPLE, "", "", cx);
        }
    }

    fn test_llm(&mut self, cx: &mut Context<Self>) {
        self.save_llm(cx);
        let service = svc(cx);
        let req = picyomi_core::explain::ExplainRequest {
            expression: "根くらべ".into(),
            reading: "ねくらべ".into(),
            sentence: "こーなったらご主人さまと根くらべよ！".into(),
            definition_hint: String::new(),
        };
        self.status = Some("测试中…".into());
        let handle = rt(cx).spawn(async move { picyomi_core::explain::explain(&service.db, &req).await });
        cx.spawn(async move |this, cx| {
            let r = handle.await;
            this.update(cx, |this, cx| {
                this.status = Some(match r {
                    Ok(Ok(e)) => format!("连接成功：{}", e.meaning_zh),
                    Ok(Err(e)) => format!("失败：{e:#}"),
                    Err(e) => format!("失败：{e}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save_srs(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        let r: f32 = self.retention.read(cx).value().trim().parse().unwrap_or(0.9);
        let n: usize = self.new_per_day.read(cx).value().trim().parse().unwrap_or(20);
        let _ = service.db.set_setting("srs.desired_retention", Some(&r.clamp(0.7, 0.99).to_string()));
        let _ = service.db.set_setting("srs.new_per_day", Some(&n.min(500).to_string()));
        self.status = Some("已保存复习设置".into());
        cx.notify();
    }

    fn download_models(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let handle = rt(cx).spawn(async move {
            models::ensure_all(|name, got, total| {
                let pct = total.map(|t| format!("{:.0}%", got as f64 * 100.0 / t as f64)).unwrap_or_else(|| format!("{} KB", got / 1024));
                let _ = tx.send(format!("下载 {name} {pct}"));
            })
            .await
        });
        self.model_progress = Some("开始下载…".into());
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                let mut last = None;
                while let Ok(m) = rx.try_recv() {
                    last = Some(m);
                }
                let finished = handle.is_finished();
                this.update(cx, |this, cx| {
                    if let Some(m) = last {
                        this.model_progress = Some(m);
                    }
                    if finished {
                        this.model_progress = Some(if models::all_present() { "模型已就绪".into() } else { "下载失败，请重试".into() });
                    }
                    cx.notify();
                })
                .ok();
                if finished {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn load_pairing(&mut self, cx: &mut Context<Self>) {
        let service = svc(cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    let token = picyomi_core::server::token(&service)?;
                    let out = std::process::Command::new("tailscale").args(["status", "--json"]).output();
                    let dns = out
                        .ok()
                        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
                        .and_then(|v| v["Self"]["DNSName"].as_str().map(|s| s.trim_end_matches('.').to_string()))
                        .filter(|s| !s.is_empty());
                    let url = dns.as_ref().map(|d| format!("https://{d}/app/#pair={token}"));
                    let qr = url.as_ref().and_then(|u| qr_png(u).ok());
                    anyhow::Ok((dns, url, qr))
                })
                .await;
            this.update(cx, |this, cx| {
                match res {
                    Ok((Some(dns), url, qr)) => {
                        this.tailscale_note = format!("本机 Tailscale 地址：{dns}");
                        this.pair_url = url;
                        this.qr = qr.map(|b| Arc::new(Image::from_bytes(ImageFormat::Png, b)));
                    }
                    Ok((None, _, _)) => this.tailscale_note = "没有检测到 Tailscale。安装并登录后重新打开设置页。".into(),
                    Err(e) => this.tailscale_note = format!("读取配对信息失败：{e:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn qr_png(text: &str) -> anyhow::Result<Vec<u8>> {
    let code = qrcode::QrCode::new(text.as_bytes())?;
    let img = code.render::<image::Luma<u8>>().min_dimensions(240, 240).quiet_zone(true).build();
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}

fn section(title: &str, description: &str, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .child(
            v_flex()
                .gap_0p5()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                .when(!description.is_empty(), |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(description.to_string()))),
        )
}

const SERVE_CMD: &str = "tailscale serve --bg 7331";
const VOICE_SAMPLE: &str = "こーなったらご主人さまと根くらべよ！";

/// Small 「复制」 button that puts `text` on the clipboard.
fn copy_button(id: &'static str, text: String, cx: &mut Context<SettingsView>) -> Button {
    Button::new(id).xsmall().ghost().icon(IconName::Copy).label("复制").on_click(cx.listener(move |this, _, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        this.status = Some("已复制到剪贴板".into());
        cx.notify();
    }))
}

/// Label + control on one line.
fn field(label: &str, control: impl IntoElement) -> Div {
    h_flex().gap_3().items_center().child(div().flex_none().w(px(88.)).text_sm().child(label.to_string())).child(div().flex_1().min_w_0().child(control))
}

/// Switch + title (+ muted explanation).
fn toggle(switch: impl IntoElement, title: &str, note: &str, cx: &App) -> Div {
    h_flex().gap_3().items_start().child(div().pt_0p5().child(switch)).child(
        v_flex()
            .flex_1()
            .min_w_0()
            .child(div().text_sm().child(title.to_string()))
            .when(!note.is_empty(), |d| d.child(div().text_xs().text_color(cx.theme().muted_foreground).child(note.to_string()))),
    )
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;

        let mut dicts = section("词典", "导入 Yomitan 格式的词典包（.zip）。只保存在本机，不会上传。排在前面的优先显示。", cx);
        for d in &self.dicts {
            let id = d.id;
            let kind = if d.term_count > 0 { format!("{} 词条", d.term_count) } else { format!("{} 频率", d.meta_count) };
            dicts = dicts.child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Switch::new(("dict", id as usize)).checked(d.enabled).on_change(cx.listener(move |this, on: &bool, _, cx| {
                            let _ = dict::set_enabled(&svc(cx).db, id, *on);
                            this.refresh_dicts(cx);
                        })),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(d.title.clone()))
                    .child(div().flex_none().text_xs().text_color(muted).child(kind)),
            );
        }
        if self.dicts.is_empty() {
            dicts = dicts.child(div().text_sm().text_color(theme.danger).child("还没有词典：查词需要至少一本日语词典。"));
        }
        dicts = dicts.child(
            h_flex().child(
                Button::new("import-dict").small().icon(IconName::FolderOpen).label("导入词典…").disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.import_dicts(cx))),
            ),
        );

        let llm = section("中文解释", "任何 OpenAI 兼容接口（DeepSeek、通义、Kimi、OpenAI、本地 Ollama）或 Gemini。只在你点「中文解释」或加入词单时调用，按句子缓存。", cx)
            .child(field("接口地址", Input::new(&self.base_url).small()))
            .child(field("模型", Input::new(&self.model).small()))
            .child(field("API Key", Input::new(&self.api_key).small().mask_toggle()))
            .child(
                h_flex()
                    .gap_2()
                    .pl(px(100.))
                    .child(Button::new("save-llm").small().primary().label("保存").on_click(cx.listener(|this, _, _, cx| this.save_llm(cx))))
                    .child(Button::new("test-llm").small().label("测试").on_click(cx.listener(|this, _, _, cx| this.test_llm(cx)))),
            );

        let tts = TtsConfig::load(&svc(cx).db).unwrap_or_default();
        let mut voices = h_flex().flex_wrap().gap_1();
        for (i, (id, label)) in audio::VOICES.iter().enumerate() {
            voices = voices.child(
                Button::new(("voice", i))
                    .xsmall()
                    .label(*label)
                    .when(tts.voice == *id, |b| b.primary())
                    .when(tts.voice != *id, |b| b.outline())
                    .on_click(cx.listener(move |this, _, _, cx| this.try_voice(Some(id), cx))),
            );
        }
        let speech = section(
            "发音",
            "单词优先用真人录音（JapanesePod101，没有再找 Lingua Libre）。查不到录音的词和整句朗读用 Azure 神经语音；同形异读的词（如「上手」）按词典读音念。没填 key 时用 macOS 自带的 Kyoko。音频缓存在本机。",
            cx,
        )
        .child(field("Azure Key", Input::new(&self.azure_key).small().mask_toggle()))
        .child(field("区域", h_flex().gap_2().child(div().w(px(140.)).child(Input::new(&self.azure_region).small())).child(div().text_xs().text_color(muted).child("创建语音资源时选的区域，如 japaneast"))))
        .child(field("声音", voices))
        .child(
            h_flex()
                .gap_2()
                .pl(px(100.))
                .child(Button::new("save-tts").small().primary().label("保存").on_click(cx.listener(|this, _, _, cx| {
                    this.save_tts(None, cx);
                })))
                .child(Button::new("try-tts").small().icon(crate::sound::speaker_icon()).label("试听").on_click(cx.listener(|this, _, _, cx| this.try_voice(None, cx)))),
        )
        .child(toggle(
            Switch::new("autoplay").checked(tts.autoplay).on_change(cx.listener(|_, on: &bool, _, cx| {
                let _ = svc(cx).db.set_setting(audio::AUTOPLAY, Some(if *on { "1" } else { "0" }));
                cx.notify();
            })),
            "复习翻面时自动播放",
            "看答案时念一遍单词；按 R 再听一遍。手机端同样生效。",
            cx,
        ));

        let srs = section("复习", "FSRS 记忆曲线排程。", cx)
            .child(field("目标记忆率", h_flex().gap_2().child(div().w(px(90.)).child(Input::new(&self.retention).small())).child(div().text_xs().text_color(muted).child("0.7–0.99，越高复习越频繁"))))
            .child(field("每日新词", div().w(px(90.)).child(Input::new(&self.new_per_day).small())))
            .child({
                let four = svc(cx).four_buttons();
                toggle(
                    Switch::new("four-buttons").checked(four).on_change(cx.listener(|_, on: &bool, _, cx| {
                        let _ = svc(cx).db.set_setting(picyomi_core::service::FOUR_BUTTONS, Some(if *on { "1" } else { "0" }));
                        cx.notify();
                    })),
                    "显示「困难」「简单」",
                    "默认只有「忘了 / 记得」两个按钮，记忆曲线靠的主要就是这个判断。打开后恢复 Anki 式的四个按钮。",
                    cx,
                )
            })
            .child(h_flex().pl(px(100.)).child(Button::new("save-srs").small().primary().label("保存").on_click(cx.listener(|this, _, _, cx| this.save_srs(cx)))));

        let scan_on = crate::scan::setting_on(crate::scan::SETTING_ENABLED, true, cx);
        let scan_browsers = crate::scan::setting_on(crate::scan::SETTING_BROWSERS, false, cx);
        let keys = section("屏幕查词", "第一次使用时 macOS 会请求「屏幕录制」权限；截图只在本机处理。", cx)
            .child(toggle(
                Switch::new("scan-shift").checked(scan_on).on_change(cx.listener(|_, on: &bool, _, cx| {
                    let _ = svc(cx).db.set_setting(crate::scan::SETTING_ENABLED, Some(if *on { "1" } else { "0" }));
                    cx.notify();
                })),
                "按住 Shift 查词",
                "在任意 App 里按住 Shift 移动鼠标，弹窗跟着查，查到的词会在屏幕上标出。",
                cx,
            ))
            .child(toggle(
                Switch::new("scan-browsers").checked(scan_browsers).on_change(cx.listener(|_, on: &bool, _, cx| {
                    let _ = svc(cx).db.set_setting(crate::scan::SETTING_BROWSERS, Some(if *on { "1" } else { "0" }));
                    cx.notify();
                })),
                "在浏览器里也用",
                "默认关：浏览器里交给 pic_yomi 扩展，它直接读网页文字，更准。",
                cx,
            ))
            .child(h_flex().gap_3().child(div().w(px(36.)).text_sm().font_weight(FontWeight::SEMIBOLD).child("⌥⌘J")).child(div().text_sm().text_color(muted).child("识别鼠标附近的文字，打开一个可操作的查词窗口。")));

        let ready = models::all_present();
        let ocr = section("OCR 模型", &format!("识别模型许可：{}；检测模型许可：{}", models::RECOGNIZER.license, models::DETECTOR.license), cx)
            .child(div().text_sm().child(if ready { "已就绪：RT-DETR 文字检测 + charspot 逐字识别（漫画竖排文字）。横排文字用 macOS 自带的 Vision。" } else { "尚未下载（约 55 MB）。漫画竖排文字需要它。" }))
            .when(!ready, |d| d.child(h_flex().child(Button::new("dl-models").small().primary().icon(IconName::ArrowDown).label("下载模型").on_click(cx.listener(|this, _, _, cx| this.download_models(cx))))))
            .when_some(self.model_progress.clone(), |d, p| d.child(div().text_xs().text_color(muted).child(p)));

        let phone = section("手机同步", "手机用网页版复习，离线也能背，连上 Mac 后自动同步。", cx)
            .child(div().text_sm().child(self.tailscale_note.clone()))
            .child(div().text_xs().text_color(muted).child("第一次使用：在终端运行下面的命令，让手机能通过 Tailscale（只有你自己的设备能访问）以 HTTPS 打开。"))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(theme.secondary)
                    .child(div().flex_1().min_w_0().font_family("Menlo").text_sm().child(SERVE_CMD))
                    .child(copy_button("copy-serve", SERVE_CMD.to_string(), cx)),
            )
            .child(div().text_xs().text_color(muted).child("然后用 iPhone 相机扫二维码，在 Safari 打开后「分享 → 添加到主屏幕」。"))
            .when_some(self.qr.clone(), |d, q| d.child(h_flex().child(div().flex_none().p_2().rounded_md().bg(gpui_kit::white()).child(img(q).size(px(180.))))))
            .when_some(self.pair_url.clone(), |d, u| {
                d.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w_0().text_xs().text_color(muted).truncate().child(u.clone()))
                        .child(copy_button("copy-pair", u, cx)),
                )
            });

        let banner = self.status.clone().map(|s| {
            h_flex()
                .flex_none()
                .mx_5()
                .mt_3()
                .px_3()
                .py_2()
                .gap_2()
                .rounded_md()
                .bg(theme.secondary)
                .text_sm()
                .child(div().flex_1().min_w_0().child(s))
                .child(Button::new("dismiss-status").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                    this.status = None;
                    cx.notify();
                })))
        });

        let body = v_flex()
            .size_full()
            .min_h_0()
            .children(banner)
            .child(div().flex_1().min_h_0().child(crate::ui::scroll(
                "settings-scroll",
                crate::ui::column(crate::ui::READ_WIDTH).gap_4().child(dicts).child(llm).child(speech).child(keys).child(srs).child(ocr).child(phone),
            )));
        crate::ui::page(crate::ui::header("设置", div(), cx), body)
    }
}
