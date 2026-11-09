#![cfg_attr(not(test), windows_subsystem = "windows")]

mod display;
mod icon;

use std::sync::atomic::{AtomicUsize, Ordering};

use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW,
    MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, PostQuitMessage, QS_ALLINPUT,
    TranslateMessage, WM_QUIT,
};

use crate::display::Display;

const TRAY_GUID: u128 = 0xA3C1_5E70_4B2D_4F18_8C6A_91D2_E4B7_053A;
static INSTANCE_LOCK: AtomicUsize = AtomicUsize::new(0);

fn main() {
    enable_dpi_awareness();
    if !acquire_single_instance() {
        return;
    }

    let initial_hz = display::enumerate()
        .ok()
        .and_then(|displays| {
            displays
                .into_iter()
                .find(|display| display.primary)
                .map(|display| display.current_hz)
        })
        .filter(|hz| *hz >= 20)
        .unwrap_or(0);

    let menu = Menu::new();
    let tray = TrayIconBuilder::new()
        .with_guid(TRAY_GUID)
        .with_menu(Box::new(menu))
        .with_tooltip("HzSetter")
        .with_icon(icon::hz_icon(initial_hz))
        .with_menu_on_left_click(true)
        .with_menu_on_right_click(true)
        .build();
    let tray = match tray {
        Ok(tray) => tray,
        Err(err) => {
            show_message(&format!("无法创建托盘图标：{err}"), MB_OK | MB_ICONERROR);
            return;
        }
    };

    let mut shown_hz = (initial_hz >= 20).then_some(initial_hz);
    let mut fingerprint = String::new();
    sync_tray(&tray, &mut shown_hz, &mut fingerprint);

    loop {
        unsafe {
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 800, QS_ALLINPUT);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "quit" => {
                    unsafe { PostQuitMessage(0) };
                }
                id => {
                    if let Some((device, hz)) = parse_rate_id(id) {
                        if let Err(err) = display::set_refresh_rate(&device, hz) {
                            show_message(&err, MB_OK | MB_ICONERROR);
                        }
                        fingerprint.clear();
                        sync_tray(&tray, &mut shown_hz, &mut fingerprint);
                    }
                }
            }
        }

        sync_tray(&tray, &mut shown_hz, &mut fingerprint);
    }
}

fn sync_tray(tray: &TrayIcon, shown_hz: &mut Option<u32>, fingerprint: &mut String) {
    let displays = match display::enumerate() {
        Ok(displays) => displays,
        Err(err) => {
            if fingerprint.as_str() != "error" {
                let menu = Menu::new();
                let info = MenuItem::with_id("info", err, false, None);
                let quit = MenuItem::with_id("quit", "退出", true, None);
                let _ = menu.append(&info);
                let _ = menu.append(&PredefinedMenuItem::separator());
                let _ = menu.append(&quit);
                tray.set_menu(Some(Box::new(menu)));
                let _ = tray.set_tooltip(Some("HzSetter · 没有检测到显示器"));
                if shown_hz.is_some() {
                    let _ = tray.set_icon(Some(icon::hz_icon(0)));
                    *shown_hz = None;
                }
                *fingerprint = "error".to_string();
            }
            return;
        }
    };

    let next = fingerprint_of(&displays);
    let icon_hz = displays
        .iter()
        .find(|display| display.primary)
        .map(|display| display.current_hz);
    if next == *fingerprint {
        return;
    }

    tray.set_menu(Some(Box::new(build_menu(&displays))));
    let _ = tray.set_tooltip(Some(tooltip_of(&displays)));
    if *shown_hz != icon_hz {
        let _ = tray.set_icon(Some(icon::hz_icon(icon_hz.unwrap_or(0))));
        *shown_hz = icon_hz;
    }
    *fingerprint = next;
}

fn build_menu(displays: &[Display]) -> Menu {
    let menu = Menu::new();
    if displays.len() == 1 {
        append_rates(&menu, &displays[0], true);
    } else {
        let mut secondary = 1usize;
        for display in displays {
            let label = if display.primary {
                "主屏幕".to_string()
            } else {
                let label = format!("屏幕 {secondary}");
                secondary += 1;
                label
            };
            let title = format!("{label} · {} Hz", display.current_hz);
            let submenu = Submenu::new(escape_mnemonic(&title), true);
            append_rates(&submenu, display, false);
            let _ = menu.append(&submenu);
        }
    }
    let quit = MenuItem::with_id("quit", "退出", true, None);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);
    menu
}

fn append_rates(menu: &impl AppendMenu, display: &Display, show_current: bool) {
    if show_current {
        let info = MenuItem::with_id("info", escape_mnemonic(&current_text(display)), false, None);
        menu.push(&info);
        menu.push(&PredefinedMenuItem::separator());
    } else {
        let detail = detail_text(display);
        let info = MenuItem::with_id(
            format!("info-{}", display.device_name),
            escape_mnemonic(&detail),
            false,
            None,
        );
        menu.push(&info);
        menu.push(&PredefinedMenuItem::separator());
    }

    if display.rates.is_empty() {
        let empty = MenuItem::with_id(
            format!("empty-{}", display.device_name),
            "没有可选刷新率",
            false,
            None,
        );
        menu.push(&empty);
    } else {
        for hz in &display.rates {
            let item = CheckMenuItem::with_id(
                format!("hz|{}|{hz}", display.device_name),
                format!("{hz} Hz"),
                true,
                *hz == display.current_hz,
                None,
            );
            menu.push(&item);
        }
    }
}

fn current_text(display: &Display) -> String {
    let mut text = format!(
        "当前 {} Hz · {}×{}",
        display.current_hz, display.width, display.height
    );
    if !display.monitor_name.is_empty() {
        text.push_str(" · ");
        text.push_str(&display.monitor_name);
    }
    text
}

fn detail_text(display: &Display) -> String {
    let mut text = format!("{}×{}", display.width, display.height);
    if !display.monitor_name.is_empty() {
        text.push_str(" · ");
        text.push_str(&display.monitor_name);
    }
    text
}

fn tooltip_of(displays: &[Display]) -> String {
    if displays.len() == 1 {
        let display = &displays[0];
        return format!(
            "HzSetter · {} Hz · {}×{}",
            display.current_hz, display.width, display.height
        );
    }
    let mut secondary = 1usize;
    let parts: Vec<String> = displays
        .iter()
        .map(|display| {
            let label = if display.primary {
                "主屏幕".to_string()
            } else {
                let label = format!("屏幕 {secondary}");
                secondary += 1;
                label
            };
            format!("{label} {} Hz", display.current_hz)
        })
        .collect();
    format!("HzSetter · {}", parts.join(" · "))
}

fn fingerprint_of(displays: &[Display]) -> String {
    let mut text = String::new();
    for display in displays {
        text.push_str(&display.device_name);
        text.push('|');
        text.push_str(&display.current_hz.to_string());
        text.push('|');
        text.push_str(&display.width.to_string());
        text.push('x');
        text.push_str(&display.height.to_string());
        text.push('|');
        text.push_str(&display.monitor_name);
        text.push('|');
        for hz in &display.rates {
            text.push_str(&hz.to_string());
            text.push(',');
        }
        text.push(';');
    }
    text
}

fn parse_rate_id(id: &str) -> Option<(String, u32)> {
    let rest = id.strip_prefix("hz|")?;
    let (device, hz) = rest.rsplit_once('|')?;
    let hz = hz.parse().ok()?;
    if device.is_empty() {
        return None;
    }
    Some((device.to_string(), hz))
}

fn escape_mnemonic(text: &str) -> String {
    text.replace('&', "&&")
}

trait AppendMenu {
    fn push(&self, item: &dyn IsMenuItem);
}

impl AppendMenu for Menu {
    fn push(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}

impl AppendMenu for Submenu {
    fn push(&self, item: &dyn IsMenuItem) {
        let _ = self.append(item);
    }
}

fn enable_dpi_awareness() {
    // 先声明 DPI 感知，才能拿到托盘槽的真实像素尺寸，按 1:1 绘制。
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn acquire_single_instance() -> bool {
    let name = to_wide("Local\\HzSetter.SingleInstance");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if handle.is_null() {
        return true;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        show_message("HzSetter 已在运行。", MB_OK | MB_ICONINFORMATION);
        return false;
    }
    // 句柄在进程退出前保持打开，命名互斥量才不会被释放。
    INSTANCE_LOCK.store(handle as usize, Ordering::Relaxed);
    true
}

fn show_message(text: &str, flags: u32) {
    let text = to_wide(text);
    let title = to_wide("HzSetter");
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags);
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
