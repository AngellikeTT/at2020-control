//! System-tray icon and global hotkeys. Both deliver their events from the UI thread's
//! message loop through handlers; we forward them over a channel and wake the UI.

use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ToggleOutputMute,
    ToggleBypass,
    ToggleMicMute,
    ShowWindow,
    Quit,
}

pub struct Integration {
    tray: Option<TrayIcon>,
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    hotkey_ids: HashMap<u32, Action>,
    menu_ids: HashMap<tray_icon::menu::MenuId, Action>,
    rx: Receiver<Action>,
    shared_ids: Option<std::sync::Arc<Mutex<HashMap<u32, Action>>>>,
    pub hotkey_errors: Vec<String>,
}

impl Integration {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel::<Action>();
        let mut me = Self {
            tray: None,
            manager: None,
            registered: Vec::new(),
            hotkey_ids: HashMap::new(),
            menu_ids: HashMap::new(),
            rx,
            shared_ids: None,
            hotkey_errors: Vec::new(),
        };

        // Tray menu.
        let menu = Menu::new();
        let items = [
            ("Show control panel", Action::ShowWindow),
            ("Mute / unmute output", Action::ToggleOutputMute),
            ("Bypass chain on / off", Action::ToggleBypass),
            ("Mute / unmute microphone", Action::ToggleMicMute),
        ];
        for (label, action) in items {
            let it = MenuItem::new(label, true, None);
            me.menu_ids.insert(it.id().clone(), action);
            let _ = menu.append(&it);
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
        let quit = MenuItem::new("Quit", true, None);
        me.menu_ids.insert(quit.id().clone(), Action::Quit);
        let _ = menu.append(&quit);

        let rgba = crate::icon::rgba(32);
        let icon = tray_icon::Icon::from_rgba(rgba, 32, 32).ok();
        let mut b = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("AT2020USB-X Control Panel");
        if let Some(i) = icon {
            b = b.with_icon(i);
        }
        me.tray = b.build().ok();

        let menu_ids = me.menu_ids.clone();
        let (txm, ctxm) = (Mutex::new(tx.clone()), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some(a) = menu_ids.get(&e.id) {
                if let Ok(tx) = txm.lock() {
                    let _ = tx.send(*a);
                }
                ctxm.request_repaint();
            }
        }));
        let (txt, ctxt) = (Mutex::new(tx.clone()), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = e
            {
                if let Ok(tx) = txt.lock() {
                    let _ = tx.send(Action::ShowWindow);
                }
                ctxt.request_repaint();
            }
        }));

        // Global hotkeys.
        me.manager = GlobalHotKeyManager::new().ok();
        let (txh, ctxh) = (Mutex::new(tx), ctx.clone());
        let ids: std::sync::Arc<Mutex<HashMap<u32, Action>>> =
            std::sync::Arc::new(Mutex::new(HashMap::new()));
        let ids2 = ids.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state() != HotKeyState::Pressed {
                return;
            }
            let a = ids2.lock().ok().and_then(|m| m.get(&e.id()).copied());
            if let Some(a) = a {
                if let Ok(tx) = txh.lock() {
                    let _ = tx.send(a);
                }
                ctxh.request_repaint();
            }
        }));
        me.shared_ids = Some(ids);
        me
    }

    /// (Re)registers hotkeys. Empty strings are skipped; failures are reported.
    pub fn set_hotkeys(&mut self, enabled: bool, binds: &[(&str, Action)]) {
        self.hotkey_errors.clear();
        if let Some(m) = &self.manager {
            for hk in self.registered.drain(..) {
                let _ = m.unregister(hk);
            }
        }
        self.hotkey_ids.clear();
        if let Some(ids) = &self.shared_ids {
            if let Ok(mut g) = ids.lock() {
                g.clear();
            }
        }
        if !enabled {
            return;
        }
        let Some(m) = &self.manager else {
            self.hotkey_errors
                .push("Global hotkeys are unavailable on this system.".into());
            return;
        };
        for (text, action) in binds {
            let t = text.trim();
            if t.is_empty() {
                continue;
            }
            match HotKey::from_str(t) {
                Ok(hk) => match m.register(hk) {
                    Ok(()) => {
                        self.hotkey_ids.insert(hk.id(), *action);
                        if let Some(ids) = &self.shared_ids {
                            if let Ok(mut g) = ids.lock() {
                                g.insert(hk.id(), *action);
                            }
                        }
                        self.registered.push(hk);
                    }
                    Err(e) => self.hotkey_errors.push(format!(
                        "\"{t}\" could not be registered ({e}); it may be in use."
                    )),
                },
                Err(e) => self
                    .hotkey_errors
                    .push(format!("\"{t}\" is not a valid hotkey ({e}).")),
            }
        }
    }

    pub fn poll(&self) -> Vec<Action> {
        self.rx.try_iter().collect()
    }

    pub fn set_tooltip(&self, text: &str) {
        if let Some(t) = &self.tray {
            let _ = t.set_tooltip(Some(text));
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }
}
