//! Gamepad play for Warcraft Survivors. 1.12.1 has no gamepad and this mode has no reference to
//! match (`CONTROLS.md`); the pad instead drives the same keys, mouse cursor and wheel the
//! keyboard and mouse already feed, so `hero`, `mobility`, `hud`, `menu` and `cards` need no pad
//! code of their own — except that movement and facing in play go through [`PadSteer`], since a
//! mouse cursor left wherever the right stick last put it made the hero backpedal away from it — — a face-button click lands on whatever bevy_ui button the cursor sits over,
//! the same path a real mouse click takes ([`Interaction`]).
//!
//! | Pad | Does the same as |
//! |---|---|
//! | Left stick, D-pad (play) | [`PadSteer::walk`]: analog movement, the hero faces where he runs |
//! | Right stick (play) | [`PadSteer::look`]: locked camera, the hero faces it; unlocked, it swings the camera |
//! | Either stick (outside play) | moving the mouse: menu/card/pause navigation |
//! | South (B) | `Space`, jump |
//! | East (A) | `F`, the class mobility spell; click in menus |
//! | North | `C`, the camera lock/unlock |
//! | D-pad left / up / right | pick level-up card 1 / 2 / 3 |
//! | Start | `Escape`, pause; `Enter` at the menu, Enter World |
//! | Left / right shoulder | the mouse wheel, zoom out / in |
//!
//! The first pad to connect also tunes the camera for a handheld: closer, lower, and easing a few
//! yards ahead of the run while the pad steers (`hero::place_camera`).
//!
//! Native only: `benilla_world::boot::tuned_default_plugins` disables bevy's gilrs backend for
//! the keyboard/mouse-only 1.12 client; this mode drives gilrs itself ([`link`]), so it can bring
//! a pad back after a suspend; gilrs has no wasm32 backend
//! wired in here, so the module is cfg'd out of the web build.

mod link;

use std::hash::Hash;

use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::hero::{CamRig, PadSteer};
use super::Phase;

/// Stick deflection below which a stick is treated as at rest.
const DEADZONE: f32 = 0.2;
/// The virtual cursor's speed at full stick deflection, logical pixels a second.
const CURSOR_SPEED: f32 = 1200.0;
/// Wheel notches a second at full shoulder-button hold (`MouseScrollUnit::Line`, as a real wheel
/// sends).
const ZOOM_RATE: f32 = 3.0;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(link::plugin)
        .init_resource::<PadSteer>()
        .add_systems(
            PreUpdate,
            (tune_camera, drive_gamepad)
                .after(bevy::input::InputSystems)
                // The virtual click and cursor reach bevy_ui's hover/press pass the same frame.
                .before(bevy::ui::UiSystems::Focus),
        );
}

/// The whole pad, once a frame, before anything else reads a key, the cursor or the wheel.
fn drive_gamepad(
    time: Res<Time>,
    pads: Query<&Gamepad>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut wheel: MessageWriter<MouseWheel>,
    mut steer: ResMut<PadSteer>,
    motion: Res<AccumulatedMouseMotion>,
    phase: Res<State<Phase>>,
) {
    let Some(pad) = pads.iter().next() else {
        return;
    };
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };

    let playing = *phase.get() == Phase::Playing;

    // ── Sticks in play: analog walk and look, read by `drive_hero` and `place_camera`. ──
    let left = pad.left_stick();
    let right = pad.right_stick();
    let dpad = Vec2::new(
        f32::from(u8::from(pad.pressed(GamepadButton::DPadRight)))
            - f32::from(u8::from(pad.pressed(GamepadButton::DPadLeft))),
        f32::from(u8::from(pad.pressed(GamepadButton::DPadUp)))
            - f32::from(u8::from(pad.pressed(GamepadButton::DPadDown))),
    );
    let walk = if left.length() >= DEADZONE {
        left.normalize()
    } else {
        dpad.normalize_or_zero()
    };
    let look = if right.length() >= DEADZONE {
        right.normalize() * deflection(right.length())
    } else {
        Vec2::ZERO
    };
    if playing {
        // The pad takes over steering at its first touch and hands it back when the mouse moves,
        // so a mouse and keyboard still play as before on a machine with a pad plugged in.
        let touched =
            walk != Vec2::ZERO || look != Vec2::ZERO || pad.get_pressed().next().is_some();
        let active = touched || (steer.active && motion.delta == Vec2::ZERO);
        let next = PadSteer {
            active,
            walk: if active { walk } else { Vec2::ZERO },
            look: if active { look } else { Vec2::ZERO },
        };
        if *steer != next {
            *steer = next;
        }
    } else if steer.walk != Vec2::ZERO || steer.look != Vec2::ZERO {
        steer.walk = Vec2::ZERO;
        steer.look = Vec2::ZERO;
    }

    // ── Virtual cursor, outside play only: either stick, the left taking precedence. ──
    let cursor_stick = if left.length() >= DEADZONE {
        left
    } else {
        right
    };
    if !playing && cursor_stick.length() >= DEADZONE {
        let speed = CURSOR_SPEED * deflection(cursor_stick.length()).powf(1.5);
        let at = window
            .cursor_position()
            .unwrap_or_else(|| Vec2::new(window.width() / 2.0, window.height() / 2.0));
        // Window y runs down; stick y runs up.
        let to = at
            + Vec2::new(cursor_stick.x, -cursor_stick.y).normalize_or_zero()
                * speed
                * time.delta_secs();
        let to = to.clamp(
            Vec2::ZERO,
            Vec2::new(window.width() - 1.0, window.height() - 1.0),
        );
        window.set_cursor_position(Some(to));
    }

    // ── Face buttons: combat and UI never overlap. ──
    set(
        &mut keys,
        KeyCode::Space,
        playing && pad.pressed(GamepadButton::South),
    );
    set(
        &mut keys,
        KeyCode::KeyF,
        playing && pad.pressed(GamepadButton::East),
    );
    set(
        &mut keys,
        KeyCode::KeyC,
        playing && pad.pressed(GamepadButton::North),
    );

    let picking = *phase.get() == Phase::LevelUp;
    for (key, button) in [
        (KeyCode::Digit1, GamepadButton::DPadLeft),
        (KeyCode::Digit2, GamepadButton::DPadUp),
        (KeyCode::Digit3, GamepadButton::DPadRight),
    ] {
        set(&mut keys, key, picking && pad.pressed(button));
    }

    let clickable = matches!(
        *phase.get(),
        Phase::Menu | Phase::LevelUp | Phase::Paused | Phase::GameOver
    );
    set(
        &mut mouse,
        MouseButton::Left,
        clickable && pad.pressed(GamepadButton::East),
    );
    set(
        &mut keys,
        KeyCode::Enter,
        match *phase.get() {
            // Start also enters the world from the menu, wherever the cursor is.
            Phase::Menu => pad.pressed(GamepadButton::Start),
            Phase::GameOver => pad.pressed(GamepadButton::East),
            _ => false,
        },
    );
    set(
        &mut keys,
        KeyCode::Escape,
        *phase.get() != Phase::Menu && pad.pressed(GamepadButton::Start),
    );

    // ── Shoulder buttons: the mouse wheel, zoom out / in. ──
    let notches = ZOOM_RATE * time.delta_secs();
    if pad.pressed(GamepadButton::LeftTrigger) {
        wheel.write(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -notches,
            window: window_entity,
        });
    }
    if pad.pressed(GamepadButton::RightTrigger) {
        wheel.write(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: notches,
            window: window_entity,
        });
    }
}

/// The first pad to connect switches the camera to the handheld view ([`CamRig::tune_for_pad`]),
/// once: after that the player's own zoom and tilt stand.
fn tune_camera(added: Query<(), Added<Gamepad>>, mut rig: ResMut<CamRig>, mut done: Local<bool>) {
    if !*done && !added.is_empty() {
        *done = true;
        rig.tune_for_pad();
    }
}

/// `ButtonInput::press`/`release` already no-op on a repeat of the same state (only the real
/// transition sets `just_pressed`/`just_released`), so this can run every frame without faking a
/// key-repeat.
fn set<T: Clone + Eq + Hash + Send + Sync + 'static>(
    input: &mut ButtonInput<T>,
    button: T,
    held: bool,
) {
    if held {
        input.press(button);
    } else {
        input.release(button);
    }
}

/// A stick's length rescaled past the deadzone, so motion starts from zero at its edge.
fn deflection(length: f32) -> f32 {
    ((length - DEADZONE) / (1.0 - DEADZONE).max(f32::EPSILON)).clamp(0.0, 1.0)
}
