//! System cursor shape sampling for pointer telemetry: the current cursor's
//! image, hotspot, and a stable ID derived from its pixels (the same scheme
//! as the macOS bridge), with standard shapes named like the macOS ones.
use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::POINT;
use ::windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject, BITMAP,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, LoadCursorW, CURSORINFO, CURSOR_SHOWING, DI_NORMAL,
    HCURSOR, HICON, ICONINFO, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDC_NO, IDC_SIZENESW,
    IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE,
};

/// Same limits as the telemetry logger.
const MAX_SIZE: u32 = 256;
const MAX_PNG_BYTES: usize = 64_000;

#[derive(Clone, Debug)]
pub struct CursorSample {
    pub id: String,
    pub name: Option<&'static str>,
    pub hotspot_x: f64,
    pub hotspot_y: f64,
    pub width: f64,
    pub height: f64,
    pub png: Option<Vec<u8>>,
}

/// The current system cursor handle, or `None` while the cursor is hidden.
pub fn current() -> Option<HCURSOR> {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut info) }.ok()?;
    (info.flags == CURSOR_SHOWING && !info.hCursor.is_invalid()).then_some(info.hCursor)
}

/// What to record while an app hides the cursor: a named, image-less shape
/// so the editor draws nothing.
pub fn hidden() -> CursorSample {
    CursorSample {
        id: format!("{:016x}", fnv1a(b"hidden", FNV_OFFSET)),
        name: Some("hidden"),
        hotspot_x: 0.0,
        hotspot_y: 0.0,
        width: 1.0,
        height: 1.0,
        png: None,
    }
}

pub fn describe(cursor: HCURSOR) -> Option<CursorSample> {
    let mut info = ICONINFO::default();
    unsafe { GetIconInfo(HICON(cursor.0), &mut info) }.ok()?;
    let color = (!info.hbmColor.is_invalid()).then_some(info.hbmColor);
    let size = bitmap_size(color.unwrap_or(info.hbmMask));
    unsafe {
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        if let Some(color) = color {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
    }
    let (width, mut height) = size?;
    // A monochrome cursor stacks its AND and XOR masks in one bitmap.
    if color.is_none() {
        height /= 2;
    }
    if width == 0 || height == 0 || width > MAX_SIZE || height > MAX_SIZE {
        return None;
    }
    let hotspot = POINT {
        x: info.xHotspot as i32,
        y: info.yHotspot as i32,
    };
    if hotspot.x as u32 > width || hotspot.y as u32 > height {
        return None;
    }
    let pixels = render_rgba(cursor, width, height)?;

    let mut hash = fnv1a(&pixels, FNV_OFFSET);
    for value in [f64::from(hotspot.x), f64::from(hotspot.y)] {
        hash = fnv1a(&value.to_bits().to_le_bytes(), hash);
    }
    let transparent = !pixels.chunks_exact(4).any(|p| p[3] > 8);
    Some(CursorSample {
        id: format!("{hash:016x}"),
        name: if transparent {
            Some("hidden")
        } else {
            standard_name(cursor)
        },
        hotspot_x: f64::from(hotspot.x),
        hotspot_y: f64::from(hotspot.y),
        width: f64::from(width),
        height: f64::from(height),
        png: encode_png(&pixels, width, height).filter(|png| png.len() <= MAX_PNG_BYTES),
    })
}

fn bitmap_size(bitmap: HBITMAP) -> Option<(u32, u32)> {
    let mut info = BITMAP::default();
    let written = unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut _),
        )
    };
    (written > 0).then_some((info.bmWidth.max(0) as u32, info.bmHeight.max(0) as u32))
}

/// Draw the cursor on black and on white; the difference recovers alpha for
/// every cursor type, including monochrome and inverting ones.
fn render_rgba(cursor: HCURSOR, width: u32, height: u32) -> Option<Vec<u8>> {
    let on_black = draw_on(cursor, width, height, 0x00)?;
    let on_white = draw_on(cursor, width, height, 0xff)?;
    let mut rgba = Vec::with_capacity(on_black.len());
    for (black, white) in on_black.chunks_exact(4).zip(on_white.chunks_exact(4)) {
        // BGRA; alpha = 255 - (white - black), per the most opaque channel.
        let spread = (0..3)
            .map(|c| i32::from(white[c]) - i32::from(black[c]))
            .max()
            .unwrap_or(255)
            .clamp(0, 255);
        let alpha = 255 - spread;
        let channel = |c: usize| -> u8 {
            if alpha == 0 {
                0
            } else {
                (i32::from(black[c]) * 255 / alpha).clamp(0, 255) as u8
            }
        };
        rgba.extend_from_slice(&[channel(2), channel(1), channel(0), alpha as u8]);
    }
    Some(rgba)
}

fn draw_on(cursor: HCURSOR, width: u32, height: u32, background: u8) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32), // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .ok()
            .filter(|_| !bits.is_null())
            .and_then(|bitmap| {
                let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
                let len = (width * height * 4) as usize;
                let buffer = std::slice::from_raw_parts_mut(bits as *mut u8, len);
                buffer.fill(background);
                let drawn = DrawIconEx(
                    dc,
                    0,
                    0,
                    HICON(cursor.0),
                    width as i32,
                    height as i32,
                    0,
                    None,
                    DI_NORMAL,
                );
                let pixels = drawn.is_ok().then(|| buffer.to_vec());
                SelectObject(dc, previous);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                pixels
            });
        let _ = DeleteDC(dc);
        result
    }
}

/// Windows' shared system cursors, named like the macOS ones.
fn standard_name(cursor: HCURSOR) -> Option<&'static str> {
    const STANDARD: &[(PCWSTR, &str)] = &[
        (IDC_ARROW, "arrow"),
        (IDC_IBEAM, "i_beam"),
        (IDC_HAND, "pointing_hand"),
        (IDC_CROSS, "crosshair"),
        (IDC_SIZEWE, "resize_left_right"),
        (IDC_SIZENS, "resize_up_down"),
        (IDC_SIZENWSE, "frame_resize_diagonal_nwse"),
        (IDC_SIZENESW, "frame_resize_diagonal_nesw"),
        (IDC_NO, "operation_not_allowed"),
    ];
    STANDARD.iter().find_map(|(id, name)| {
        let shared = unsafe { LoadCursorW(None, *id) }.ok()?;
        (shared == cursor).then_some(*name)
    })
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_cursor_is_named_hashed_and_encoded() {
        let arrow = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap();
        let sample = describe(arrow).expect("the arrow cursor renders");
        assert_eq!(sample.name, Some("arrow"));
        assert_eq!(sample.id.len(), 16);
        assert!(sample.width > 0.0 && sample.width <= 256.0);
        assert!((0.0..=sample.width).contains(&sample.hotspot_x));
        let png = sample.png.expect("a PNG image");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // Stable: the same shape always gets the same ID.
        assert_eq!(describe(arrow).unwrap().id, sample.id);
    }

    #[test]
    fn ids_follow_pixels_and_names_follow_the_system_role() {
        let arrow = describe(unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap()).unwrap();
        let beam = describe(unsafe { LoadCursorW(None, IDC_IBEAM) }.unwrap()).unwrap();
        // A cursor scheme may draw both roles with one image; the ID tracks
        // the image, the name the role.
        assert_eq!(arrow.id == beam.id, arrow.png == beam.png);
        assert_eq!(beam.name, Some("i_beam"));
        assert_eq!(hidden().name, Some("hidden"));
    }
}
