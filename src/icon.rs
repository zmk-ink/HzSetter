use tray_icon::Icon;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, ICONINFO, SM_CXSMICON,
};

const TOP: u8 = 1 << 0;
const UPPER_RIGHT: u8 = 1 << 1;
const LOWER_RIGHT: u8 = 1 << 2;
const BOTTOM: u8 = 1 << 3;
const LOWER_LEFT: u8 = 1 << 4;
const UPPER_LEFT: u8 = 1 << 5;
const MIDDLE: u8 = 1 << 6;

pub fn hz_icon(hz: u32) -> Icon {
    let size = tray_size();
    let pixels = render(hz, size);
    if let Some(handle) = hicon_with_alpha(&pixels, size) {
        return Icon::from_handle(handle);
    }
    let mut pixels = pixels;
    premultiply(&mut pixels);
    Icon::from_rgba(pixels, size, size).expect("刷新率图标尺寸正确")
}

fn tray_size() -> u32 {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if size < 16 { 16 } else { size as u32 }
}

fn render(hz: u32, size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let background = background_color(hz);
    let radius = (size * 3 / 16).clamp(2, size / 2);
    for y in 0..size {
        for x in 0..size {
            let alpha = cover(x, y, radius, size);
            if alpha == 0 {
                continue;
            }
            let mut color = background;
            color[3] = alpha;
            put(&mut pixels, size, x, y, color);
        }
    }

    let text = if hz >= 20 {
        hz.to_string()
    } else {
        "--".to_string()
    };
    draw_text(&mut pixels, size, radius, text.as_bytes(), [255, 255, 255, 255]);
    pixels
}

fn background_color(hz: u32) -> [u8; 4] {
    let [r, g, b] = if hz == 0 {
        [104, 110, 118]
    } else {
        let to_25 = distance_to_step(hz, 25);
        let to_30 = distance_to_step(hz, 30);
        // 相差不超过 2 才算倍数：24、48、49 跟 25，30、59、90 跟 30。144 两边都差 6。
        const NEAR: u32 = 2;
        if to_25 <= NEAR && to_25 < to_30 {
            [138, 104, 72]
        } else if to_30 <= NEAR {
            [82, 108, 138]
        } else {
            [86, 120, 102]
        }
    };
    [r, g, b, 255]
}

fn distance_to_step(hz: u32, step: u32) -> u32 {
    let remainder = hz % step;
    remainder.min(step - remainder)
}

fn draw_text(pixels: &mut [u8], size: u32, radius: u32, text: &[u8], color: [u8; 4]) {
    let len = text.len() as u32;
    if len == 0 || size < 8 {
        return;
    }
    // 字号按三位定死，60 和 144 用同一套格子和笔画。
    let Some(layout) = layout_digits(size, radius, len.max(3)) else {
        return;
    };
    let content_w = len * layout.digit_w + len.saturating_sub(1) * layout.gap;
    let origin_x = (size - content_w) / 2;
    for (index, &ch) in text.iter().enumerate() {
        let x = origin_x + index as u32 * (layout.digit_w + layout.gap);
        draw_digit(
            pixels,
            size,
            ch,
            x,
            layout.origin_y,
            layout.digit_w,
            layout.digit_h,
            layout.thickness,
            color,
        );
    }
}

struct Layout {
    origin_y: u32,
    digit_w: u32,
    digit_h: u32,
    gap: u32,
    thickness: u32,
}

fn layout_digits(size: u32, radius: u32, len: u32) -> Option<Layout> {
    let pad = content_pad(size, radius);
    let avail_w = size.saturating_sub(pad * 2);
    let avail_h = size.saturating_sub(pad * 2);
    if avail_w < len || avail_h < 5 {
        return None;
    }

    let mut best: Option<(u32, u32, u32, u32, u32, u32)> = None;
    let max_gap = if len <= 1 { 0 } else { avail_w / len };
    for gap in 0..=max_gap {
        if len > 1 && gap == 0 {
            continue;
        }
        let used = gap * len.saturating_sub(1);
        if used >= avail_w {
            continue;
        }
        let digit_w = (avail_w - used) / len;
        if digit_w < 3 {
            continue;
        }
        let thickness = stroke_width(digit_w, avail_h);
        let hole = digit_w.saturating_sub(thickness * 2);
        let candidate = (thickness, hole, gap, digit_w, avail_h, used);
        if best.map(|current| candidate > current).unwrap_or(true) {
            best = Some(candidate);
        }
    }

    let (thickness, _, gap, digit_w, digit_h, _) = best?;
    Some(Layout {
        origin_y: (size - digit_h) / 2,
        digit_w,
        digit_h,
        gap,
        thickness,
    })
}

fn content_pad(size: u32, radius: u32) -> u32 {
    let mut pad = 0;
    while pad < size / 2 && cover(pad, pad, radius, size) < 200 {
        pad += 1;
    }
    pad.max(1).min(size / 4)
}

fn stroke_width(digit_w: u32, digit_h: u32) -> u32 {
    let mut chosen = 1u32;
    let mut thickness = 1u32;
    loop {
        if digit_w < thickness * 2 + 1 || digit_h < thickness * 3 + 2 {
            break;
        }
        let hole = digit_w - thickness * 2;
        let open_enough = hole >= 2 || (thickness == 1 && hole >= 1);
        if open_enough {
            chosen = thickness;
        }
        thickness += 1;
    }
    chosen
}

fn draw_digit(
    pixels: &mut [u8],
    size: u32,
    ch: u8,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    t: u32,
    color: [u8; 4],
) {
    let mask = segments(ch);
    let rest = h.saturating_sub(3 * t);
    let upper = rest / 2;
    let y_upper = y + t;
    let y_mid = y_upper + upper;
    let y_lower = y_mid + t;
    let y_bottom = y + h - t;

    if mask & TOP != 0 {
        fill(pixels, size, x, y, w, t, color);
    }
    if mask & MIDDLE != 0 {
        fill(pixels, size, x, y_mid, w, t, color);
    }
    if mask & BOTTOM != 0 {
        fill(pixels, size, x, y_bottom, w, t, color);
    }
    draw_side(
        pixels,
        size,
        mask,
        UPPER_LEFT,
        LOWER_LEFT,
        x,
        y,
        y_upper,
        y_mid,
        y_lower,
        y_bottom,
        h,
        t,
        color,
    );
    draw_side(
        pixels,
        size,
        mask,
        UPPER_RIGHT,
        LOWER_RIGHT,
        x + w - t,
        y,
        y_upper,
        y_mid,
        y_lower,
        y_bottom,
        h,
        t,
        color,
    );
}

fn draw_side(
    pixels: &mut [u8],
    size: u32,
    mask: u8,
    upper_bit: u8,
    lower_bit: u8,
    x: u32,
    y: u32,
    y_upper: u32,
    y_mid: u32,
    y_lower: u32,
    y_bottom: u32,
    h: u32,
    t: u32,
    color: [u8; 4],
) {
    let upper_on = mask & upper_bit != 0;
    let lower_on = mask & lower_bit != 0;
    let has_top = mask & TOP != 0;
    let has_mid = mask & MIDDLE != 0;
    let has_bottom = mask & BOTTOM != 0;
    if upper_on && lower_on && !has_mid {
        let y0 = if has_top { y_upper } else { y };
        let y1 = if has_bottom { y_bottom } else { y + h };
        fill(pixels, size, x, y0, t, y1.saturating_sub(y0), color);
        return;
    }
    if upper_on {
        let y0 = if has_top { y_upper } else { y };
        let y1 = if has_mid { y_mid } else { y_lower };
        fill(pixels, size, x, y0, t, y1.saturating_sub(y0), color);
    }
    if lower_on {
        let y0 = if has_mid { y_lower } else { y_mid };
        let y1 = if has_bottom { y_bottom } else { y + h };
        fill(pixels, size, x, y0, t, y1.saturating_sub(y0), color);
    }
}

fn segments(ch: u8) -> u8 {
    match ch {
        b'0' => TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | LOWER_LEFT | UPPER_LEFT,
        b'1' => UPPER_RIGHT | LOWER_RIGHT,
        b'2' => TOP | UPPER_RIGHT | BOTTOM | LOWER_LEFT | MIDDLE,
        b'3' => TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | MIDDLE,
        b'4' => UPPER_RIGHT | LOWER_RIGHT | UPPER_LEFT | MIDDLE,
        b'5' => TOP | LOWER_RIGHT | BOTTOM | UPPER_LEFT | MIDDLE,
        b'6' => TOP | LOWER_RIGHT | BOTTOM | LOWER_LEFT | UPPER_LEFT | MIDDLE,
        b'7' => TOP | UPPER_RIGHT | LOWER_RIGHT,
        b'8' => {
            TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | LOWER_LEFT | UPPER_LEFT | MIDDLE
        }
        b'9' => TOP | UPPER_RIGHT | LOWER_RIGHT | BOTTOM | UPPER_LEFT | MIDDLE,
        b'-' => MIDDLE,
        _ => 0,
    }
}

fn cover(x: u32, y: u32, radius: u32, size: u32) -> u8 {
    if x >= size || y >= size {
        return 0;
    }
    // 每个像素里采 8×8 个点，圆角按真实覆盖率变透明，而不是整格砍掉。
    let mut hits = 0u32;
    for sy in 0..8 {
        for sx in 0..8 {
            let px = x as f32 + (sx as f32 + 0.5) / 8.0;
            let py = y as f32 + (sy as f32 + 0.5) / 8.0;
            if inside_round_rect(px, py, radius as f32, size as f32) {
                hits += 1;
            }
        }
    }
    ((hits * 255 + 32) / 64) as u8
}

fn inside_round_rect(px: f32, py: f32, radius: f32, size: f32) -> bool {
    let cx = if px < radius {
        radius
    } else if px > size - radius {
        size - radius
    } else {
        px
    };
    let cy = if py < radius {
        radius
    } else if py > size - radius {
        size - radius
    } else {
        py
    };
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= radius * radius
}

fn hicon_with_alpha(pixels: &[u8], size: u32) -> Option<isize> {
    if size == 0 || pixels.len() != (size * size * 4) as usize {
        return None;
    }
    unsafe {
        let bitmap = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: size as i32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(
            std::ptr::null_mut(),
            &bitmap,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if color.is_null() || bits.is_null() {
            return None;
        }
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, pixels.len());
        for y in 0..size as usize {
            let src_y = size as usize - 1 - y;
            for x in 0..size as usize {
                let src = (src_y * size as usize + x) * 4;
                let dst_i = (y * size as usize + x) * 4;
                let alpha = pixels[src + 3] as u16;
                dst[dst_i] = (pixels[src + 2] as u16 * alpha / 255) as u8;
                dst[dst_i + 1] = (pixels[src + 1] as u16 * alpha / 255) as u8;
                dst[dst_i + 2] = (pixels[src] as u16 * alpha / 255) as u8;
                dst[dst_i + 3] = pixels[src + 3];
            }
        }
        let stride = ((size as usize + 15) / 16) * 2;
        let mask_bits = vec![0u8; stride * size as usize];
        let mask = CreateBitmap(
            size as i32,
            size as i32,
            1,
            1,
            mask_bits.as_ptr() as *const _,
        );
        if mask.is_null() {
            DeleteObject(color as _);
            return None;
        }
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color as _);
        DeleteObject(mask as _);
        if icon.is_null() {
            None
        } else {
            Some(icon as isize)
        }
    }
}

fn premultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u16;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        pixel[0] = (pixel[0] as u16 * alpha / 255) as u8;
        pixel[1] = (pixel[1] as u16 * alpha / 255) as u8;
        pixel[2] = (pixel[2] as u16 * alpha / 255) as u8;
    }
}

fn fill(pixels: &mut [u8], size: u32, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for dy in 0..h {
        for dx in 0..w {
            paint(pixels, size, x + dx, y + dy, color);
        }
    }
}

fn paint(pixels: &mut [u8], size: u32, x: u32, y: u32, color: [u8; 4]) {
    if x >= size || y >= size {
        return;
    }
    let index = ((y * size + x) * 4) as usize;
    let mask = pixels[index + 3];
    if mask == 0 {
        return;
    }
    let alpha = (color[3] as u16 * mask as u16 / 255) as u8;
    pixels[index] = color[0];
    pixels[index + 1] = color[1];
    pixels[index + 2] = color[2];
    pixels[index + 3] = alpha;
}

fn put(pixels: &mut [u8], size: u32, x: u32, y: u32, color: [u8; 4]) {
    if x >= size || y >= size || color[3] == 0 {
        return;
    }
    let index = ((y * size + x) * 4) as usize;
    pixels[index..index + 4].copy_from_slice(&color);
}

#[cfg(test)]
mod tests {
    #[test]
    fn corners_are_feathered() {
        let pixels = super::render(144, 16);
        let corner = alpha_at(&pixels, 16, 0, 0);
        let edge = alpha_at(&pixels, 16, 8, 0);
        let center = alpha_at(&pixels, 16, 8, 8);
        assert!(corner < 255, "outer corner should not be solid, alpha={corner}");
        assert_eq!(edge, 255, "straight edge stays solid");
        assert_eq!(center, 255);
        let partial = (0..4).any(|y| {
            (0..4).any(|x| {
                let alpha = alpha_at(&pixels, 16, x, y);
                (1..255).contains(&alpha)
            })
        });
        assert!(partial, "corner should have a soft transition");
    }

    fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
        pixels[((y * size + x) * 4 + 3) as usize]
    }

    #[test]
    fn refresh_rates_use_three_families() {
        assert_eq!(super::background_color(24), super::background_color(25));
        assert_eq!(super::background_color(48), super::background_color(50));
        assert_eq!(super::background_color(49), super::background_color(50));
        assert_eq!(super::background_color(59), super::background_color(60));
        assert_eq!(super::background_color(30), super::background_color(60));
        assert_eq!(super::background_color(90), super::background_color(60));
        assert_eq!(super::background_color(119), super::background_color(120));
        assert_eq!(super::background_color(75), super::background_color(100));
        assert_eq!(super::background_color(144), super::background_color(165));
        assert_ne!(super::background_color(60), super::background_color(75));
        assert_ne!(super::background_color(144), super::background_color(60));
        assert_ne!(super::background_color(144), super::background_color(25));
        assert_ne!(super::background_color(144), super::background_color(90));
    }

    #[test]
    fn three_digits_stay_separated_at_tray_size() {
        for size in [16u32, 20, 24, 32] {
            let pixels = super::render(144, size);
            let gaps = interior_gaps(&pixels, size);
            assert!(
                gaps >= 2,
                "{size}px 144 should keep three digits apart, gaps={gaps}"
            );
        }
    }

    #[test]
    fn two_digits_stay_separated_at_tray_size() {
        let pixels = super::render(60, 16);
        assert!(interior_gaps(&pixels, 16) >= 1);
    }

    #[test]
    fn two_and_three_digits_share_one_size() {
        for size in [16u32, 24, 32] {
            let sixty = super::render(60, size);
            let one_forty_four = super::render(144, size);
            assert_eq!(
                ink_rows(&sixty, size),
                ink_rows(&one_forty_four, size),
                "{size}px 60 and 144 should use the same cap height"
            );
            assert_eq!(
                last_run_width(&sixty, size),
                last_run_width(&one_forty_four, size),
                "{size}px the last digit should be the same width"
            );
        }
    }

    fn ink_rows(pixels: &[u8], size: u32) -> (u32, u32) {
        let mut min = size;
        let mut max = 0;
        for y in 0..size {
            for x in 0..size {
                if is_ink(pixels, size, x, y) {
                    min = min.min(y);
                    max = max.max(y);
                }
            }
        }
        (min, max)
    }

    fn last_run_width(pixels: &[u8], size: u32) -> u32 {
        let mut width = 0;
        let mut seen_gap = false;
        for x in (0..size).rev() {
            let ink = (0..size).any(|y| is_ink(pixels, size, x, y));
            if ink {
                if seen_gap {
                    break;
                }
                width += 1;
            } else if width > 0 {
                seen_gap = true;
            }
        }
        width
    }

    fn is_ink(pixels: &[u8], size: u32, x: u32, y: u32) -> bool {
        let index = ((y * size + x) * 4) as usize;
        pixels[index] > 240 && pixels[index + 1] > 240 && pixels[index + 2] > 240
    }

    fn interior_gaps(pixels: &[u8], size: u32) -> u32 {
        let columns: Vec<bool> = (0..size)
            .map(|x| {
                (0..size).any(|y| is_ink(pixels, size, x, y))
            })
            .collect();
        let mut gaps = 0;
        let mut seen_ink = false;
        let mut in_gap = false;
        for ink in columns {
            if ink {
                if in_gap && seen_ink {
                    gaps += 1;
                }
                seen_ink = true;
                in_gap = false;
            } else if seen_ink {
                in_gap = true;
            }
        }
        gaps
    }
}
