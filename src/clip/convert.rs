//! Converting between Windows' clipboard formats (what goes over the wire)
//! and the Mac's: pictures are DIBs on Windows and PNGs on a Mac, formatted
//! text is "HTML Format" (HTML behind a header of byte offsets) on Windows
//! and plain HTML on a Mac. Pure, so it's tested on any OS.

use std::io::Cursor;

/// Size of the BITMAPINFOHEADER written by [`png_to_dib`].
const INFO_HEADER: usize = 40;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A Windows DIB (24 or 32 bits per pixel) as top-down RGBA pixels.
pub fn dib_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let header = u32_at(dib, 0)? as usize;
    let width = u32_at(dib, 4)? as i32;
    let height = u32_at(dib, 8)? as i32;
    let bits = u16_at(dib, 14)?;
    let compression = u32_at(dib, 16)?;
    let colors_used = u32_at(dib, 32)?;
    if header < INFO_HEADER || width <= 0 || height == 0 || !matches!(bits, 24 | 32) {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if w.checked_mul(h)? > super::MAX_SIZE / 4 {
        return None;
    }
    // Channel masks: in the header from V4 on, else (bitfields) right after it.
    let masked = bits == 32 && matches!(compression, BI_BITFIELDS | BI_ALPHABITFIELDS);
    let masks_at = if header > INFO_HEADER { INFO_HEADER } else { header };
    let mask = |i: usize| if masked { u32_at(dib, masks_at + 4 * i) } else { None };
    let (r_mask, g_mask, b_mask) = match (mask(0), mask(1), mask(2)) {
        (Some(r), Some(g), Some(b)) => (r, g, b),
        _ => (0x00FF_0000, 0x0000_FF00, 0x0000_00FF),
    };
    let a_mask = match compression {
        _ if header > INFO_HEADER && masked => u32_at(dib, INFO_HEADER + 12).unwrap_or(0),
        BI_ALPHABITFIELDS => mask(3).unwrap_or(0),
        _ if bits == 32 => 0xFF00_0000,
        _ => 0,
    };
    let after_header = header
        + match (header == INFO_HEADER, compression) {
            (true, BI_BITFIELDS) => 12,
            (true, BI_ALPHABITFIELDS) => 16,
            _ => 0,
        };
    // 24/32-bit pictures normally have no palette, but one may be present.
    let pixels = after_header + colors_used as usize * 4;
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let data = dib.get(pixels..pixels + stride * h)?;

    let channel = |px: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let v = u64::from((px & mask) >> mask.trailing_zeros());
        let max = u64::from(mask >> mask.trailing_zeros());
        (v * 255 / max) as u8
    };
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = if height > 0 { h - 1 - y } else { y }; // bottom-up unless negative
        let line = &data[row * stride..];
        for x in 0..w {
            match bits {
                24 => {
                    let p = &line[x * 3..x * 3 + 3];
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
                _ => {
                    let px = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
                    rgba.extend_from_slice(&[
                        channel(px, r_mask),
                        channel(px, g_mask),
                        channel(px, b_mask),
                        channel(px, a_mask),
                    ]);
                }
            }
        }
    }
    // Most apps leave the alpha byte of a 32-bit DIB at zero: that's opaque.
    if bits == 32 && rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        rgba.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
    }
    Some((w as u32, h as u32, rgba))
}

/// Top-down RGBA pixels as a 32-bit bottom-up DIB (what Windows apps expect).
pub fn rgba_to_dib(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut dib = Vec::with_capacity(INFO_HEADER + w * h * 4);
    dib.extend_from_slice(&(INFO_HEADER as u32).to_le_bytes());
    dib.extend_from_slice(&(width as i32).to_le_bytes());
    dib.extend_from_slice(&(height as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes()); // planes
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&BI_RGB.to_le_bytes());
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    dib.extend_from_slice(&[0; 16]); // resolution, palette: none
    for y in (0..h).rev() {
        for p in rgba[y * w * 4..(y + 1) * w * 4].as_chunks::<4>().0 {
            dib.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    dib
}

pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgba) = dib_to_rgba(dib)?;
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

pub fn png_to_dib(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => px
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None, // expanded by the transformation
    };
    Some(rgba_to_dib(w, h, &rgba))
}

/// Windows' "HTML Format" around plain HTML.
pub fn to_cf_html(html: &str) -> Vec<u8> {
    const START: &str = "<html><body>\r\n<!--StartFragment-->";
    const END: &str = "<!--EndFragment-->\r\n</body></html>";
    let header_len = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n".len();
    let start_fragment = header_len + START.len();
    let end_fragment = start_fragment + html.len();
    let end_html = end_fragment + END.len();
    let mut out = format!(
        "Version:0.9\r\nStartHTML:{header_len:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    )
    .into_bytes();
    out.extend_from_slice(START.as_bytes());
    out.extend_from_slice(html.as_bytes());
    out.extend_from_slice(END.as_bytes());
    out
}

/// The HTML inside Windows' "HTML Format" (the whole document).
pub fn from_cf_html(cf: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(cf);
    let offset = |key: &str| -> Option<usize> {
        let at = text.find(key)? + key.len();
        let digits: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let (start, end) = match (offset("StartHTML:"), offset("EndHTML:")) {
        (Some(s), Some(e)) => (s, e),
        _ => (offset("StartFragment:")?, offset("EndFragment:")?),
    };
    let end = end.min(cf.len());
    let html = cf.get(start..end)?;
    Some(String::from_utf8_lossy(html).trim_end_matches('\0').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                [
                    (i * 7) as u8,
                    (i * 13) as u8,
                    (i * 29) as u8,
                    if i % 3 == 0 { 128 } else { 255 },
                ]
            })
            .collect()
    }

    #[test]
    fn dib_and_png_roundtrip() {
        let (w, h) = (37, 11); // odd width: rows are padded in DIBs
        let rgba = picture(w, h);
        let dib = rgba_to_dib(w, h, &rgba);
        assert_eq!(dib_to_rgba(&dib), Some((w, h, rgba.clone())));
        let png = dib_to_png(&dib).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert_eq!(dib_to_rgba(&png_to_dib(&png).unwrap()), Some((w, h, rgba)));
    }

    #[test]
    fn windows_24_bit_and_top_down_pictures() {
        // 3x2, 24-bit, bottom-up: rows are padded to 12 bytes.
        let mut dib = rgba_to_dib(3, 2, &[0; 24])[..INFO_HEADER].to_vec();
        dib[14] = 24;
        let bottom = [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0];
        let top = [10, 11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0];
        dib.extend_from_slice(&bottom);
        dib.extend_from_slice(&top);
        let (w, h, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(&rgba[..4], &[12, 11, 10, 255], "top row first, BGR to RGB");
        assert_eq!(&rgba[12..16], &[3, 2, 1, 255]);

        // The same with a negative height is top-down.
        dib[8..12].copy_from_slice(&(-2i32).to_le_bytes());
        let (_, _, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!(&rgba[..4], &[3, 2, 1, 255]);
    }

    #[test]
    fn a_dib_with_unused_alpha_is_opaque() {
        let mut dib = rgba_to_dib(2, 1, &[9, 9, 9, 0, 9, 9, 9, 0]);
        dib[16] = BI_BITFIELDS as u8;
        let masks = [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF];
        let tail = dib.split_off(INFO_HEADER);
        dib.extend(masks.iter().flat_map(|m| m.to_le_bytes()));
        dib.extend(tail);
        let (_, _, rgba) = dib_to_rgba(&dib).unwrap();
        assert_eq!(rgba, [9, 9, 9, 255, 9, 9, 9, 255]);
    }

    #[test]
    fn garbage_is_refused() {
        assert_eq!(dib_to_rgba(b"nope"), None);
        assert_eq!(
            dib_to_rgba(&rgba_to_dib(100, 100, &[0; 40000])[..500]),
            None,
            "truncated"
        );
        assert_eq!(png_to_dib(b"\x89PNG broken"), None);
    }

    #[test]
    fn html_format_roundtrip() {
        let cf = to_cf_html("<b>héllo</b>");
        let text = String::from_utf8(cf.clone()).unwrap();
        let start: usize = text["Version:0.9\r\nStartHTML:".len()..][..10].parse().unwrap();
        assert!(text[start..].starts_with("<html>"), "offsets are bytes into the data");
        let html = from_cf_html(&cf).unwrap();
        assert!(html.contains("<b>héllo</b>") && html.starts_with("<html>") && html.ends_with("</html>"));

        // What Chrome on Windows puts there.
        let chrome = "Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000199\r\nStartFragment:0000000141\r\nEndFragment:0000000163\r\n<html>\r\n<body>\r\n<!--StartFragment--><i>copied</i><!--EndFragment-->\r\n</body>\r\n</html>\0";
        let html = from_cf_html(chrome.as_bytes()).unwrap();
        assert!(html.starts_with("<html>") && html.contains("<i>copied</i>"));
        assert_eq!(from_cf_html(b"not html format"), None);
    }
}
