use std::collections::BTreeSet;

use windows_sys::Win32::Graphics::Gdi::{
    CDS_TEST, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_BADDUALVIEW,
    DISP_CHANGE_BADFLAGS, DISP_CHANGE_BADMODE, DISP_CHANGE_BADPARAM, DISP_CHANGE_FAILED,
    DISP_CHANGE_NOTUPDATED, DISP_CHANGE_RESTART, DISP_CHANGE_SUCCESSFUL,
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_MIRRORING_DRIVER,
    DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, DM_BITSPERPEL, DM_DISPLAYFREQUENCY,
    DM_DISPLAYORIENTATION, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS,
    EnumDisplayDevicesW, EnumDisplaySettingsExW,
};

#[derive(Clone, Debug)]
pub struct Display {
    pub device_name: String,
    pub monitor_name: String,
    pub primary: bool,
    pub width: u32,
    pub height: u32,
    pub current_hz: u32,
    pub rates: Vec<u32>,
}

pub fn enumerate() -> Result<Vec<Display>, String> {
    let mut displays = Vec::new();
    let mut index = 0u32;

    loop {
        let mut adapter = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..DISPLAY_DEVICEW::default()
        };
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut adapter, 0) } == 0 {
            break;
        }
        index += 1;

        let flags = adapter.StateFlags;
        if flags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0
            || flags & DISPLAY_DEVICE_MIRRORING_DRIVER != 0
        {
            continue;
        }

        let device_name = wide_to_string(&adapter.DeviceName);
        if device_name.is_empty() {
            continue;
        }
        let device = to_wide(&device_name);

        let mut current = blank_devmode();
        if unsafe {
            EnumDisplaySettingsExW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut current, 0)
        } == 0
            || current.dmPelsWidth == 0
            || current.dmPelsHeight == 0
        {
            continue;
        }

        let mut rates = BTreeSet::new();
        let mut mode_index = 0u32;
        loop {
            let mut mode = blank_devmode();
            if unsafe { EnumDisplaySettingsExW(device.as_ptr(), mode_index, &mut mode, 0) } == 0 {
                break;
            }
            mode_index += 1;
            if mode.dmPelsWidth == current.dmPelsWidth
                && mode.dmPelsHeight == current.dmPelsHeight
                && mode.dmBitsPerPel == current.dmBitsPerPel
                && mode.dmDisplayFrequency >= 20
            {
                rates.insert(mode.dmDisplayFrequency);
            }
            if mode_index > 2000 {
                break;
            }
        }

        if current.dmDisplayFrequency >= 20 {
            rates.insert(current.dmDisplayFrequency);
        }

        displays.push(Display {
            monitor_name: monitor_name(&device),
            device_name,
            primary: flags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
            width: current.dmPelsWidth,
            height: current.dmPelsHeight,
            current_hz: current.dmDisplayFrequency,
            rates: rates.into_iter().collect(),
        });
    }

    if displays.is_empty() {
        return Err("没有检测到正在使用的显示器。".into());
    }

    if !displays.iter().any(|display| display.primary) {
        displays[0].primary = true;
    }
    displays.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then_with(|| a.device_name.cmp(&b.device_name))
    });
    Ok(displays)
}

pub fn set_refresh_rate(device_name: &str, hz: u32) -> Result<(), String> {
    let prepared = prepare_mode(device_name, hz)?;
    if prepared.current_hz == hz {
        return Ok(());
    }

    let test = change_mode(&prepared, CDS_TEST);
    if test != DISP_CHANGE_SUCCESSFUL {
        return Err(format!(
            "无法切换到 {hz} Hz（{}）。",
            disp_change_text(test)
        ));
    }

    let applied = change_mode(&prepared, CDS_UPDATEREGISTRY);
    if applied == DISP_CHANGE_SUCCESSFUL || applied == DISP_CHANGE_RESTART {
        return Ok(());
    }

    let dynamic = change_mode(&prepared, 0);
    if dynamic == DISP_CHANGE_SUCCESSFUL || dynamic == DISP_CHANGE_RESTART {
        Ok(())
    } else {
        Err(format!(
            "设置 {hz} Hz 失败（{}）。",
            disp_change_text(dynamic)
        ))
    }
}

struct PreparedMode {
    device: Vec<u16>,
    mode: DEVMODEW,
    current_hz: u32,
}

fn prepare_mode(device_name: &str, hz: u32) -> Result<PreparedMode, String> {
    if hz < 20 {
        return Err("刷新率无效。".into());
    }

    let device = to_wide(device_name);
    let mut current = blank_devmode();
    if unsafe { EnumDisplaySettingsExW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut current, 0) }
        == 0
    {
        return Err("无法读取当前显示模式。".into());
    }

    let mut mode = find_mode(&device, &current, hz).unwrap_or_else(|| {
        let mut fallback = current;
        fallback.dmDisplayFrequency = hz;
        fallback
    });
    mode.Anonymous1 = current.Anonymous1;
    mode.dmFields |= DM_POSITION
        | DM_DISPLAYORIENTATION
        | DM_BITSPERPEL
        | DM_PELSWIDTH
        | DM_PELSHEIGHT
        | DM_DISPLAYFREQUENCY;
    mode.dmDisplayFrequency = hz;
    Ok(PreparedMode {
        device,
        mode,
        current_hz: current.dmDisplayFrequency,
    })
}

fn change_mode(prepared: &PreparedMode, flags: u32) -> i32 {
    unsafe {
        ChangeDisplaySettingsExW(
            prepared.device.as_ptr(),
            &prepared.mode,
            std::ptr::null_mut(),
            flags,
            std::ptr::null(),
        )
    }
}

fn find_mode(device: &[u16], current: &DEVMODEW, hz: u32) -> Option<DEVMODEW> {
    let mut fallback = None;
    let mut mode_index = 0u32;
    loop {
        let mut mode = blank_devmode();
        if unsafe { EnumDisplaySettingsExW(device.as_ptr(), mode_index, &mut mode, 0) } == 0 {
            break;
        }
        mode_index += 1;
        if mode.dmPelsWidth == current.dmPelsWidth
            && mode.dmPelsHeight == current.dmPelsHeight
            && mode.dmDisplayFrequency == hz
        {
            if mode.dmBitsPerPel == current.dmBitsPerPel {
                return Some(mode);
            }
            if fallback.is_none() || mode.dmBitsPerPel >= 32 {
                fallback = Some(mode);
            }
        }
        if mode_index > 2000 {
            break;
        }
    }
    fallback
}

fn monitor_name(adapter: &[u16]) -> String {
    let mut monitor = DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..DISPLAY_DEVICEW::default()
    };
    if unsafe { EnumDisplayDevicesW(adapter.as_ptr(), 0, &mut monitor, 0) } == 0 {
        return String::new();
    }
    let name = wide_to_string(&monitor.DeviceString);
    const GENERIC: &[&str] = &[
        "Generic PnP Monitor",
        "Generic Non-PnP Monitor",
        "Default Monitor",
        "通用即插即用监视器",
        "通用非即插即用监视器",
        "默认监视器",
    ];
    if GENERIC
        .iter()
        .any(|generic| name.eq_ignore_ascii_case(generic) || name == *generic)
    {
        String::new()
    } else {
        name
    }
}

fn blank_devmode() -> DEVMODEW {
    let mut mode = DEVMODEW::default();
    mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    mode
}

fn disp_change_text(code: i32) -> &'static str {
    match code {
        DISP_CHANGE_SUCCESSFUL => "已成功",
        DISP_CHANGE_RESTART => "需要重启后生效",
        DISP_CHANGE_FAILED => "驱动拒绝了这个模式",
        DISP_CHANGE_BADMODE => "显示器不支持这个模式",
        DISP_CHANGE_NOTUPDATED => "无法写入当前设置",
        DISP_CHANGE_BADFLAGS => "参数不被支持",
        DISP_CHANGE_BADPARAM => "参数无效",
        DISP_CHANGE_BADDUALVIEW => "多显示器模式不兼容",
        _ => "未知错误",
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&unit| unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::enumerate;

    #[test]
    fn lists_attached_displays() {
        let displays = enumerate().expect("枚举显示器");
        for display in &displays {
            eprintln!(
                "{} primary={} {}x{} {}Hz rates={:?}",
                display.device_name,
                display.primary,
                display.width,
                display.height,
                display.current_hz,
                display.rates
            );
        }
        assert!(!displays.is_empty());
        let primary = displays
            .iter()
            .find(|display| display.primary)
            .expect("主屏幕");
        assert!(primary.width >= 640, "width {}", primary.width);
        assert!(primary.height >= 480, "height {}", primary.height);
        assert!(primary.current_hz >= 20, "hz {}", primary.current_hz);
        assert!(primary.rates.contains(&primary.current_hz));
    }

    #[test]
    fn listed_rates_pass_driver_test() {
        let displays = enumerate().expect("枚举显示器");
        for display in &displays {
            for hz in &display.rates {
                let prepared = super::prepare_mode(&display.device_name, *hz)
                    .unwrap_or_else(|err| panic!("{} {hz} Hz: {err}", display.device_name));
                let result = super::change_mode(&prepared, super::CDS_TEST);
                assert_eq!(
                    result,
                    super::DISP_CHANGE_SUCCESSFUL,
                    "{} {hz} Hz 未被驱动接受（{}）",
                    display.device_name,
                    super::disp_change_text(result)
                );
            }
        }
    }
}
