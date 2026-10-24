//! Gamepad play for Warcraft Survivors. 1.12.1 has no gamepad and this mode has no reference to
//! match (`CONTROLS.md`); the pad instead drives the same keys, mouse cursor and wheel the
//! keyboard and mouse already feed, so `hero`, `mobility`, `hud`, `menu` and `cards` need no pad
//! code of their own, except that movement and facing in play go through [`PadSteer`] (a mouse
//! cursor left wherever the right stick last put it made the hero backpedal away from it). A
//! face-button click lands on whatever bevy_ui button the cursor sits over, the same path a real
//! mouse click takes ([`Interaction`]); [`nav`] snaps the cursor from button to button.
//!
//! | Pad | Does the same as |
//! |---|---|
//! | Left stick, D-pad (play) | [`PadSteer::walk`]: camera-relative movement, the hero faces the run |
//! | Right stick (play) | [`PadSteer::look`]: swing and tilt the camera |
//! | D-pad, left stick (outside play) | [`nav`]: jump the cursor to the next button that way |
//! | Right stick (outside play) | moving the mouse freely |
//! | South (B) | `Space`, jump |
//! | East (A) | `F`, the class mobility spell; click in menus |
//! | North | `C`: recentre a swung camera (else the lock/unlock) |
//! | Start | `Escape`, pause; `Enter` at the menu, Enter World |
//! | Select (−), paused or results | quit the game |
//! | Left / right shoulder | the mouse wheel, zoom out / in |
//!
//! The pad pauses the run when it drops out or the machine sleeps ([`auto_pause`]), and rumbles
//! when the hero is hit ([`rumble_on_hurt`]; [`link`] plays it).
//!
//! The first pad to connect also tunes the camera for a handheld: closer, lower, and easing a few
//! yards ahead of the run while the pad steers (`hero::place_camera`).
//!
//! Native only: `benilla_world::boot::tuned_default_plugins` disables bevy's gilrs backend for
//! the keyboard/mouse-only 1.12 client; this mode drives gilrs itself ([`link`]), so it can bring
//! a pad back after a suspend; gilrs has no wasm32 backend
//! wired in here, so the module is cfg'd out of the web build.

mod link;
mod nav;

use std::hash::Hash;

use std::time::Duration;

use bevy::input::gamepad::{
    GamepadConnection, GamepadConnectionEvent, GamepadRumbleIntensity, GamepadRumbleRequest,
};
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use super::hero::PadSteer;
use super::{Phase, Run};

/// Stick deflection below which a stick is treated as at rest. The Switch Lite's left stick rests
/// as far as 0.16 off centre and drifts past that, so this is wide.
const DEADZONE: f32 = 0.3;
/// The virtual cursor's speed at full stick deflection, logical pixels a second.
const CURSOR_SPEED: f32 = 1200.0;
/// Wheel notches a second while a shoulder button is held (`MouseScrollUnit::Line`, as a real
/// wheel sends), on top of the one notch a press sends at once, so a tap zooms a visible step.
const ZOOM_RATE: f32 = 8.0;
/// A frame longer than this, in real seconds, was a suspend (or a stall as bad): pause the run.
const SLEPT_SECS: f32 = 1.0;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(link::plugin)
        .init_resource::<PadSteer>()
        .add_systems(
            PreUpdate,
            (auto_pause, drive_gamepad, nav::navigate)
                .chain()
                .after(bevy::input::InputSystems)
                // The virtual click and cursor reach bevy_ui's hover/press pass the same frame.
                .before(bevy::ui::UiSystems::Focus),
        )
        .add_systems(Update, rumble_on_hurt);
}

/// The whole pad, once a frame, before anything else reads a key, the cursor or the wheel.
fn drive_gamepad(
    time: Res<Time>,
    pads: Query<&Gamepad>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut wheel: MessageWriter<MouseWheel>,
    mut exit: MessageWriter<AppExit>,
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

    // ── Free cursor, outside play only: the right stick (the left one and the D-pad snap, `nav`). ──
    let cursor_stick = right;
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

    // Select quits from the pause menu or the results; a clean exit, so the launcher stops too.
    if matches!(*phase.get(), Phase::Paused | Phase::GameOver)
        && pad.just_pressed(GamepadButton::Select)
    {
        exit.write(AppExit::Success);
    }

    // ── Shoulder buttons: the mouse wheel, zoom out / in. ──
    for (button, sign) in [
        (GamepadButton::LeftTrigger, -1.0),
        (GamepadButton::RightTrigger, 1.0),
    ] {
        let mut notches = 0.0;
        if pad.just_pressed(button) {
            notches += 1.0;
        } else if pad.pressed(button) {
            notches += ZOOM_RATE * time.delta_secs();
        }
        if notches > 0.0 {
            wheel.write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y: sign * notches,
                window: window_entity,
            });
        }
    }
}

/// Pause the run when the pad drops out, or after a frame as long as a suspend: coming back to
/// the Switch from sleep finds the run waiting rather than the hero dead.
fn auto_pause(
    time: Res<Time<Real>>,
    phase: Res<State<Phase>>,
    mut connections: MessageReader<GamepadConnectionEvent>,
    mut next: ResMut<NextState<Phase>>,
) {
    let dropped = connections
        .read()
        .any(|c| matches!(c.connection, GamepadConnection::Disconnected));
    if *phase.get() == Phase::Playing && (dropped || time.delta_secs() > SLEPT_SECS) {
        next.set(Phase::Paused);
    }
}

/// A short buzz when the hero is hit, harder the bigger the blow, and a long one when he dies.
fn rumble_on_hurt(
    run: Res<Run>,
    phase: Res<State<Phase>>,
    pads: Query<Entity, With<Gamepad>>,
    mut last: Local<Option<f32>>,
    mut rumble: MessageWriter<GamepadRumbleRequest>,
) {
    let was = last.replace(run.hp);
    let Some(was) = was.filter(|_| *phase.get() == Phase::Playing) else {
        return;
    };
    let lost = was - run.hp;
    if lost <= 0.5 {
        return;
    }
    let (strength, ms) = if run.hp <= 0.0 {
        (1.0, 450)
    } else {
        (
            (lost / run.stats().max_hp.max(1.0) * 5.0).clamp(0.25, 0.9),
            110,
        )
    };
    for gamepad in &pads {
        rumble.write(GamepadRumbleRequest::Add {
            gamepad,
            duration: Duration::from_millis(ms),
            intensity: GamepadRumbleIntensity {
                strong_motor: strength,
                weak_motor: strength * 0.6,
            },
        });
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
