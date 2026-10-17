//! gilrs to bevy_input, in place of `bevy::gilrs::GilrsPlugin`, which cannot recover a pad that
//! gilrs missed coming back. On the Switch a suspend re-creates the built-in controller's evdev
//! device; gilrs logs the removal, then never sees the new node (its udev monitor drops it), so
//! the game sat deaf to the pad after every wake. Here a pad gone for [`RETRY_SECS`] while the
//! kernel lists one by its name again ([`kernel_has`]) restarts gilrs, whose start-up enumeration
//! finds it, and it rejoins as the same bevy [`Gamepad`] entity it was.
//!
//! The event translation and the rumble are bevy_gilrs 0.18's own (`gilrs_system.rs`,
//! `converter.rs`, `rumble.rs`), cut to what this mode uses.

use bevy::input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent, GamepadRumbleRequest,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::platform::cell::SyncCell;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use gilrs::ev::filter::axis_dpad_to_button;
use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay};
use gilrs::{EventType, Filter, GamepadId, Gilrs};

/// Seconds between restarts while a pad that was connected stays missing.
const RETRY_SECS: f32 = 2.0;
/// Restarts per disappearance: each leaves the old gilrs's hotplug thread parked until the next
/// udev event, so a pad the kernel lists but nobody may open is not retried forever.
const MAX_RETRIES: u32 = 10;

#[derive(Resource)]
struct Link {
    gilrs: Option<SyncCell<Gilrs>>,
    pads: Pads,
    /// Playing rumbles and the real time each ends; dropping an effect stops it.
    rumbles: Vec<(f32, SyncCell<gilrs::ff::Effect>)>,
}

#[derive(Default)]
struct Pads {
    /// The current gilrs's ids; a restart renumbers, so this is rebuilt then.
    entities: HashMap<GamepadId, Entity>,
    /// A pad's entity by name, kept across restarts so a returning pad keeps its entity.
    by_name: HashMap<String, Entity>,
    /// Pads seen connected and now gone, by name, with the real time they went.
    missing: HashMap<String, f32>,
    retry_at: f32,
    retries: u32,
    /// The start-up pads are announced on the first run, as bevy_gilrs's `PreStartup` pass does.
    announced: bool,
}

pub(super) fn plugin(app: &mut App) {
    let gilrs = start();
    app.insert_resource(Link {
        gilrs,
        pads: Pads::default(),
        rumbles: Vec::new(),
    })
    .add_message::<GamepadRumbleRequest>()
    .add_systems(PreUpdate, pump.before(bevy::input::InputSystems))
    .add_systems(PostUpdate, rumble);
}

fn start() -> Option<SyncCell<Gilrs>> {
    // bevy_gilrs's settings: bevy_input does its own deadzones and keeps its own state.
    match gilrs::GilrsBuilder::new()
        .with_default_filters(false)
        .set_update_state(false)
        .build()
    {
        Ok(g) => Some(SyncCell::new(g)),
        Err(err) => {
            error!("gamepad: failed to start gilrs: {err}");
            None
        }
    }
}

/// Every gilrs event since the last frame, as bevy_input's raw gamepad messages; and the restart.
fn pump(
    time: Res<Time<Real>>,
    mut link: ResMut<Link>,
    mut commands: Commands,
    mut raw: MessageWriter<RawGamepadEvent>,
    mut connections: MessageWriter<GamepadConnectionEvent>,
    mut buttons: MessageWriter<RawGamepadButtonChangedEvent>,
    mut axes: MessageWriter<RawGamepadAxisChangedEvent>,
) {
    let now = time.elapsed_secs();
    let Link {
        gilrs: cell,
        pads: link,
        rumbles,
    } = &mut *link;

    if !link.missing.is_empty() && now >= link.retry_at && link.retries < MAX_RETRIES {
        link.retry_at = now + RETRY_SECS;
        if link
            .missing
            .iter()
            .any(|(name, gone)| now - gone >= RETRY_SECS && kernel_has(name))
        {
            link.retries += 1;
            info!(
                "gamepad: a pad is back in the kernel but not in gilrs; restarting gilrs ({}/{MAX_RETRIES})",
                link.retries
            );
            rumbles.clear();
            *cell = start();
            link.entities.clear();
            link.announced = false;
        }
    }

    let Some(cell) = cell.as_mut() else {
        return;
    };
    let gilrs = cell.get();

    if !link.announced {
        link.announced = true;
        let ids: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
        for id in ids {
            connect(gilrs, id, link, &mut commands, &mut raw, &mut connections);
        }
        if link.missing.is_empty() {
            link.retries = 0;
        }
    }

    while let Some(ev) = gilrs.next_event().filter_ev(&axis_dpad_to_button, gilrs) {
        gilrs.update(&ev);
        match ev.event {
            EventType::Connected => {
                connect(
                    gilrs,
                    ev.id,
                    link,
                    &mut commands,
                    &mut raw,
                    &mut connections,
                );
                if link.missing.is_empty() {
                    link.retries = 0;
                }
            }
            EventType::Disconnected => {
                let Some(&entity) = link.entities.get(&ev.id) else {
                    continue;
                };
                link.missing
                    .insert(gilrs.gamepad(ev.id).name().to_string(), now);
                link.retry_at = now + RETRY_SECS;
                let event = GamepadConnectionEvent::new(entity, GamepadConnection::Disconnected);
                raw.write(event.clone().into());
                connections.write(event);
            }
            EventType::ButtonChanged(button, value, _) => {
                let (Some(button), Some(&entity)) =
                    (convert_button(button), link.entities.get(&ev.id))
                else {
                    continue;
                };
                raw.write(RawGamepadButtonChangedEvent::new(entity, button, value).into());
                buttons.write(RawGamepadButtonChangedEvent::new(entity, button, value));
            }
            EventType::AxisChanged(axis, value, _) => {
                let (Some(axis), Some(&entity)) = (convert_axis(axis), link.entities.get(&ev.id))
                else {
                    continue;
                };
                raw.write(RawGamepadAxisChangedEvent::new(entity, axis, value).into());
                axes.write(RawGamepadAxisChangedEvent::new(entity, axis, value));
            }
            _ => {}
        }
    }
    gilrs.inc();
}

/// Play [`GamepadRumbleRequest`]s on pads that have force feedback, and stop them when done.
fn rumble(
    time: Res<Time<Real>>,
    mut link: ResMut<Link>,
    mut requests: MessageReader<GamepadRumbleRequest>,
) {
    let now = time.elapsed_secs();
    let link = &mut *link;
    link.rumbles.retain(|(end, _)| *end > now);
    let Some(cell) = link.gilrs.as_mut() else {
        requests.clear();
        return;
    };
    let gilrs = cell.get();
    for request in requests.read() {
        let target = request.gamepad();
        let Some(id) = link
            .pads
            .entities
            .iter()
            .find_map(|(id, e)| (*e == target).then_some(*id))
            .filter(|id| gilrs.gamepad(*id).is_ff_supported())
        else {
            continue;
        };
        match request {
            GamepadRumbleRequest::Stop { .. } => link.rumbles.clear(),
            GamepadRumbleRequest::Add {
                duration,
                intensity,
                ..
            } => {
                let magnitude = |r: f32| (r.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
                let scheduling = Replay {
                    play_for: (*duration).into(),
                    ..Default::default()
                };
                let effect = EffectBuilder::new()
                    .add_effect(BaseEffect {
                        kind: BaseEffectType::Strong {
                            magnitude: magnitude(intensity.strong_motor),
                        },
                        scheduling,
                        ..Default::default()
                    })
                    .add_effect(BaseEffect {
                        kind: BaseEffectType::Weak {
                            magnitude: magnitude(intensity.weak_motor),
                        },
                        scheduling,
                        ..Default::default()
                    })
                    .gamepads(&[id])
                    .finish(gilrs);
                match effect.and_then(|e| e.play().map(|()| e)) {
                    Ok(e) => link
                        .rumbles
                        .push((now + duration.as_secs_f32(), SyncCell::new(e))),
                    Err(err) => debug!("gamepad: rumble failed: {err}"),
                }
            }
        }
    }
}

/// Announce a pad, on the entity it had before if it was here before.
fn connect(
    gilrs: &Gilrs,
    id: GamepadId,
    link: &mut Pads,
    commands: &mut Commands,
    raw: &mut MessageWriter<RawGamepadEvent>,
    connections: &mut MessageWriter<GamepadConnectionEvent>,
) {
    let pad = gilrs.gamepad(id);
    let name = pad.name().to_string();
    let by_name = &mut link.by_name;
    let entity = *link.entities.entry(id).or_insert_with(|| {
        *by_name
            .entry(name.clone())
            .or_insert_with(|| commands.spawn_empty().id())
    });
    link.missing.remove(&name);
    let event = GamepadConnectionEvent::new(
        entity,
        GamepadConnection::Connected {
            name,
            vendor_id: pad.vendor_id(),
            product_id: pad.product_id(),
        },
    );
    raw.write(event.clone().into());
    connections.write(event);
}

/// Whether the kernel lists an input device of this name with an evdev node
/// (`/proc/bus/input/devices`: a `N: Name="…"` line, then `H: Handlers=… eventN`).
fn kernel_has(name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else {
        return false;
    };
    let want = format!("N: Name=\"{name}\"");
    text.split("\n\n").any(|dev| {
        dev.lines().any(|l| l == want)
            && dev
                .lines()
                .any(|l| l.starts_with("H: Handlers=") && l.contains("event"))
    })
}

fn convert_button(button: gilrs::Button) -> Option<GamepadButton> {
    use gilrs::Button as B;
    Some(match button {
        B::South => GamepadButton::South,
        B::East => GamepadButton::East,
        B::North => GamepadButton::North,
        B::West => GamepadButton::West,
        B::C => GamepadButton::C,
        B::Z => GamepadButton::Z,
        B::LeftTrigger => GamepadButton::LeftTrigger,
        B::LeftTrigger2 => GamepadButton::LeftTrigger2,
        B::RightTrigger => GamepadButton::RightTrigger,
        B::RightTrigger2 => GamepadButton::RightTrigger2,
        B::Select => GamepadButton::Select,
        B::Start => GamepadButton::Start,
        B::Mode => GamepadButton::Mode,
        B::LeftThumb => GamepadButton::LeftThumb,
        B::RightThumb => GamepadButton::RightThumb,
        B::DPadUp => GamepadButton::DPadUp,
        B::DPadDown => GamepadButton::DPadDown,
        B::DPadLeft => GamepadButton::DPadLeft,
        B::DPadRight => GamepadButton::DPadRight,
        B::Unknown => return None,
    })
}

fn convert_axis(axis: gilrs::Axis) -> Option<GamepadAxis> {
    use gilrs::Axis as A;
    Some(match axis {
        A::LeftStickX => GamepadAxis::LeftStickX,
        A::LeftStickY => GamepadAxis::LeftStickY,
        A::LeftZ => GamepadAxis::LeftZ,
        A::RightStickX => GamepadAxis::RightStickX,
        A::RightStickY => GamepadAxis::RightStickY,
        A::RightZ => GamepadAxis::RightZ,
        // `axis_dpad_to_button` turns the d-pad axes into buttons.
        A::Unknown | A::DPadX | A::DPadY => return None,
    })
}
