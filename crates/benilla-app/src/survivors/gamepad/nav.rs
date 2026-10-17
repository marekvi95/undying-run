//! Snap navigation on the UI screens (menu, level-up, pause, results): a D-pad press or a left
//! stick flick jumps the cursor onto the next button in that direction, so A clicks it the way a
//! mouse would. Held, it repeats. The right stick still moves the cursor freely.
//!
//! "A button" is any visible bevy_ui node with an [`Interaction`], which is every clickable thing
//! the screens build (glue buttons, race faces, zones, cards, pause buttons); nodes as wide or tall
//! as most of the window (a backdrop, the XP bar) are skipped.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::DEADZONE;
use crate::survivors::Phase;

/// A left-stick flick counts past this deflection.
const FLICK: f32 = 0.6;
/// Held, the first repeat comes after this many seconds, then one every [`REPEAT_SECS`].
const REPEAT_DELAY_SECS: f32 = 0.35;
const REPEAT_SECS: f32 = 0.13;
/// How much a sideways offset counts against a candidate, against its distance ahead.
const SIDEWAYS_WEIGHT: f32 = 2.5;
/// Nodes bigger than this share of the window, in either axis, are backdrops, not buttons.
const MAX_SHARE: f32 = 0.6;

#[derive(Default)]
pub(super) struct Repeat {
    held: Option<IVec2>,
    next_at: f32,
}

pub(super) fn navigate(
    time: Res<Time<Real>>,
    phase: Res<State<Phase>>,
    pads: Query<&Gamepad>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    buttons: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<Interaction>>,
    mut repeat: Local<Repeat>,
) {
    let Some(pad) = pads.iter().next() else {
        return;
    };
    if *phase.get() == Phase::Playing {
        repeat.held = None;
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };

    // Window y runs down.
    let dpad = IVec2::new(
        i32::from(pad.pressed(GamepadButton::DPadRight))
            - i32::from(pad.pressed(GamepadButton::DPadLeft)),
        i32::from(pad.pressed(GamepadButton::DPadDown))
            - i32::from(pad.pressed(GamepadButton::DPadUp)),
    );
    let left = pad.left_stick();
    let flick = if left.length() >= FLICK.max(DEADZONE) {
        // The stick's dominant axis, so a slightly diagonal flick still goes straight.
        if left.x.abs() >= left.y.abs() {
            IVec2::new(left.x.signum() as i32, 0)
        } else {
            IVec2::new(0, -left.y.signum() as i32)
        }
    } else {
        IVec2::ZERO
    };
    let dir = if dpad != IVec2::ZERO { dpad } else { flick };

    let now = time.elapsed_secs();
    if dir == IVec2::ZERO {
        repeat.held = None;
        return;
    }
    if repeat.held == Some(dir) {
        if now < repeat.next_at {
            return;
        }
        repeat.next_at = now + REPEAT_SECS;
    } else {
        repeat.held = Some(dir);
        repeat.next_at = now + REPEAT_DELAY_SECS;
    }

    let size = Vec2::new(window.width(), window.height());
    let from = window.cursor_position().unwrap_or(size / 2.0);
    let d = dir.as_vec2().normalize();
    let mut best: Option<(f32, Vec2)> = None;
    for (node, at, visible) in &buttons {
        if !visible.get() {
            continue;
        }
        let scale = node.inverse_scale_factor();
        let extent = node.size() * scale;
        if extent.x < 4.0
            || extent.y < 4.0
            || extent.x > size.x * MAX_SHARE
            || extent.y > size.y * MAX_SHARE
        {
            continue;
        }
        let center = at.translation * scale;
        // The button under the cursor is where we are, not where to go.
        if (from - center).abs().cmple(extent / 2.0).all() {
            continue;
        }
        let v = center - from;
        let ahead = v.dot(d);
        if ahead <= 4.0 {
            continue;
        }
        let score = ahead + (v - d * ahead).length() * SIDEWAYS_WEIGHT;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, center));
        }
    }
    if let Some((_, to)) = best {
        window.set_cursor_position(Some(to));
    }
}
