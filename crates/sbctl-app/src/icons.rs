use gpui_kit::{App, DevicePixels, SvgSize, size};
use gpui_tray::{Error, Icon};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayVisualState {
    Connected,
    Disconnected,
    Transitioning,
    Broken,
}

const WORLD: &[u8] = include_bytes!("../../../assets/icons/tray/world.svg");
const WORLD_CHECK: &[u8] = include_bytes!("../../../assets/icons/tray/world-check.svg");

fn globe_svg(grid: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3.6 9h16.8m-16.8 6h16.8"/>{grid}</svg>"#
    )
}

fn steady_globe(state: TrayVisualState) -> String {
    let indicator = match state {
        TrayVisualState::Transitioning => r#"<path d="M10 1.1h4"/>"#,
        TrayVisualState::Broken => r#"<path d="M21.5 15.5v3m0 3v.1"/>"#,
        TrayVisualState::Connected | TrayVisualState::Disconnected => "",
    };
    globe_svg(&format!(
        r#"<path d="M11.5 3a17 17 0 0 0 0 18m1-18a17 17 0 0 1 0 18"/>{indicator}"#
    ))
}

fn build_image(state: TrayVisualState, svg: &[u8], cx: &App) -> gpui_tray::Result<Icon> {
    #[cfg(target_os = "macos")]
    const SIZE: u32 = 36;
    #[cfg(not(target_os = "macos"))]
    const SIZE: u32 = 32;

    let renderer = cx.svg_renderer();
    let pixels = DevicePixels(SIZE as i32);
    let image = renderer
        .parse_svg(svg)
        .and_then(|source| {
            renderer.render_parsed(&source, SvgSize::ExactSize(size(pixels, pixels)))
        })
        .map_err(|err| Error::InvalidIcon(err.to_string()))?;
    let bitmap = image
        .as_bytes(0)
        .ok_or_else(|| Error::InvalidIcon("missing rasterized frame".into()))?;

    #[cfg(target_os = "macos")]
    {
        let bitmap = Icon::from_rgba(bitmap.to_vec(), SIZE, SIZE)
            .scale_factor(2.0)
            .template(true)
            .build()?;
        let image = bitmap.to_ns_image()?;
        let description = objc2_foundation::NSString::from_str(match state {
            TrayVisualState::Connected => "Proxy connected",
            TrayVisualState::Disconnected => "Proxy disconnected",
            TrayVisualState::Transitioning => "Proxy updating",
            TrayVisualState::Broken => "Proxy connection issue",
        });
        image.setAccessibilityDescription(Some(&description));
        Ok(Icon::from_ns_image(image))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let tint: [u8; 3] = match state {
            TrayVisualState::Connected => [94, 234, 158],
            TrayVisualState::Disconnected => [244; 3],
            TrayVisualState::Transitioning => [255, 204, 102],
            TrayVisualState::Broken => [255, 138, 133],
        };
        // The dark disk gives the tray icon a predictable background on Windows.
        const BACKGROUND: [u8; 3] = [18, 27, 43];
        let mut rgba = Vec::with_capacity(bitmap.len());
        let side = SIZE as f32;
        for (i, px) in bitmap.as_chunks::<4>().0.iter().enumerate() {
            let x = (i as u32 % SIZE) as f32 + 0.5 - side / 2.0;
            let y = (i as u32 / SIZE) as f32 + 0.5 - side / 2.0;
            let mark = f32::from(px[3]) / 255.0;
            let backing = (0.5 + side * 0.46 - x.hypot(y)).clamp(0.0, 1.0) * (1.0 - mark);
            let opacity = mark + backing;
            for (fg, bg) in tint.into_iter().zip(BACKGROUND) {
                let color =
                    (f32::from(fg) * mark + f32::from(bg) * backing) / opacity.max(f32::EPSILON);
                rgba.push(color.round() as u8);
            }
            rgba.push((opacity * 255.0).round() as u8);
        }
        Icon::from_rgba(rgba, SIZE, SIZE).build()
    }
}

pub fn build_tray_icon(state: TrayVisualState, cx: &App) -> gpui_tray::Result<Icon> {
    match state {
        TrayVisualState::Disconnected => build_image(state, WORLD, cx),
        TrayVisualState::Connected => build_image(state, WORLD_CHECK, cx),
        TrayVisualState::Transitioning | TrayVisualState::Broken => {
            build_image(state, steady_globe(state).as_bytes(), cx)
        }
    }
}

#[cfg(target_os = "macos")]
fn spin_svg(phase: f32) -> String {
    use std::f32::consts::FRAC_PI_2;
    use std::fmt::Write as _;

    let mut grid = String::new();
    for meridian in 0..4 {
        let angle = phase + FRAC_PI_2 * meridian as f32;
        if angle.cos() <= 0.0 {
            continue;
        }
        let x = angle.sin() * 9.0;
        if x.abs() < 0.05 {
            grid.push_str(r#"<path d="M12 3v18"/>"#);
        } else {
            let sweep = u8::from(x > 0.0);
            let _ = write!(
                grid,
                r#"<path d="M12 3a{:.3} 9 0 0 {sweep} 0 18"/>"#,
                x.abs()
            );
        }
    }
    globe_svg(&grid)
}

#[cfg(target_os = "macos")]
pub fn build_spin_frames(cx: &App) -> gpui_tray::Result<Vec<Icon>> {
    const FRAMES: usize = 36;
    // Four evenly spaced meridians repeat after a quarter turn.
    (0..FRAMES)
        .map(|frame| {
            let phase = std::f32::consts::FRAC_PI_2 * frame as f32 / FRAMES as f32;
            build_image(
                TrayVisualState::Transitioning,
                spin_svg(phase).as_bytes(),
                cx,
            )
        })
        .collect()
}
