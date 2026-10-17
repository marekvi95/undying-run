//! Knobs for a small handheld (the Switch launcher sets them; unset, nothing changes):
//!
//! - `SURVIVORS_MAX_FPS`: cap the frame rate. 30 halves the Tegra's GPU load, heat and drain;
//!   the frame pacer then snaps every frame to two refreshes.
//! - `SURVIVORS_UI_SCALE`: scale the bevy_ui screens (HUD, cards, pause, results) for a 5.5"
//!   panel, through [`UiScale`].
//!
//! Native only: the browser paces itself to `requestAnimationFrame` and has its own zoom.

use std::time::{Duration, Instant};

use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    if let Some(fps) = env_f32("SURVIVORS_MAX_FPS").filter(|f| (5.0..=240.0).contains(f)) {
        info!("handheld: frame rate capped at {fps}");
        app.insert_resource(FrameCap {
            period: Duration::from_secs_f32(1.0 / fps),
            last: None,
        })
        .add_systems(Last, cap_frame);
    }
    if let Some(scale) = env_f32("SURVIVORS_UI_SCALE").filter(|s| (0.5..=3.0).contains(s)) {
        info!("handheld: UI scale {scale}");
        app.insert_resource(UiScale(scale));
    }
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

#[derive(Resource)]
struct FrameCap {
    period: Duration,
    last: Option<Instant>,
}

/// Sleep out what is left of the frame's slot, last thing in the frame.
fn cap_frame(mut cap: ResMut<FrameCap>) {
    let now = Instant::now();
    let next = match cap.last {
        // Fell behind by more than a slot (a load, a stall): start the cadence over from now.
        Some(last) if now < last + cap.period * 2 => last + cap.period,
        _ => now,
    };
    if next > now {
        std::thread::sleep(next - now);
    }
    cap.last = Some(next.max(now));
}
