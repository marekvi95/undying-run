//! The hero: its body, WASD movement over the terrain, the top-down camera, and its health.

use benilla_world::terrain_stream::StreamLookahead;
use benilla_world::view::WorldCamera;
use benilla_world::world_point::WorldPoint;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use super::continent::{route_of, ROUTE_STREAM_CHUNKS};
use super::data::{self, ClassDef};
use super::units::{self, flat, wow_to_bevy, yaw_toward, Fx, Marks, UnitSpec};
use super::{Phase, Run, Selection};
use crate::creature_anim::{move_flags, MovementState};
use crate::net::{FieldChanged, Guid, ObjectStore};

#[derive(Component)]
pub(super) struct Hero {
    /// Vertical velocity in yards/second while airborne; 0 on the ground.
    pub(super) vy: f32,
    /// Height above the ground while airborne. Kept here, not in the transform: the creature
    /// ground clamp (`net::motion::ground_clamp_creatures`) re-seats every unit on the floor at
    /// the top of the frame, so the arc is re-applied over the ground each frame.
    pub(super) air: f32,
}

/// The body radius enemies stop at.
pub(super) const HERO_RADIUS: f32 = 0.7;

/// The hop's initial upward speed and the gravity that brings it back down — the reference
/// `CMovement` jump seed and gravity (`player/state.rs`'s `JUMP_SPEED` / `GRAVITY`).
const JUMP_SPEED: f32 = 7.955_547;
const GRAVITY: f32 = 19.291_105;

/// Radians per raw mouse unit while right-dragging: the hero's turn and the camera's tilt.
const ORBIT_RATE: f32 = 0.008;
/// Radians a second the keys turn the hero (A/D and the arrows).
const TURN_RATE: f32 = 3.0;
/// Radians a second of orbit (yaw) and tilt (pitch) at full right-stick deflection.
const STICK_YAW_RATE: f32 = 2.6;
const STICK_PITCH_RATE: f32 = 1.4;
/// Elevation clamp (radians): ~10°..~83° above the horizon, so the camera never dives underground.
const PITCH_MIN: f32 = 0.18;
const PITCH_MAX: f32 = 1.45;
/// The camera's tilt at the start and after a recentre.
const DEFAULT_PITCH: f32 = 61.0 * std::f32::consts::PI / 180.0;
/// How far ahead of a running hero the pad-steered camera looks, as a share of the zoom and at
/// most yards, so a close camera never leads him off the screen; and how fast (1/s) it eases.
const LEAD_SHARE: f32 = 0.15;
const LEAD_YD: f32 = 4.0;
const LEAD_RATE: f32 = 2.5;
/// A hero step longer than this in one frame is a blink or a respawn, not a run.
const LEAD_JUMP_YD: f32 = 3.0;
/// Wheel zoom range, yards from the hero's shoulders.
const ZOOM_MIN: f32 = 1.5;
const ZOOM_MAX: f32 = 70.0;

#[derive(Resource)]
pub(super) struct CamRig {
    pub dist: f32,
    pub orbit: f32,
    /// The hero's heading (radians); the camera sits behind him along it. Turned by A/D, right-drag
    /// and the pad's right stick.
    pub yaw: f32,
    /// Elevation above the horizon (radians), clamped so the camera never dives underground.
    pub pitch: f32,
    /// The camera's eased look-ahead on the ground, and the hero's last focus it is measured from.
    lead: Vec3,
    last_focus: Option<Vec3>,
}

impl Default for CamRig {
    fn default() -> Self {
        Self {
            dist: 30.0,
            orbit: 0.6,
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            lead: Vec3::ZERO,
            last_focus: None,
        }
    }
}

/// The gamepad's steering in play (`gamepad.rs`, native only; absent, or inactive while the mouse
/// is in use). Both are stick-space: +x right, +y up the screen.
#[derive(Resource, Default, Clone, Copy, PartialEq)]
pub(super) struct PadSteer {
    /// Whether the pad, not the mouse and keyboard, is steering.
    pub(super) active: bool,
    /// Unit run direction, or zero.
    pub(super) walk: Vec2,
    /// The right stick, past its deadzone: the camera swing and tilt.
    pub(super) look: Vec2,
}

impl PadSteer {
    fn steering(pad: Option<&Self>) -> Option<&Self> {
        pad.filter(|p| p.active)
    }
}

/// Stick-space to the ground plane, turned by the camera's yaw: screen-up is away from the lens.
fn stick_to_ground(v: Vec2, yaw: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (x, z) = (v.x, -v.y);
    Vec3::new(x * cy + z * sy, 0.0, z * cy - x * sy)
}

/// The hero's forward and right on the ground for a heading `yaw`; a yaw of 0 faces −Z.
fn heading(yaw: f32) -> (Vec3, Vec3) {
    let (sy, cy) = yaw.sin_cos();
    (Vec3::new(-sy, 0.0, -cy), Vec3::new(cy, 0.0, -sy))
}

/// Where a battleground's runs start.
pub(super) fn arena(map: &data::MapDef) -> Vec3 {
    wow_to_bevy(map.center)
}

/// The world is the battleground being stood in ([`Selection::stage`]): entering one on another
/// continent worldports, which raises the loading screen until the new ground is in. A route
/// keeps its stream window a ring wider, so a run that outruns the reference ring does not stall.
pub(super) fn follow_map(
    selection: Res<Selection>,
    current: Option<ResMut<benilla_world::world_map::CurrentMap>>,
    lookahead: Option<ResMut<StreamLookahead>>,
) {
    let stage = selection.stage_def();
    if let Some(mut current) = current {
        if current.0 != stage.map {
            current.0 = stage.map;
        }
    }
    if let Some(mut lookahead) = lookahead {
        let floor = if route_of(stage).is_some() {
            ROUTE_STREAM_CHUNKS
        } else {
            0
        };
        if lookahead.outer_floor_chunks != floor {
            lookahead.outer_floor_chunks = floor;
        }
    }
}

/// The hero in the class trainer's look, weapons drawn.
pub(super) fn spawn_hero(
    commands: &mut Commands,
    class: &'static ClassDef,
    display: u32,
    race: u8,
    sex: u8,
    serial: u64,
    pos: Vec3,
) -> Entity {
    let bytes0 = u32::from(race)
        | u32::from(class.id) << 8
        | u32::from(sex) << 16
        | u32::from(class.power) << 24;
    let e = units::spawn_unit(
        commands,
        &UnitSpec {
            serial,
            display,
            scale: 1.0,
            level: 60,
            faction: units::FACTION_FRIENDLY,
            bytes0,
            weapons: class.weapons,
            sheath: class.sheath,
        },
        pos,
        0.0,
    );
    commands
        .entity(e)
        .insert((Hero { vy: 0.0, air: 0.0 }, Marks::buffs()));
    e
}

/// Midday over the battleground, whatever the clock says.
pub(super) const MINUTE_OF_DAY: u32 = 780;

pub(super) fn daylight(debug: Option<ResMut<benilla_world::dev_state::DebugState>>) {
    let Some(mut debug) = debug else { return };
    if debug.lighting.follow_server_time || debug.lighting.manual_minute != MINUTE_OF_DAY {
        debug.lighting.follow_server_time = false;
        debug.lighting.manual_minute = MINUTE_OF_DAY;
    }
}

/// The keys and mouse run the hero over the terrain within the battleground's leash, and stream the
/// focus on him.
#[allow(clippy::too_many_arguments)]
pub(super) fn drive_hero(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    phase: Res<State<Phase>>,
    run: Res<Run>,
    selection: Res<Selection>,
    mut rig: ResMut<CamRig>,
    world: WorldPoint,
    auto: Option<Res<super::auto::Auto>>,
    pad: Option<Res<PadSteer>>,
    mut player: ResMut<crate::player::Player>,
    mut hero: Query<(&mut Transform, &mut MovementState, &mut Hero)>,
) {
    let focus = match hero.single_mut() {
        Ok((mut tf, mut mv, mut hm)) => {
            let dt = time.delta_secs();
            let pad = PadSteer::steering(pad.as_deref());
            let playing = *phase.get() == Phase::Playing && run.dead_t.is_none();
            let mut dir = Vec3::ZERO;
            // Classic keys: his heading is the camera's yaw, so he runs away from the lens. A/D turn
            // him, W/S run, Q/E strafe, and right-drag turns him too; both buttons run forward.
            let mut forward = None;
            if let Some(auto) = auto.as_deref().filter(|_| playing) {
                dir = super::auto::steer(auto);
            } else if let Some(pad) = pad.filter(|_| playing) {
                dir = stick_to_ground(pad.walk, rig.yaw);
            } else if playing {
                let turn = f32::from(keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]))
                    - f32::from(keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]));
                rig.yaw += turn * TURN_RATE * dt;
                if mouse.pressed(MouseButton::Right) {
                    rig.yaw -= motion.delta.x * ORBIT_RATE;
                }
                rig.yaw = rig.yaw.rem_euclid(std::f32::consts::TAU);
                let both = mouse.pressed(MouseButton::Left) && mouse.pressed(MouseButton::Right);
                let ahead = f32::from(keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) || both)
                    - f32::from(keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]));
                let side =
                    f32::from(keys.pressed(KeyCode::KeyE)) - f32::from(keys.pressed(KeyCode::KeyQ));
                let (look, right) = heading(rig.yaw);
                dir = right * side + look * ahead;
                forward = Some(look);
            }
            let mut pos = tf.translation;
            // Classic, he faces his heading; the pad and the autopilot turn him toward their run.
            if forward.is_some() {
                tf.rotation = Quat::from_rotation_y(rig.yaw);
            }
            if dir != Vec3::ZERO && dt > 0.0 {
                let dir = dir.normalize();
                let speed = run.stats().speed * run.haste_mult() * run.run_speed();
                pos += dir * speed * dt;
                mv.speed = speed;
                mv.flags = match forward {
                    Some(look) => gait(look, dir),
                    None => {
                        let target = Quat::from_rotation_y(yaw_toward(dir));
                        tf.rotation = tf.rotation.slerp(target, (dt * 20.0).min(1.0));
                        move_flags::FORWARD
                    }
                };
            } else if mv.speed != 0.0 || mv.flags != 0 {
                mv.speed = 0.0;
                mv.flags = 0;
            }
            let map = selection.stage_def();
            let center = arena(map);
            let off = flat(pos - center);
            if off.length() > map.leash * 2.0 {
                // Just entered another battleground: start at its centre.
                pos = center;
            } else if off.length() > map.leash {
                let held = center + off.normalize() * map.leash;
                pos.x = held.x;
                pos.z = held.z;
            }
            let ground = world.terrain_height_under(pos).unwrap_or(pos.y);
            // Space hops: lift off the terrain, then gravity brings the hero back down.
            if playing && keys.just_pressed(KeyCode::Space) && hm.vy == 0.0 && hm.air == 0.0 {
                hm.vy = JUMP_SPEED;
            }
            if playing && (hm.vy != 0.0 || hm.air > 0.0) {
                hm.vy -= GRAVITY * dt;
                hm.air += hm.vy * dt;
                if hm.air <= 0.0 {
                    hm.air = 0.0;
                    hm.vy = 0.0;
                }
            } else {
                hm.air = 0.0;
                hm.vy = 0.0;
            }
            pos.y = ground + hm.air;
            // Feed the animation driver: `vertical_speed`'s sign and the `FALLING` flag are what
            // select the Jump (37/38) / Fall (40) animation (`creature_anim::select`).
            mv.vertical_speed = hm.vy;
            if hm.vy != 0.0 {
                mv.flags |= move_flags::FALLING;
            }
            if tf.translation != pos {
                tf.translation = pos;
            }
            pos
        }
        Err(_) => {
            let mut p = arena(selection.stage_def());
            if let Some(h) = world.terrain_height_under(p) {
                p.y = h;
            }
            p
        }
    };
    // The terrain streams and the loading screen waits on the ground under this.
    player.active = true;
    player.detached = false;
    player.settling = false;
    player.world_stale = false;
    player.pos = focus;
}

/// A slow orbit at the menu; in play, the camera follows the hero's heading, with the right stick
/// or right-drag swinging and tilting it and `C` recentring it behind him.
pub(super) fn place_camera(
    time: Res<Time<Real>>,
    phase: Res<State<Phase>>,
    mut rig: ResMut<CamRig>,
    mut wheel: MessageReader<MouseWheel>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    player: Res<crate::player::Player>,
    world: WorldPoint,
    pad: Option<Res<PadSteer>>,
    hero: Query<&Transform, (With<Hero>, Without<WorldCamera>)>,
    mut cams: Query<&mut Transform, With<WorldCamera>>,
) {
    // `C` swings the camera back behind the hero and resets its tilt. On the keyboard the camera
    // already follows the heading, so this matters for the pad, whose right stick swings it free.
    if keys.just_pressed(KeyCode::KeyC) {
        if let Ok(tf) = hero.single() {
            rig.yaw = yaw_toward(flat(tf.rotation * Vec3::NEG_Z));
            rig.pitch = DEFAULT_PITCH;
        }
    }
    // With no hero yet, the focus `drive_hero` grounded.
    let focus = hero.single().map_or(player.pos, |t| t.translation);
    // Multiplicative, so a notch moves as far by eye close in as far out; all the way down to an
    // over-the-shoulder view.
    for w in wheel.read() {
        let notches = match w.unit {
            MouseScrollUnit::Line => w.y,
            MouseScrollUnit::Pixel => w.y * 0.02,
        };
        rig.dist = (rig.dist * (1.0 - 0.12 * notches).clamp(0.5, 1.5)).clamp(ZOOM_MIN, ZOOM_MAX);
    }
    let (eye, look) = if *phase.get() == Phase::Menu {
        rig.orbit += time.delta_secs() * 0.2;
        let d = 7.0;
        (
            focus + Vec3::new(rig.orbit.sin() * d, 2.4, rig.orbit.cos() * d),
            focus + Vec3::Y * 1.1,
        )
    } else {
        // Right-drag tilts the camera; the swing is the hero's turn in `drive_hero`.
        if mouse.pressed(MouseButton::Right) {
            rig.pitch = (rig.pitch + motion.delta.y * ORBIT_RATE).clamp(PITCH_MIN, PITCH_MAX);
        }
        // The right stick swings and tilts it, as in a console third-person game:
        // right swings the camera right, up tilts it toward the horizon.
        if let Some(pad) = PadSteer::steering(pad.as_deref()) {
            let dt = time.delta_secs();
            rig.yaw -= pad.look.x * STICK_YAW_RATE * dt;
            rig.pitch =
                (rig.pitch - pad.look.y * STICK_PITCH_RATE * dt).clamp(PITCH_MIN, PITCH_MAX);
        }
        let (sy, cy) = rig.yaw.sin_cos();
        let (sp, cp) = rig.pitch.sin_cos();
        // Pad-steered, the view eases a few yards ahead of a running hero, to see what he runs
        // into.
        let dt = time.delta_secs();
        let step = rig.last_focus.map_or(Vec3::ZERO, |last| flat(focus - last));
        rig.last_focus = Some(focus);
        let want = if PadSteer::steering(pad.as_deref()).is_some()
            && *phase.get() == Phase::Playing
            && step.length() < LEAD_JUMP_YD
        {
            step.normalize_or_zero() * (rig.dist * LEAD_SHARE).min(LEAD_YD)
        } else {
            Vec3::ZERO
        };
        if dt > 0.0 {
            rig.lead = rig.lead.lerp(want, 1.0 - (-LEAD_RATE * dt).exp());
        }
        let look = focus + rig.lead + Vec3::Y * 1.4;
        let mut eye = look + Vec3::new(cp * sy, sp, cp * cy) * rig.dist;
        // Never under the ground: a low tilt on a slope would bury the lens.
        if let Some(h) = world.terrain_height_under(eye) {
            eye.y = eye.y.max(h + 0.6);
        }
        (eye, look)
    };
    let placed = Transform::from_translation(eye).looking_at(look, Vec3::Y);
    for mut cam in &mut cams {
        *cam = placed;
    }
}

/// Damage the hero takes from one blow, after immunity, armour and the shield; returns what got
/// through.
pub(super) fn hurt_hero(run: &mut Run, raw: f32) -> f32 {
    if run.immune_t > 0.0 || run.dead_t.is_some() {
        return 0.0;
    }
    let mut dmg = (raw - run.stats().armor).max(1.0);
    if run.shield_t > 0.0 && run.shield > 0.0 {
        let absorbed = dmg.min(run.shield);
        run.shield -= absorbed;
        dmg -= absorbed;
    }
    run.hp -= dmg;
    dmg
}

/// The clock, regeneration, the timed buffs and their visuals, death and victory.
pub(super) fn hero_vitals(
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut next: ResMut<NextState<Phase>>,
    mut hero: Query<(Entity, &Guid, &mut ObjectStore, &mut Marks), With<Hero>>,
    mut fields: MessageWriter<FieldChanged>,
    mut fx: Fx,
) {
    let dt = time.delta_secs();
    let stats = run.stats();
    let Ok((entity, guid, mut store, mut marks)) = hero.single_mut() else {
        return;
    };
    if let Some(t) = run.dead_t.as_mut() {
        *t += dt;
        if *t > 3.0 {
            next.set(Phase::GameOver);
        }
        return;
    }
    run.elapsed += dt;
    let r = &mut *run;
    for t in [&mut r.shield_t, &mut r.immune_t, &mut r.thorns_t] {
        *t = (*t - dt).max(0.0);
    }
    let mut heal = stats.regen * dt;
    if run.heal_t > 0.0 {
        heal += run.heal_rate * dt.min(run.heal_t);
        run.heal_t = (run.heal_t - dt).max(0.0);
    }
    run.hp = (run.hp + heal).min(stats.max_hp);

    // The buffs show as their own state kits: the shield bubble, the immunity glow, the orbs.
    let mut shown: Vec<u32> = Vec::new();
    if run.shield_t > 0.0 && run.shield > 0.0 {
        shown.push(17);
    }
    if run.immune_t > 0.0 {
        shown.push(642);
    }
    if run.thorns_t > 0.0 {
        shown.push(run.thorns_spell);
    }
    if run.heal_t > 0.0 {
        shown.push(774);
    }
    for owned in &run.abilities {
        if owned.def.key == "lightning_shield" {
            shown.push(owned.def.spell);
        }
    }
    if run.passives.iter().any(|(p, _)| p.key == "renew") {
        shown.push(139);
    }
    marks.set(&shown);
    marks.flush(entity, guid.0, &mut store, &mut fields);

    if run.hp <= 0.0 {
        run.hp = 0.0;
        run.dead_t = Some(0.0);
        units::kill_unit(&mut store);
        marks.clear();
        marks.flush(entity, guid.0, &mut store, &mut fields);
    } else if run.finished() {
        run.won = true;
        // A last flourish before the results.
        fx.cast(entity, 642, &[entity], None);
        next.set(Phase::GameOver);
    }
}

/// The ground-plane distance from the hero, the measure every range in the mode uses.
/// The movement flags for running `dir` while facing `look`: forward, backpedal or strafe, by
/// which quarter the run falls in, so the walk, backpedal and strafe animations play.
fn gait(look: Vec3, dir: Vec3) -> u32 {
    let ahead = look.dot(dir);
    // Positive when `dir` is to the hero's left: WoW faces −Z with +X on the right.
    let left = look.cross(dir).y;
    let quarter = std::f32::consts::FRAC_1_SQRT_2;
    if ahead >= quarter {
        move_flags::FORWARD
    } else if ahead <= -quarter {
        move_flags::BACKWARD
    } else if left > 0.0 {
        move_flags::STRAFE_LEFT
    } else {
        move_flags::STRAFE_RIGHT
    }
}

pub(super) fn reach(hero: Vec3, other: Vec3) -> f32 {
    flat(other - hero).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys run and strafe along the heading the camera follows, and the hero's model faces
    /// the way `yaw_toward` reads back, so the camera and the hero always agree.
    #[test]
    fn heading_runs_along_the_camera_and_faces_it() {
        let (look, right) = heading(0.0);
        assert!(look.distance(Vec3::NEG_Z) < 1e-5);
        assert!(right.distance(Vec3::X) < 1e-5);
        for yaw in [0.0_f32, 1.0, std::f32::consts::FRAC_PI_2, 4.0] {
            let (look, _) = heading(yaw);
            let model_facing = Quat::from_rotation_y(yaw_toward(look)) * Vec3::NEG_Z;
            assert!(model_facing.distance(look) < 1e-5, "yaw {yaw}");
        }
    }
}
