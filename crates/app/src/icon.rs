//! Original application icon, drawn procedurally (no external art). This file is
//! `include!`d by build.rs to produce the .ico embedded in the executable, so it must
//! depend on `std` only.

#[allow(dead_code)]
/// RGBA8 pixels of the icon at `size` x `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut px = vec![0u8; n * n * 4];
    let s = size as f32;
    // Signed distance helpers in unit coordinates (0..1).
    fn rbox(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
        let (dx, dy) = ((x - cx).abs() - (hw - r), (y - cy).abs() - (hh - r));
        let (ox, oy) = (dx.max(0.0), dy.max(0.0));
        (ox * ox + oy * oy).sqrt() + dx.max(dy).min(0.0) - r
    }
    fn capsule(x: f32, y: f32, cx: f32, y0: f32, y1: f32, r: f32) -> f32 {
        let py = y.clamp(y0, y1);
        ((x - cx).powi(2) + (y - py).powi(2)).sqrt() - r
    }
    fn ring(x: f32, y: f32, cx: f32, cy: f32, r: f32, w: f32) -> f32 {
        (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).abs() - w
    }
    let cover = |d: f32, aa: f32| (0.5 - d / aa).clamp(0.0, 1.0);
    for j in 0..n {
        for i in 0..n {
            let (x, y) = ((i as f32 + 0.5) / s, (j as f32 + 0.5) / s);
            let aa = 1.2 / s;
            // Background: rounded square with a diagonal gradient.
            let bg = cover(rbox(x, y, 0.5, 0.5, 0.47, 0.47, 0.20), aa);
            let t = (x * 0.6 + y * 0.9).clamp(0.0, 1.0);
            let (r0, g0, b0) = (28.0, 36.0, 66.0);
            let (r1, g1, b1) = (74.0, 132.0, 235.0);
            let mut c = [
                r0 + (r1 - r0) * (1.0 - t),
                g0 + (g1 - g0) * (1.0 - t),
                b0 + (b1 - b0) * (1.0 - t),
            ];
            // Capsule microphone body.
            let body = cover(capsule(x, y, 0.5, 0.30, 0.46, 0.115), aa);
            // Cradle (U shape) and stand.
            let cradle = {
                let d = ring(x, y, 0.5, 0.46, 0.185, 0.02);
                let below = if y >= 0.46 { 1.0 } else { 0.0 };
                cover(d, aa) * below
            };
            let stem = cover(rbox(x, y, 0.5, 0.735, 0.02, 0.07, 0.01), aa);
            let foot = cover(rbox(x, y, 0.5, 0.80, 0.14, 0.022, 0.011), aa);
            // Sound arcs on both sides.
            let arc = |side: f32| {
                let d = ring(x, y, 0.5, 0.38, 0.30 + side, 0.016);
                let ang = ((y - 0.38).atan2((x - 0.5).abs())).abs();
                cover(d, aa) * if ang < 0.62 { 1.0 } else { 0.0 }
            };
            let arcs = arc(0.0).max(arc(0.085)) * 0.8;
            let fg = body.max(cradle).max(stem).max(foot).max(arcs);
            for ch in c.iter_mut() {
                *ch = *ch * (1.0 - fg) + 255.0 * fg;
            }
            let o = (j * n + i) * 4;
            px[o] = c[0] as u8;
            px[o + 1] = c[1] as u8;
            px[o + 2] = c[2] as u8;
            px[o + 3] = (bg * 255.0) as u8;
        }
    }
    px
}

/// Serialises the icon as a multi-resolution .ico (32-bit BMP entries).
pub fn ico_bytes(sizes: &[u32]) -> Vec<u8> {
    let mut images: Vec<(u32, Vec<u8>)> = Vec::new();
    for &s in sizes {
        let px = rgba(s);
        let n = s as usize;
        // BITMAPINFOHEADER + bottom-up BGRA XOR bitmap + 1bpp AND mask.
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(s as i32).to_le_bytes());
        dib.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&((n * n * 4) as u32).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]);
        for row in (0..n).rev() {
            for col in 0..n {
                let o = (row * n + col) * 4;
                dib.extend_from_slice(&[px[o + 2], px[o + 1], px[o], px[o + 3]]);
            }
        }
        let mask_row = n.div_ceil(32) * 4;
        dib.resize(dib.len() + mask_row * n, 0);
        images.push((s, dib));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (s, d) in &images {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += d.len();
    }
    for (_, d) in images {
        out.extend_from_slice(&d);
    }
    out
}
