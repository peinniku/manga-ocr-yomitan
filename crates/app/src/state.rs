//! App-wide state shared by every window: the core service and the tokio runtime
//! that runs the local HTTP server, LLM calls and model downloads.

use std::sync::Arc;

use gpui_kit::{App, Global};
use picyomi_core::service::Service;

pub struct AppState {
    pub svc: Arc<Service>,
    pub rt: tokio::runtime::Handle,
}

impl Global for AppState {}

pub fn svc(cx: &App) -> Arc<Service> {
    cx.global::<AppState>().svc.clone()
}

pub fn rt(cx: &App) -> tokio::runtime::Handle {
    cx.global::<AppState>().rt.clone()
}

/// Device id used in review logs from this Mac.
pub const DEVICE_ID: &str = "mac";
