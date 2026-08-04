use std::sync::Mutex;

use tauri::menu::MenuItem;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    En,
    Zh,
}

impl Locale {
    pub fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("zh") || value.eq_ignore_ascii_case("zh-cn") {
            Self::Zh
        } else {
            Self::En
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Zh => "zh",
        }
    }
}

pub struct UiLocale {
    locale: Mutex<Locale>,
    show: Mutex<Option<MenuItem<tauri::Wry>>>,
    start: Mutex<Option<MenuItem<tauri::Wry>>>,
    stop: Mutex<Option<MenuItem<tauri::Wry>>>,
    quit: Mutex<Option<MenuItem<tauri::Wry>>>,
}

impl Default for UiLocale {
    fn default() -> Self {
        Self::new()
    }
}

impl UiLocale {
    pub fn new() -> Self {
        Self {
            locale: Mutex::new(Locale::En),
            show: Mutex::new(None),
            start: Mutex::new(None),
            stop: Mutex::new(None),
            quit: Mutex::new(None),
        }
    }

    pub fn current(&self) -> Locale {
        *self.locale.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn set_menu_items(
        &self,
        show: MenuItem<tauri::Wry>,
        start: MenuItem<tauri::Wry>,
        stop: MenuItem<tauri::Wry>,
        quit: MenuItem<tauri::Wry>,
    ) {
        *self.show.lock().unwrap_or_else(|e| e.into_inner()) = Some(show);
        *self.start.lock().unwrap_or_else(|e| e.into_inner()) = Some(start);
        *self.stop.lock().unwrap_or_else(|e| e.into_inner()) = Some(stop);
        *self.quit.lock().unwrap_or_else(|e| e.into_inner()) = Some(quit);
    }

    pub fn set_locale(&self, locale: Locale) {
        *self.locale.lock().unwrap_or_else(|e| e.into_inner()) = locale;
        let _ = self.apply_menu();
    }

    pub fn apply_menu(&self) -> Result<(), String> {
        let locale = self.current();
        if let Some(item) = self.show.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            item.set_text(tr(locale, "show")).map_err(|e| e.to_string())?;
        }
        if let Some(item) = self.start.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            item.set_text(tr(locale, "start")).map_err(|e| e.to_string())?;
        }
        if let Some(item) = self.stop.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            item.set_text(tr(locale, "stop")).map_err(|e| e.to_string())?;
        }
        if let Some(item) = self.quit.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            item.set_text(tr(locale, "quit")).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

pub fn tr(locale: Locale, key: &str) -> &'static str {
    match (locale, key) {
        (Locale::Zh, "show") => "显示面板",
        (Locale::Zh, "start") => "开始同步",
        (Locale::Zh, "stop") => "停止同步",
        (Locale::Zh, "quit") => "退出",
        (Locale::Zh, "running") => "运行中",
        (Locale::Zh, "running_no_peers") => "运行中（无对端）",
        (Locale::Zh, "stopped") => "已停止",
        (_, "show") => "Show panel",
        (_, "start") => "Start sync",
        (_, "stop") => "Stop sync",
        (_, "quit") => "Quit",
        (_, "running") => "Running",
        (_, "running_no_peers") => "Running (no peers)",
        (_, "stopped") => "Stopped",
        _ => "",
    }
}
