//! Pronunciation playback. Core fetches and caches the clip; `afplay` plays it.
//! A new request stops the clip that is playing, and a slow request that
//! finishes after a newer one is dropped.

use std::path::Path;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::*;
use picyomi_core::audio;

use crate::state::{rt, svc};

static PLAYING: Mutex<Option<Child>> = Mutex::new(None);
static LATEST: AtomicU64 = AtomicU64::new(0);

gpui_kit::assets::icon_assets!(SoundIcons, [Volume2]);

/// The app's assets: the component icons plus the speaker icon.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = SoundIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(SoundIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn speaker_icon() -> Icon {
    Icon::new(gpui_kit::assets::IconName::Volume2)
}

/// Small speaker button.
pub fn button(id: impl Into<ElementId>, tooltip: &'static str, on_click: impl Fn(&mut App) + 'static) -> Button {
    Button::new(id).ghost().xsmall().icon(speaker_icon()).tooltip(tooltip).on_click(move |_, _, cx| on_click(cx))
}

pub fn play_word(expression: &str, reading: &str, cx: &App) {
    play(None, expression, reading, cx);
}

pub fn play_sentence(sentence: &str, expression: &str, reading: &str, cx: &App) {
    play(Some(sentence.to_string()), expression, reading, cx);
}

fn play(sentence: Option<String>, expression: &str, reading: &str, cx: &App) {
    let id = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    stop();
    let (svc, e, r) = (svc(cx), expression.to_string(), reading.to_string());
    rt(cx).spawn(async move {
        let res = match &sentence {
            None => audio::word(&svc.db, &e, &r).await,
            Some(s) => audio::sentence(&svc.db, s, &e, &r).await,
        };
        match res {
            Ok(a) if LATEST.load(Ordering::SeqCst) == id => play_file(&a.path),
            Ok(_) => {}
            Err(err) => eprintln!("audio for {e}: {err:#}"),
        }
    });
}

pub fn play_file(path: &Path) {
    stop();
    let child = Command::new("/usr/bin/afplay").arg(path).spawn();
    match child {
        Ok(c) => *PLAYING.lock().unwrap() = Some(c),
        Err(e) => eprintln!("afplay: {e}"),
    }
}

pub fn stop() {
    if let Some(mut c) = PLAYING.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}
