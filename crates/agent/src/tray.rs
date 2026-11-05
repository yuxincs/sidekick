use crate::startup::{NativeStartup, StartupControl};
use anyhow::Result;
use host_agent_core::{connection::Connector, service::Agent};
use std::sync::Arc;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem},
};
pub struct Tray {
    agent: Arc<Agent>,
    startup: Result<NativeStartup>,
    _icon: TrayIcon,
    login: CheckMenuItem,
    cpu: CheckMenuItem,
    memory: CheckMenuItem,
    shutdown: CheckMenuItem,
    audio: CheckMenuItem,
    connection: MenuItem,
    quit: MenuItem,
    status: MenuItem,
}

#[derive(PartialEq)]
pub enum Event {
    None,
    Settings,
    Quit,
}

impl Tray {
    pub fn startup_enabled(&self) -> Result<bool> {
        self.startup
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .is_enabled()
    }
    pub fn set_startup(&self, enabled: bool) -> Result<()> {
        self.startup
            .as_ref()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .set_enabled(enabled)?;
        self.login.set_checked(enabled);
        Ok(())
    }
    pub fn sync(&self, connector: &Connector) {
        self.status.set_text(format!(
            "Sidekick · {}",
            connector.subscribe().borrow().phase
        ));
        if let Ok(enabled) = self.startup_enabled() {
            self.login.set_checked(enabled);
        }
        let exposure = self.agent.exposure();
        self.cpu.set_checked(exposure.cpu);
        self.memory.set_checked(exposure.memory);
        self.shutdown.set_checked(exposure.shutdown);
        self.audio.set_checked(exposure.audio);
    }

    pub fn handle(&self, event: MenuEvent) -> Event {
        let ui = self;
        if event.id == *ui.quit.id() {
            return Event::Quit;
        }
        if event.id == *ui.login.id() {
            let result = self
                .startup
                .as_ref()
                .map_err(|e| anyhow::anyhow!(e.to_string()))
                .and_then(|startup| startup.set_enabled(ui.login.is_checked()));
            match result {
                Ok(()) => ui.status.set_text("Login preference saved"),
                Err(error) => {
                    ui.status
                        .set_text(format!("Could not change login startup: {error}"));
                    ui.login.set_checked(
                        self.startup
                            .as_ref()
                            .ok()
                            .and_then(|s| s.is_enabled().ok())
                            .unwrap_or(false),
                    );
                }
            }
            return Event::None;
        }
        if event.id == *ui.connection.id() {
            return Event::Settings;
        }
        if event.id != *ui.cpu.id()
            && event.id != *ui.memory.id()
            && event.id != *ui.shutdown.id()
            && event.id != *ui.audio.id()
        {
            return Event::None;
        }
        let mut next = self.agent.exposure();
        if event.id == *ui.cpu.id() {
            next.cpu = !next.cpu;
        }
        if event.id == *ui.memory.id() {
            next.memory = !next.memory;
        }
        if event.id == *ui.audio.id() {
            next.audio = !next.audio;
        }
        if event.id == *ui.shutdown.id() {
            next.shutdown = !next.shutdown;
        }
        if let Err(error) = self.agent.set_exposure(next) {
            ui.status.set_text(format!("Settings not saved: {error}"));
            let old = self.agent.exposure();
            ui.cpu.set_checked(old.cpu);
            ui.memory.set_checked(old.memory);
            ui.shutdown.set_checked(old.shutdown);
            ui.audio.set_checked(old.audio);
        } else {
            ui.status.set_text("Settings saved");
            ui.cpu.set_checked(next.cpu);
            ui.memory.set_checked(next.memory);
            ui.shutdown.set_checked(next.shutdown);
            ui.audio.set_checked(next.audio);
        }
        Event::None
    }

    pub fn new(agent: Arc<Agent>) -> Result<Self> {
        let startup = NativeStartup::new();
        let menu = Menu::new();
        let exposure = agent.exposure();
        let status = MenuItem::new("Sidekick — local settings", false, None);
        let enabled = startup
            .as_ref()
            .ok()
            .and_then(|s| s.is_enabled().ok())
            .unwrap_or(false);
        let login = CheckMenuItem::new("Start at login", true, enabled, None);
        let cpu = CheckMenuItem::new("Expose CPU usage", true, exposure.cpu, None);
        let memory = CheckMenuItem::new("Expose RAM usage", true, exposure.memory, None);
        let shutdown = CheckMenuItem::new("Allow remote shutdown", true, exposure.shutdown, None);
        let audio = CheckMenuItem::new("Allow speaker mute/unmute", true, exposure.audio, None);
        let connection = MenuItem::new("Connection settings…", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[
            &status,
            &connection,
            &login,
            &cpu,
            &memory,
            &shutdown,
            &audio,
            &quit,
        ])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Sidekick")
            .with_menu(Box::new(menu))
            .with_icon(Icon::from_rgba(icon_rgba(), 32, 32)?)
            .build()?;
        Ok(Self {
            agent,
            startup,
            _icon: icon,
            login,
            cpu,
            memory,
            shutdown,
            audio,
            connection,
            quit,
            status,
        })
    }
}

pub fn icon_rgba() -> Vec<u8> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for y in 3..29 {
        for x in 3..29 {
            let i = (y * 32 + x) * 4;
            let bright = ((7..25).contains(&x)
                && ((7..11).contains(&y) || (14..18).contains(&y) || (21..25).contains(&y)))
                || ((7..11).contains(&x) && (7..18).contains(&y))
                || ((21..25).contains(&x) && (14..25).contains(&y));
            rgba[i..i + 4].copy_from_slice(if bright {
                &[255, 255, 255, 255]
            } else {
                &[32, 130, 220, 255]
            });
        }
    }
    rgba
}
