//! The hero: its body, WASD movement over the terrain, the top-down camera, and its health.

use benilla_world::view::WorldCamera;
use benilla_world::world_point::WorldPoint;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

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

/// Radians of orbit per raw mouse unit while right-dragging.
const ORBIT_RATE: f32 = 0.008;
/// Radians a second of orbit (yaw) and tilt (pitch) at full right-stick deflection.
const STICK_YAW_RATE: f32 = 2.6;
const STICK_PITCH_RATE: f32 = 1.4;
/// Elevation clamp (radians): ~10°..~83° above the horizon, so the camera never dives underground.
const PITCH_MIN: f32 = 0.18;
const PITCH_MAX: f32 = 1.45;
/// The locked view's tilt.
const FIXED_PITCH: f32 = 61.0 * std::f32::consts::PI / 180.0;
/// How far ahead of a running hero the pad-steered camera looks, as a share of the zoom and at
/// most yards, so a close camera never leads him off the screen; and how fast (1/s) it eases.
const LEAD_SHARE: f32 = 0.15;
const LEAD_YD: f32 = 4.0;
const LEAD_RATE: f32 = 2.5;
/// A hero step longer than this in one frame is a blink or a respawn, not a run.
const LEAD_JUMP_YD: f32 = 3.0;
/// How fast (1/s) the camera eases back out once whatever pulled it in clears: the client
/// camera's `CAM_RETURN_RATE`.
const PULL_RETURN_RATE: f32 = 6.0;
/// Wheel zoom range, yards from the hero's shoulders.
const ZOOM_MIN: f32 = 1.5;
const ZOOM_MAX: f32 = 70.0;

/// Whether the camera is locked; `C` toggles it.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum CamMode {
    /// Locked top-down (58°), laptop-friendly — the original feel.
    #[default]
    Fixed,
    /// Unlocked: a mouse drag swings and tilts it around the hero.
    Orbit,
}

#[derive(Resource)]
pub(super) struct CamRig {
    pub dist: f32,
    pub orbit: f32,
    /// Orbit azimuth (radians) around the hero; right-drag swings it during play.
    pub yaw: f32,
    /// Elevation above the horizon (radians), clamped so the camera never dives underground.
    pub pitch: f32,
    pub mode: CamMode,
    /// The tilt locking the camera snaps back to; the pad tunes it lower ([`CamRig::tune_for_pad`]).
    pub fixed_pitch: f32,
    /// The zoom last saved to `survivors-camera.txt`, `None` with nothing saved.
    pub saved_dist: Option<f32>,
    /// The camera's eased look-ahead on the ground, and the hero's last focus it is measured from.
    lead: Vec3,
    last_focus: Option<Vec3>,
    /// The collision-held boom length, yards; `None` until the first sweep.
    pull: Option<f32>,
}

impl CamRig {
    /// Closer and lower than the laptop view, for a handheld's small screen; a saved zoom wins.
    pub(super) fn tune_for_pad(&mut self) {
        self.fixed_pitch = 50.0_f32.to_radians();
        if self.mode == CamMode::Fixed {
            self.pitch = self.fixed_pitch;
        }
        if self.saved_dist.is_none() {
            self.dist = 20.0;
        }
    }
}

impl Default for CamRig {
    fn default() -> Self {
        Self {
            dist: 30.0,
            orbit: 0.6,
            yaw: 0.0,
            pitch: FIXED_PITCH,
            mode: CamMode::Fixed,
            fixed_pitch: FIXED_PITCH,
            saved_dist: None,
            lead: Vec3::ZERO,
            last_focus: None,
            pull: None,
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

/// Where a battleground's runs start.
pub(super) fn arena(map: &data::MapDef) -> Vec3 {
    wow_to_bevy(map.center)
}

/// The world is the battleground being stood in ([`Selection::stage`]): entering one on another
/// continent worldports, which raises the loading screen until the new ground is in.
pub(super) fn follow_map(
    selection: Res<Selection>,
    current: Option<ResMut<benilla_world::world_map::CurrentMap>>,
) {
    let Some(mut current) = current else {
        return;
    };
    let want = selection.stage_def().map;
    if current.0 != want {
        current.0 = want;
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

/// The ground under the mouse cursor, `None` with the cursor off the window. The locked camera's
/// hero faces it, and the aimed mobility spells (Blink, Charge) go toward it.
#[derive(Resource, Default)]
pub(super) struct AimPoint(pub(super) Option<Vec3>);

/// Cast the cursor onto the terrain: the camera ray meets the plane at the hero's height, then the
/// terrain height there, a few times over, which settles on hills within a frame.
pub(super) fn aim_cursor(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cams: Query<(&Camera, &Transform), With<WorldCamera>>,
    hero: Query<&Transform, (With<Hero>, Without<WorldCamera>)>,
    world: WorldPoint,
    pad: Option<Res<PadSteer>>,
    mut aim: ResMut<AimPoint>,
) {
    let point = (|| {
        // Steered by the pad, the cursor is wherever the right stick last left it in a menu; the
        // aimed spells go where the hero faces instead.
        if PadSteer::steering(pad.as_deref()).is_some() {
            return None;
        }
        let cursor = windows.single().ok()?.cursor_position()?;
        let (cam, tf) = cams.single().ok()?;
        let ray = cam
            .viewport_to_world(&GlobalTransform::from(*tf), cursor)
            .ok()?;
        let mut y = hero.single().ok()?.translation.y;
        let mut at = None;
        for _ in 0..4 {
            if ray.direction.y.abs() < 1e-4 {
                break;
            }
            let t = (y - ray.origin.y) / ray.direction.y;
            if t <= 0.0 {
                break;
            }
            let p = ray.origin + *ray.direction * t;
            at = Some(p);
            match world.terrain_height_under(p) {
                Some(h) if (h - y).abs() > 0.05 => y = h,
                _ => break,
            }
        }
        at
    })();
    if aim.0 != point {
        aim.0 = point;
    }
}

/// WASD over the terrain within the battleground's leash, and the streaming focus on the hero.
#[allow(clippy::too_many_arguments)]
pub(super) fn drive_hero(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    phase: Res<State<Phase>>,
    run: Res<Run>,
    selection: Res<Selection>,
    rig: Res<CamRig>,
    aim: Res<AimPoint>,
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
            let mut dir = Vec3::ZERO;
            if let Some(auto) = auto
                .as_deref()
                .filter(|_| *phase.get() == Phase::Playing && run.dead_t.is_none())
            {
                dir = super::auto::steer(auto);
            } else if let Some(pad) =
                pad.filter(|_| *phase.get() == Phase::Playing && run.dead_t.is_none())
            {
                dir = stick_to_ground(pad.walk, rig.yaw);
            } else if *phase.get() == Phase::Playing && run.dead_t.is_none() {
                if keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) {
                    dir.z -= 1.0;
                }
                if keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]) {
                    dir.z += 1.0;
                }
                if keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]) {
                    dir.x -= 1.0;
                }
                if keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]) {
                    dir.x += 1.0;
                }
                // Camera-relative: the locked view's yaw is 0 (world-relative) unless the right
                // stick has swung it.
                {
                    let (sy, cy) = rig.yaw.sin_cos();
                    dir = Vec3::new(dir.x * cy + dir.z * sy, 0.0, dir.z * cy - dir.x * sy);
                }
            }
            let mut pos = tf.translation;
            // Locked, the hero faces the cursor and WASD moves him under it, so he backpedals and
            // strafes as a player does; unlocked, driven or on the pad, he faces where he runs.
            let face = (rig.mode == CamMode::Fixed
                && auto.is_none()
                && pad.is_none()
                && *phase.get() == Phase::Playing
                && run.dead_t.is_none())
            .then(|| aim.0.map(|a| flat(a - pos)))
            .flatten()
            .filter(|d| d.length() > 0.5)
            .map(|d| d.normalize());
            if let Some(look) = face {
                let target = Quat::from_rotation_y(yaw_toward(look));
                tf.rotation = tf.rotation.slerp(target, (dt * 20.0).min(1.0));
            }
            if dir != Vec3::ZERO && dt > 0.0 {
                let dir = dir.normalize();
                let speed = run.stats().speed * run.haste_mult() * run.run_speed();
                pos += dir * speed * dt;
                mv.speed = speed;
                mv.flags = match face {
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
            let playing = *phase.get() == Phase::Playing && run.dead_t.is_none();
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

/// A slow orbit at the menu; in play, top-down until `C` unlocks it to swing and tilt.
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
    collide: benilla_world::collision::WorldCollision,
    pad: Option<Res<PadSteer>>,
    hero: Query<&Transform, (With<Hero>, Without<WorldCamera>)>,
    mut cams: Query<&mut Transform, With<WorldCamera>>,
    mut debug: Local<Option<bool>>,
    mut debug_at: Local<DebugAt>,
) {
    // `C` unlocks the camera to swing and tilt; locking it again snaps back to the top-down view,
    // so world-relative WASD lines up with the screen again. A locked view the right stick has
    // swung snaps back first, so on the pad it is the recentre button.
    if keys.just_pressed(KeyCode::KeyC) {
        let swung = rig.yaw != 0.0 || rig.pitch != rig.fixed_pitch;
        rig.mode = match rig.mode {
            CamMode::Fixed if swung => {
                rig.yaw = 0.0;
                rig.pitch = rig.fixed_pitch;
                CamMode::Fixed
            }
            CamMode::Fixed => CamMode::Orbit,
            CamMode::Orbit => {
                rig.yaw = 0.0;
                rig.pitch = rig.fixed_pitch;
                CamMode::Fixed
            }
        };
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
        // Unlocked, either mouse button drags the camera around the hero, as in WoW.
        if rig.mode == CamMode::Orbit && mouse.any_pressed([MouseButton::Left, MouseButton::Right])
        {
            rig.yaw -= motion.delta.x * ORBIT_RATE;
            rig.pitch = (rig.pitch + motion.delta.y * ORBIT_RATE).clamp(PITCH_MIN, PITCH_MAX);
        }
        // The right stick swings and tilts it in either mode, as in a console third-person game:
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
        // into; with the mouse it stays centred, so the cursor's aim stays put under the hand.
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
        // The client camera's sweep from the hero's shoulders out to the seat: a tree, a wall or a
        // hill between them pulls the camera in front of it (snapping in, easing back out), so the
        // hero is never hidden behind one.
        let boom = eye - look;
        let len = boom.length().max(1.0e-3);
        let open = collide.cast_camera(look, boom, false).unwrap_or(len);
        let pull = match rig.pull {
            Some(p) if open >= p => p + (open - p) * (1.0 - (-PULL_RETURN_RATE * dt).exp()),
            _ => open,
        }
        .min(len);
        rig.pull = Some(pull);
        eye = look + boom * (pull / len);
        if *debug.get_or_insert_with(|| std::env::var_os("SURVIVORS_CAM_DEBUG").is_some()) {
            let now = time.elapsed_secs();
            if now >= debug_at.0 {
                debug_at.0 = now + 1.0;
                info!(
                    "camera: zoom {:.1} yd, held at {pull:.1} (sweep open {open:.1}), yaw {:.2}, \
                     pitch {:.2}, lead {:.1}, pad {:?}",
                    rig.dist,
                    rig.yaw,
                    rig.pitch,
                    rig.lead.length(),
                    pad.as_deref().map(|p| (p.active, p.walk, p.look)),
                );
            }
        }
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

/// The saved zoom, `benilla-config/survivors-camera.txt` (`localStorage` on the page): the
/// camera distance in yards, kept across runs and restarts.
fn zoom_path() -> Option<std::path::PathBuf> {
    crate::local_state::home().map(|h| h.join("survivors-camera.txt"))
}

pub(super) fn restore_zoom(mut rig: ResMut<CamRig>) {
    let Some(dist) = zoom_path()
        .and_then(|p| crate::local_state::read_to_string(&p).ok())
        .and_then(|t| t.trim().parse::<f32>().ok())
        .filter(|d| d.is_finite())
    else {
        return;
    };
    rig.dist = dist.clamp(ZOOM_MIN, ZOOM_MAX);
    rig.saved_dist = Some(rig.dist);
}

/// Leaving play (pause, level-up, the results) saves a changed zoom, so a held zoom key or
/// shoulder button doesn't write every frame.
pub(super) fn save_zoom(mut rig: ResMut<CamRig>) {
    if rig.saved_dist == Some(rig.dist) {
        return;
    }
    if let Some(path) = zoom_path() {
        let _ = crate::local_state::write_atomic(&path, &format!("{:.2}", rig.dist));
    }
    rig.saved_dist = Some(rig.dist);
}

/// When `SURVIVORS_CAM_DEBUG` next logs the rig, real seconds.
#[derive(Default)]
pub(super) struct DebugAt(f32);

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
