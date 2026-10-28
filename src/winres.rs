//! Windows resources without a resource compiler: the app icon as an `.ico`
//! file and a compiled `.res` (icon group + version info) that the MSVC
//! linker takes directly. Used by `build.rs` and the tests.

#![allow(dead_code)]

pub const ICON_SIZES: [usize; 7] = [16, 20, 24, 32, 48, 64, 256];

/// One icon image: PNG for the big ones, a 32-bit DIB for the small ones
/// (what Explorer handles best at every size).
fn image_data(n: usize, premul_bgra: &[u8], png: &dyn Fn(usize, &[u8]) -> Vec<u8>) -> Vec<u8> {
    // Straight (not premultiplied) BGRA, top-down.
    let mut straight = Vec::with_capacity(premul_bgra.len());
    for px in premul_bgra.chunks_exact(4) {
        let a = px[3] as u32;
        let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a / 2) / a).min(255) as u8 };
        straight.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), px[3]]);
    }
    if n >= 128 {
        let mut rgba = Vec::with_capacity(straight.len());
        for px in straight.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        return png(n, &rgba);
    }
    let mut d = Vec::new();
    let mask_row = n.div_ceil(32) * 4;
    let push32 = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
    push32(&mut d, 40);
    push32(&mut d, n as u32);
    push32(&mut d, (n * 2) as u32);
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    push32(&mut d, 0);
    push32(&mut d, (n * n * 4 + mask_row * n) as u32);
    for _ in 0..4 {
        push32(&mut d, 0);
    }
    for y in (0..n).rev() {
        d.extend_from_slice(&straight[y * n * 4..(y + 1) * n * 4]);
    }
    d.extend(std::iter::repeat_n(0u8, mask_row * n));
    d
}

/// All icon images, smallest first.
pub fn icon_images(pixels: &dyn Fn(usize) -> Vec<u8>, png: &dyn Fn(usize, &[u8]) -> Vec<u8>) -> Vec<(usize, Vec<u8>)> {
    ICON_SIZES.iter().map(|&n| (n, image_data(n, &pixels(n), png))).collect()
}

fn dir_entry(out: &mut Vec<u8>, n: usize, len: usize) {
    out.push(if n >= 256 { 0 } else { n as u8 });
    out.push(if n >= 256 { 0 } else { n as u8 });
    out.push(0);
    out.push(0);
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
}

/// A standalone `.ico` file (for the installer).
pub fn ico_file(images: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (n, data) in images {
        dir_entry(&mut out, *n, data.len());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
}

/// One entry of a `.res` file with numeric type and name.
fn res_entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0x0409u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// A version-info block: header, key, value, then DWORD-aligned children.
fn vblock(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0, 0];
    b.extend_from_slice(&value_len.to_le_bytes());
    b.extend_from_slice(&(text as u16).to_le_bytes());
    b.extend_from_slice(&utf16z(key));
    pad4(&mut b);
    b.extend_from_slice(value);
    for c in children {
        pad4(&mut b);
        b.extend_from_slice(c);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b
}

fn vstring(key: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    vblock(key, &v, (v.len() / 2) as u16, true, &[])
}

pub fn version_info(version: &str, strings: &[(&str, &str)]) -> Vec<u8> {
    let nums: Vec<u32> = version.split('.').map(|p| p.parse().unwrap_or(0)).chain(std::iter::repeat(0)).take(4).collect();
    let ms = (nums[0] << 16) | nums[1];
    let ls = (nums[2] << 16) | nums[3];
    let mut fixed = Vec::new();
    for v in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend_from_slice(&v.to_le_bytes());
    }
    let table: Vec<Vec<u8>> = strings.iter().map(|(k, v)| vstring(k, v)).collect();
    let string_table = vblock("040904B0", &[], 0, true, &table);
    let sfi = vblock("StringFileInfo", &[], 0, true, &[string_table]);
    let var = vblock("Translation", &0x04B0_0409u32.to_le_bytes(), 4, false, &[]);
    let vfi = vblock("VarFileInfo", &[], 0, true, &[var]);
    vblock("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[sfi, vfi])
}

/// A complete `.res`: icon images (ids 1..), icon group 1, version info 1.
pub fn res_file(images: &[(usize, Vec<u8>)], version: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // The empty entry every .res starts with.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    out.extend_from_slice(&[0u8; 16]);
    for (i, (_, data)) in images.iter().enumerate() {
        res_entry(&mut out, 3, (i + 1) as u16, 0x1010, data);
    }
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (i, (n, data)) in images.iter().enumerate() {
        dir_entry(&mut group, *n, data.len());
        group.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }
    res_entry(&mut out, 14, 1, 0x1030, &group);
    res_entry(&mut out, 16, 1, 0x0030, version);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_png(n: usize, rgba: &[u8]) -> Vec<u8> {
        let mut v = b"PNG".to_vec();
        v.extend_from_slice(&(n as u32).to_le_bytes());
        v.extend_from_slice(&rgba[..4]);
        v
    }

    fn solid(n: usize) -> Vec<u8> {
        // Premultiplied half-transparent blue.
        (0..n * n).flat_map(|_| [0x80u8, 0, 0, 0x80]).collect()
    }

    #[test]
    fn ico_has_a_directory_entry_per_size() {
        let imgs = icon_images(&solid, &fake_png);
        let ico = ico_file(&imgs);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]) as usize, ICON_SIZES.len());
        // First entry is 16 px; the 256 px one is written as 0.
        assert_eq!(ico[6], 16);
        let last = 6 + 16 * (ICON_SIZES.len() - 1);
        assert_eq!(ico[last], 0);
        // Offsets point inside the file and the data there is what we wrote.
        let off = u32::from_le_bytes(ico[last + 12..last + 16].try_into().unwrap()) as usize;
        assert_eq!(&ico[off..off + 3], b"PNG");
        // Small sizes are DIBs: 40-byte header, double height, straight alpha.
        let (n, d) = &imgs[0];
        assert_eq!(u32::from_le_bytes(d[0..4].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(d[8..12].try_into().unwrap()) as usize, n * 2);
        assert_eq!(&d[40..44], &[0xFF, 0, 0, 0x80]);
    }

    #[test]
    fn res_entries_are_aligned_and_typed() {
        let imgs = icon_images(&solid, &fake_png);
        let ver = version_info("0.6.0", &[("FileDescription", "Snap by rng")]);
        let res = res_file(&imgs, &ver);
        // Walk the entries.
        let mut at = 0;
        let mut kinds = Vec::new();
        while at < res.len() {
            assert_eq!(at % 4, 0);
            let size = u32::from_le_bytes(res[at..at + 4].try_into().unwrap()) as usize;
            let hdr = u32::from_le_bytes(res[at + 4..at + 8].try_into().unwrap()) as usize;
            kinds.push(u16::from_le_bytes([res[at + 10], res[at + 11]]));
            at += hdr + size;
            at = at.div_ceil(4) * 4;
        }
        assert_eq!(at, res.len());
        assert_eq!(kinds[0], 0);
        assert_eq!(kinds.iter().filter(|k| **k == 3).count(), ICON_SIZES.len());
        assert_eq!(kinds.iter().filter(|k| **k == 14).count(), 1);
        assert_eq!(kinds.iter().filter(|k| **k == 16).count(), 1);
    }

    #[test]
    fn version_info_layout() {
        let v = version_info("1.2.3", &[("ProductName", "Snap")]);
        assert_eq!(u16::from_le_bytes([v[0], v[1]]) as usize, v.len());
        assert_eq!(u16::from_le_bytes([v[2], v[3]]), 52);
        // Key, then the fixed info after padding: signature and 1.2.3.0.
        let key_end = 6 + "VS_VERSION_INFO".len() * 2 + 2;
        let fixed = key_end.div_ceil(4) * 4;
        assert_eq!(u32::from_le_bytes(v[fixed..fixed + 4].try_into().unwrap()), 0xFEEF04BD);
        assert_eq!(u32::from_le_bytes(v[fixed + 8..fixed + 12].try_into().unwrap()), (1 << 16) | 2);
        assert_eq!(u32::from_le_bytes(v[fixed + 12..fixed + 16].try_into().unwrap()), 3 << 16);
        let text: String = v.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).filter_map(|c| char::from_u32(c as u32)).collect();
        assert!(text.contains("StringFileInfo") && text.contains("040904B0") && text.contains("ProductName") && text.contains("Snap") && text.contains("Translation"));
    }
}
