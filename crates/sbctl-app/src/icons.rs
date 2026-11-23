use gpui_tray::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayVisualState {
    Connected,
    Disconnected,
    Transitioning,
    Broken,
}

fn sdf_rounded_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let dx = (px - cx).abs() - (hw - r);
    let dy = (py - cy).abs() - (hh - r);
    let ox = dx.max(0.0);
    let oy = dy.max(0.0);
    (ox * ox + oy * oy).sqrt() + dx.max(dy).min(0.0) - r
}

fn sdf_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let denom = bax * bax + bay * bay + 1e-6;
    let h = ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0);
    let dx = pax - bax * h;
    let dy = pay - bay * h;
    (dx * dx + dy * dy).sqrt() - r
}

fn smooth_cov(dist: f32, aa: f32) -> f32 {
    (0.5 - dist / aa).clamp(0.0, 1.0)
}

#[cfg(not(target_os = "macos"))]
fn blend_over(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let [sr, sg, sb, sa] = src;
    let [dr, dg, db, da] = dst;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 1e-6 {
        return [0.0; 4];
    }
    let r = (sr * sa + dr * da * (1.0 - sa)) / out_a;
    let g = (sg * sa + dg * da * (1.0 - sa)) / out_a;
    let b = (sb * sa + db * da * (1.0 - sa)) / out_a;
    [r, g, b, out_a]
}

/// Render a 36x36 monochrome template glyph for the macOS menu bar fallback.
#[cfg(target_os = "macos")]
pub fn macos_template_rgba(state: TrayVisualState) -> (Vec<u8>, u32, u32) {
    let size = 36u32;
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let outer = sdf_rounded_box(px, py, s * 0.5, s * 0.5, s * 0.36, s * 0.36, s * 0.14);
            let inner = sdf_rounded_box(px, py, s * 0.5, s * 0.5, s * 0.25, s * 0.25, s * 0.08);
            let ring_d = outer.max(-inner);

            let r_bar = s * 0.055;
            let s_mark = sdf_segment(px, py, s * 0.35, s * 0.34, s * 0.65, s * 0.34, r_bar)
                .min(sdf_segment(
                    px,
                    py,
                    s * 0.35,
                    s * 0.34,
                    s * 0.65,
                    s * 0.66,
                    r_bar,
                ))
                .min(sdf_segment(
                    px,
                    py,
                    s * 0.35,
                    s * 0.66,
                    s * 0.65,
                    s * 0.66,
                    r_bar,
                ));

            let mut cov = smooth_cov(ring_d, 1.1).max(smooth_cov(s_mark, 1.1));
            match state {
                TrayVisualState::Connected => {
                    let dot = ((px - s * 0.78).hypot(py - s * 0.22)) - s * 0.10;
                    cov = cov.max(smooth_cov(dot, 1.1));
                }
                TrayVisualState::Disconnected => {
                    cov *= 0.55;
                }
                TrayVisualState::Transitioning => {
                    let dot = ((px - s * 0.50).hypot(py - s * 0.84)) - s * 0.08;
                    cov = cov.max(smooth_cov(dot, 1.1));
                }
                TrayVisualState::Broken => {
                    let bang =
                        sdf_segment(px, py, s * 0.78, s * 0.12, s * 0.78, s * 0.30, s * 0.055);
                    cov = cov.max(smooth_cov(bang, 1.1));
                }
            }

            let idx = ((y * size + x) * 4) as usize;
            out[idx] = 0;
            out[idx + 1] = 0;
            out[idx + 2] = 0;
            out[idx + 3] = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }

    (out, size, size)
}

/// Render a crisp 32x32 color-coded tray icon suitable for the Windows taskbar tray.
#[cfg(not(target_os = "macos"))]
pub fn windows_tray_rgba(state: TrayVisualState) -> (Vec<u8>, u32, u32) {
    let size = 32u32;
    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];

    let accent = match state {
        TrayVisualState::Connected => [16.0 / 255.0, 185.0 / 255.0, 129.0 / 255.0],
        TrayVisualState::Disconnected => [148.0 / 255.0, 163.0 / 255.0, 184.0 / 255.0],
        TrayVisualState::Transitioning => [245.0 / 255.0, 158.0 / 255.0, 11.0 / 255.0],
        TrayVisualState::Broken => [239.0 / 255.0, 68.0 / 255.0, 68.0 / 255.0],
    };

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let bg_d = sdf_rounded_box(px, py, s * 0.48, s * 0.48, s * 0.40, s * 0.40, s * 0.14);
            let bg_cov = smooth_cov(bg_d, 1.1);
            let mut col = [15.0 / 255.0, 23.0 / 255.0, 42.0 / 255.0, bg_cov];

            let r_bar = s * 0.060;
            let s_mark = sdf_segment(px, py, s * 0.30, s * 0.32, s * 0.62, s * 0.32, r_bar)
                .min(sdf_segment(
                    px,
                    py,
                    s * 0.30,
                    s * 0.32,
                    s * 0.62,
                    s * 0.64,
                    r_bar,
                ))
                .min(sdf_segment(
                    px,
                    py,
                    s * 0.30,
                    s * 0.64,
                    s * 0.62,
                    s * 0.64,
                    r_bar,
                ));
            let mark_cov = smooth_cov(s_mark, 1.1);
            col = blend_over(col, [248.0 / 255.0, 250.0 / 255.0, 252.0 / 255.0, mark_cov]);

            // Status indicator badge at bottom-right.
            let bx = s * 0.76;
            let by = s * 0.76;
            let rim_d = (px - bx).hypot(py - by) - s * 0.21;
            let rim_cov = smooth_cov(rim_d, 1.1);
            col = blend_over(col, [15.0 / 255.0, 23.0 / 255.0, 42.0 / 255.0, rim_cov]);

            let dot_d = (px - bx).hypot(py - by) - s * 0.15;
            let dot_cov = smooth_cov(dot_d, 1.1);
            col = blend_over(col, [accent[0], accent[1], accent[2], dot_cov]);

            let idx = ((y * size + x) * 4) as usize;
            out[idx] = (col[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 1] = (col[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 2] = (col[2].clamp(0.0, 1.0) * 255.0).round() as u8;
            out[idx + 3] = (col[3].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }

    (out, size, size)
}

/// Build the platform-appropriate tray icon for the current health state.
pub fn build_tray_icon(state: TrayVisualState) -> gpui_tray::Result<Icon> {
    #[cfg(target_os = "macos")]
    {
        let (symbol, desc) = match state {
            TrayVisualState::Connected => ("shield.lefthalf.filled", "sbctl connected"),
            TrayVisualState::Disconnected => ("network", "sbctl disconnected"),
            TrayVisualState::Transitioning => ("arrow.triangle.2.circlepath", "sbctl switching"),
            TrayVisualState::Broken => ("exclamationmark.triangle.fill", "sbctl error"),
        };
        if let Ok(icon) = Icon::system_symbol(symbol)
            .accessibility_description(desc)
            .point_size(14.5)
            .build()
        {
            return Ok(icon);
        }
        let (rgba, w, h) = macos_template_rgba(state);
        Icon::from_rgba(rgba, w, h)
            .scale_factor(2.0)
            .template(true)
            .build()
    }

    #[cfg(not(target_os = "macos"))]
    {
        let (rgba, w, h) = windows_tray_rgba(state);
        Icon::from_rgba(rgba, w, h).build()
    }
}
