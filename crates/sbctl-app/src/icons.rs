use gpui_tray::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayVisualState {
    Connected,
    Disconnected,
    Transitioning,
    Broken,
}

#[derive(Clone, Copy)]
struct Badge {
    x: f32,
    y: f32,
    radius: f32,
    color: [u8; 3],
    alert: bool,
}

impl Badge {
    fn for_state(state: TrayVisualState, angle: f32) -> Option<Self> {
        let (x, y, radius, color, alert) = match state {
            TrayVisualState::Connected | TrayVisualState::Disconnected => return None,
            TrayVisualState::Broken => (0.75, 0.75, 0.119, [255, 138, 133], true),
            TrayVisualState::Transitioning => (
                0.5 + 0.365 * angle.cos(),
                0.5 + 0.365 * angle.sin(),
                0.077,
                [255, 204, 102],
                false,
            ),
        };
        Some(Self {
            x,
            y,
            radius,
            color,
            alert,
        })
    }
}

fn coverage(distance: f32, pixel: f32) -> f32 {
    (0.5 - distance / pixel).clamp(0.0, 1.0)
}

/// The outline and filled globes share the same geometry. All states and
/// animation frames use the same canvas to keep the tray item the same size.
fn globe_rgba(size: u32, state: TrayVisualState, angle: f32, template: bool) -> Vec<u8> {
    let pixel = 1.0 / size as f32;
    let badge = Badge::for_state(state, angle);
    let connected = state == TrayVisualState::Connected;
    let globe_color = if connected { [94, 234, 158] } else { [244; 3] };
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    for y in 0..size {
        for x in 0..size {
            let x = (x as f32 + 0.5) * pixel;
            let y = (y as f32 + 0.5) * pixel;
            let dx = x - 0.5;
            let dy = y - 0.5;
            let distance = dx.hypot(dy);
            let rim = (distance - 0.337).abs() - 0.036;
            let longitude = ((dx / 0.154).hypot(dy / 0.337) - 1.0).abs() * 0.154 - 0.027;
            let equator = (dy.abs() - 0.029).max(distance - 0.337);
            let grid = longitude.min(equator);
            let mut globe = if connected {
                // Carve the grid into the disk, leaving a continuous rim.
                let cutout = (grid + 0.002).max(distance - 0.313);
                coverage(distance - 0.373, pixel) * (1.0 - coverage(cutout, pixel))
            } else {
                coverage(rim.min(grid), pixel)
            };

            let mark = if let Some(badge) = badge {
                let bx = x - badge.x;
                let by = y - badge.y;
                let distance = bx.hypot(by);
                globe *= 1.0 - coverage(distance - badge.radius - 0.035, pixel);
                let mark = coverage(distance - badge.radius, pixel);
                if badge.alert {
                    let stem = bx.hypot(by.clamp(-0.065, 0.012) - by) - 0.023;
                    let dot = bx.hypot(by - 0.054) - 0.025;
                    mark * (1.0 - coverage(stem.min(dot), pixel))
                } else {
                    mark
                }
            } else {
                0.0
            };

            let glyph_alpha = mark + globe * (1.0 - mark);
            if template {
                rgba.extend([0, 0, 0, (glyph_alpha * 255.0).round() as u8]);
            } else {
                // A dark backing keeps the white globe readable on light and dark taskbars.
                let backing = coverage(distance - 0.46, pixel) * (1.0 - glyph_alpha);
                let alpha = glyph_alpha + backing;
                let badge_color = badge.map_or(globe_color, |badge| badge.color);
                for ((badge_channel, globe_channel), dark) in badge_color
                    .into_iter()
                    .zip(globe_color)
                    .zip([18.0, 27.0, 43.0])
                {
                    let color = badge_channel as f32 * mark
                        + globe_channel as f32 * globe * (1.0 - mark)
                        + dark * backing;
                    rgba.push((color / alpha.max(f32::EPSILON)).round() as u8);
                }
                rgba.push((alpha * 255.0).round() as u8);
            }
        }
    }

    rgba
}

fn build_image(state: TrayVisualState, angle: f32) -> gpui_tray::Result<Icon> {
    #[cfg(target_os = "macos")]
    {
        const SIZE: u32 = 36;
        let bitmap = Icon::from_rgba(globe_rgba(SIZE, state, angle, true), SIZE, SIZE)
            .scale_factor(2.0)
            .template(true)
            .build()?;
        // Convert once. An animated frame can be reused without re-encoding
        // PNG or allocating a new native image every time it is displayed.
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
        const SIZE: u32 = 32;
        Icon::from_rgba(globe_rgba(SIZE, state, angle, false), SIZE, SIZE).build()
    }
}

const ORBIT_START: f32 = -std::f32::consts::FRAC_PI_2;

pub fn build_tray_icon(state: TrayVisualState) -> gpui_tray::Result<Icon> {
    build_image(state, ORBIT_START)
}

#[cfg(target_os = "macos")]
pub const ORBIT_FRAMES: usize = 36;

#[cfg(target_os = "macos")]
pub fn build_orbit_frames() -> gpui_tray::Result<Vec<Icon>> {
    (0..ORBIT_FRAMES)
        .map(|frame| {
            let angle = ORBIT_START + std::f32::consts::TAU * frame as f32 / ORBIT_FRAMES as f32;
            build_image(TrayVisualState::Transitioning, angle)
        })
        .collect()
}
