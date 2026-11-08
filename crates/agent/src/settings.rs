//! Shared native desktop UI. No HTTP listener, webview, or browser process.
mod theme;

use crate::tray::{Event, Tray};
use anyhow::{Result, ensure};
use eframe::egui::{self, RichText, ViewportCommand};
use host_agent_core::{
    connection::{Connector, endpoint},
    service::Agent,
};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};
use theme::{Palette, caption, card, divider, switch_row};
use tokio::runtime::Handle;
use tray_icon::menu::MenuEvent;

#[derive(Default)]
struct PairForm {
    url: String,
    code: String,
}
impl PairForm {
    fn request(&self) -> Result<(String, String)> {
        let url = self.url.trim().to_owned();
        endpoint(&url, "/api/sidekick/pair", false)?;
        let code = self.code.trim().replace('-', "").to_ascii_uppercase();
        ensure!(
            code.len() == 16
                && code
                    .bytes()
                    .all(|c| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&c)),
            "Enter the 16-character pairing code shown in Home Assistant"
        );
        Ok((url, code))
    }
}

pub fn run(agent: Arc<Agent>, connector: Arc<Connector>, runtime: Handle) -> Result<()> {
    let show = agent.enrollment().is_none();
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Sidekick Settings")
            .with_inner_size([640.0, 820.0])
            .with_min_inner_size([460.0, 600.0])
            .with_visible(show)
            .with_icon(Arc::new(egui::IconData {
                rgba: crate::tray::icon_rgba(),
                width: 32,
                height: 32,
            })),
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                builder.with_activation_policy(ActivationPolicy::Accessory);
            }
            let _ = builder;
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Sidekick",
        options,
        Box::new(move |cc| {
            theme::configure(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            let (tx, menu) = mpsc::channel();
            MenuEvent::set_event_handler(Some(move |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            }));
            let (results_tx, results) = mpsc::channel();
            let mut form = PairForm::default();
            if let Some(enrollment) = agent.enrollment() {
                form.url = enrollment.url;
            }
            // Creation runs on the event loop's main thread, after its native window exists.
            let tray = Tray::new(agent.clone())?;
            let mut snapshots = agent.subscribe();
            let repaint_ctx = cc.egui_ctx.clone();
            let repaint = runtime.spawn(async move {
                while snapshots.changed().await.is_ok() {
                    repaint_ctx.request_repaint();
                }
            });
            Ok(Box::new(Settings {
                repaint,
                agent,
                connector,
                runtime,
                tray,
                menu,
                results_tx,
                results,
                form,
                busy: false,
                error: None,
                quit: false,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Could not open native settings: {e}"))
}

struct Settings {
    repaint: tokio::task::JoinHandle<()>,
    agent: Arc<Agent>,
    connector: Arc<Connector>,
    runtime: Handle,
    tray: Tray,
    menu: mpsc::Receiver<MenuEvent>,
    results_tx: mpsc::Sender<Result<()>>,
    results: mpsc::Receiver<Result<()>>,
    form: PairForm,
    busy: bool,
    error: Option<String>,
    quit: bool,
}

impl eframe::App for Settings {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        while let Ok(event) = self.menu.try_recv() {
            match self.tray.handle(event) {
                Event::Settings => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                Event::Quit => {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                Event::None => {}
            }
        }
        while let Ok(result) = self.results.try_recv() {
            self.busy = false;
            self.error = result.err().map(|e| e.to_string());
        }
        self.tray.sync(&self.connector);
        if !self.quit && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let p = Palette::new(ui.visuals().dark_mode);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.background).inner_margin(24))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    theme::header(ui, p);
                    ui.add_space(20.0);
                    self.connection_card(ui, p);
                    ui.add_space(14.0);
                    self.permissions_card(ui, p);
                    ui.add_space(14.0);
                    self.startup_card(ui, p);
                    if let Some(error) = &self.error {
                        ui.add_space(14.0);
                        egui::Frame::new().fill(p.card).stroke(egui::Stroke::new(1.0, p.danger))
                            .corner_radius(10).inner_margin(14).show(ui, |ui| {
                                ui.colored_label(p.danger, error);
                            });
                    }
                    ui.add_space(16.0);
                    caption(ui, "Changes save automatically. Closing settings keeps Sidekick in the tray.", p);
                });
            });
    }
}

impl Settings {
    fn connection_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        let status = self.connector.subscribe().borrow().clone();
        let paired = self.agent.enrollment().is_some();
        card(ui, p, |ui| {
            let (label, color) = match status.phase.as_str() {
                "connected" => ("Connected", p.positive),
                "offline" => ("Reconnecting", p.caution),
                "approval" => ("Confirm pairing", p.accent),
                "pairing" | "connecting" => ("Connecting", p.accent),
                "error" => ("Needs attention", p.danger),
                _ => ("Not paired", p.muted),
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Home Assistant").size(18.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(p.soft)
                        .corner_radius(20)
                        .inner_margin(egui::Margin::symmetric(10, 5))
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("●  {label}")).size(12.0).color(color));
                        });
                });
            });
            ui.add_space(4.0);
            caption(ui, &status.message, p);
            if let Some(url) = &status.url {
                ui.label(RichText::new(url).size(12.0).color(p.muted));
            }
            if let Some(comparison) = &status.comparison {
                ui.add_space(12.0);
                egui::Frame::new()
                    .fill(p.soft)
                    .corner_radius(10)
                    .inner_margin(16)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.vertical_centered(|ui| {
                            caption(ui, "CONFIRM THIS NUMBER", p);
                            ui.label(RichText::new(comparison).monospace().size(34.0).strong());
                            caption(
                                ui,
                                "Approve in Home Assistant only if the numbers match.",
                                p,
                            );
                        });
                    });
            }
            ui.add_space(10.0);
            if paired {
                egui::CollapsingHeader::new("Pair again")
                    .id_salt("pair_again")
                    .show(ui, |ui| self.pair_fields(ui, p));
                ui.add_space(6.0);
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Disconnect computer"))
                    .clicked()
                {
                    self.launch(ui.ctx(), |c| async move { c.disconnect().await });
                }
            } else {
                self.pair_fields(ui, p);
            }
            if self.busy {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    caption(ui, "Waiting for Home Assistant…", p);
                });
                caption(ui, "You can cancel this pairing in Home Assistant.", p);
            }
        });
    }

    fn pair_fields(&mut self, ui: &mut egui::Ui, p: Palette) {
        caption(
            ui,
            "Add Sidekick in Home Assistant to get a pairing code.",
            p,
        );
        ui.add_space(6.0);
        ui.add_enabled_ui(!self.busy, |ui| {
            caption(ui, "Home Assistant address", p);
            ui.add(
                egui::TextEdit::singleline(&mut self.form.url)
                    .hint_text("https://homeassistant.example:8123")
                    .margin(egui::vec2(10.0, 9.0))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            caption(ui, "Pairing code", p);
            ui.add(
                egui::TextEdit::singleline(&mut self.form.code)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("XXXX-XXXX-XXXX-XXXX")
                    .margin(egui::vec2(10.0, 9.0))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("Pair this computer").color(p.on_accent))
                        .fill(p.accent)
                        .min_size(egui::vec2(0.0, 38.0)),
                )
                .clicked()
            {
                match self.form.request() {
                    Ok((url, code)) => {
                        self.form.code.clear();
                        self.launch(ui.ctx(), move |c| async move { c.pair(url, code).await });
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        });
        ui.add_space(6.0);
        caption(ui, "No Home Assistant password or API token needed.", p);
    }

    fn permissions_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        card(ui, p, |ui| {
            ui.label(RichText::new("Sharing & controls").size(18.0).strong());
            caption(ui, "Choose what Home Assistant can see and do.", p);
            ui.add_space(12.0);
            let mut exposure = self.agent.exposure();
            let mut changed = switch_row(
                ui,
                "CPU usage",
                "Share your processor's current usage.",
                &mut exposure.cpu,
                p,
            );
            divider(ui);
            changed |= switch_row(
                ui,
                "Memory usage",
                "Share how much RAM is in use.",
                &mut exposure.memory,
                p,
            );
            divider(ui);
            changed |= switch_row(
                ui,
                "Speaker mute",
                "Allow mute/unmute and share the current mute state.",
                &mut exposure.audio,
                p,
            );
            if exposure.audio {
                let muted = self.agent.subscribe().borrow().metrics.audio_muted;
                ui.add_space(4.0);
                caption(
                    ui,
                    match muted {
                        Some(true) => "Currently muted",
                        Some(false) => "Currently unmuted",
                        None => "Waiting for a supported audio output…",
                    },
                    p,
                );
                #[cfg(target_os = "linux")]
                caption(
                    ui,
                    "Requires wpctl + pw-dump (PipeWire) or pactl (PulseAudio).",
                    p,
                );
            }
            divider(ui);
            changed |= switch_row(
                ui,
                "Remote shutdown",
                "Allow Home Assistant to turn off this computer.",
                &mut exposure.shutdown,
                p,
            );
            if changed && let Err(e) = self.agent.set_exposure(exposure) {
                self.error = Some(format!("Settings not saved: {e}"));
            }
        });
    }

    fn startup_card(&mut self, ui: &mut egui::Ui, p: Palette) {
        card(ui, p, |ui| {
            ui.label(RichText::new("On this computer").size(18.0).strong());
            ui.add_space(10.0);
            match self.tray.startup_enabled() {
                Ok(mut enabled) => {
                    if switch_row(
                        ui,
                        "Launch at login",
                        "Keep Sidekick ready when you sign in.",
                        &mut enabled,
                        p,
                    ) && let Err(e) = self.tray.set_startup(enabled)
                    {
                        self.error = Some(e.to_string());
                    }
                }
                Err(e) => {
                    ui.colored_label(p.caution, format!("Startup settings unavailable: {e}"));
                }
            }
        });
    }

    fn launch<F, Fut>(&mut self, ctx: &egui::Context, operation: F)
    where
        F: FnOnce(Arc<Connector>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.busy = true;
        self.error = None;
        let connector = self.connector.clone();
        let tx = self.results_tx.clone();
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let result = operation(connector).await;
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        self.repaint.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairing_form_validates_before_network_or_credential_changes() {
        let mut form = PairForm {
            url: " https://ha.local:8123/ ".into(),
            code: "abcd-efgh-jkmn-pqrs".into(),
        };
        assert_eq!(
            form.request().unwrap(),
            ("https://ha.local:8123/".into(), "ABCDEFGHJKMNPQRS".into())
        );
        for address in [
            "http://ha.local",
            "https://user:password@ha.local",
            "https://ha.local/?token=secret",
        ] {
            form.url = address.into();
            assert!(form.request().is_err());
        }
        form.url = "https://ha.local".into();
        for code in ["", "1234", "IIII-IIII-IIII-IIII", "<script>alert(1)"] {
            form.code = code.into();
            assert!(form.request().is_err());
        }
    }
}
