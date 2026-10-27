//! The continent run: the hero crosses the Eastern Kingdoms zone by zone instead of holding one
//! battleground. Each zone's centre is a stop; reaching one brings that zone's horde, and each zone
//! entered on the way brings its boss (the start and the final zone excepted). The route guides
//! rather than fences: the hero may run anywhere on the continent, and the run is won at the last
//! stop.

use std::f32::consts::FRAC_PI_2;

use bevy::light::NotShadowCaster;
use bevy::mesh::ConeMeshBuilder;
use bevy::prelude::*;

use super::data::{self, EnemyDef, MapDef, Roster};
use super::enemies::Enemy;
use super::hero::Hero;
use super::hud::{centered_bundle, GOLD_TONE};
use super::units::{flat, wow_to_bevy, yaw_toward};
use super::Run;

/// The routes, by the name of the [`MapDef`] that starts them: zone names in order, each resolved
/// through [`data::zone`] for its centre and roster. Consecutive zones touch, so the run crosses
/// the continent without a gap.
pub(super) static ROUTES: &[(&str, &[&str])] = &[(
    "Eastern Kingdoms Run",
    &[
        "Westfall",
        "Elwynn Forest",
        "Duskwood",
        "Redridge Mountains",
        "Burning Steppes",
        "Searing Gorge",
        "Dun Morogh",
        "Wetlands",
        "Hillsbrad Foothills",
        "Alterac Mountains",
        "Tirisfal Glades",
        "Western Plaguelands",
        "Naxxramas",
    ],
)];

/// Run speed on a route, over the class's base: 14 yd/s before haste, faster than a mount.
const RUN_SPEED: f32 = 2.0;
/// The stream window's outer floor on a route, in chunks (26 ≈ 867 yd): the reach the view
/// distance's maximum already gives, so a run ahead of the reference ring's edge does not stall.
pub(super) const ROUTE_STREAM_CHUNKS: i32 = 26;
/// Within this many yards of a zone's centre, the zone is reached.
const STOP_RADIUS: f32 = 150.0;
/// Horde further behind the hero than this is removed; the spawner keeps the horde ahead.
const CULL_YD: f32 = 150.0;

/// The route's readout under the banner: the next stop, how far, and the stops reached.
#[derive(Component)]
pub(super) struct RouteText;

/// The arrow over the hero, turned by [`show_route`] to the next stop.
#[derive(Component)]
pub(super) struct RouteArrow;

/// How far above the hero's feet the arrow hovers, in yards.
const ARROW_HOVER: f32 = 4.0;

/// The route's readout, under the banner.
pub(super) fn spawn_route(root: &mut ChildSpawnerCommands, font: &Handle<Font>) {
    root.spawn(Node {
        position_type: PositionType::Absolute,
        top: Val::Percent(30.0),
        width: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        ..default()
    })
    .with_children(|col| {
        col.spawn((
            RouteText,
            centered_bundle(font, "", 16.0, GOLD_TONE),
            Visibility::Hidden,
        ));
    });
}

/// The arrow: a gold shaft and a four-sided head, three yards from tail to tip. Built once at
/// startup and hidden until a route is walked; it is not a run entity, so it outlives the runs.
pub(super) fn spawn_arrow(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let gold = materials.add(StandardMaterial {
        base_color: GOLD_TONE,
        unlit: true,
        ..default()
    });
    let shaft = meshes.add(Cuboid::new(0.3, 0.3, 2.0));
    // Four sides rather than a smooth cone, so the head reads as a point.
    let head = meshes.add(
        ConeMeshBuilder {
            cone: Cone {
                radius: 0.5,
                height: 1.0,
            },
            resolution: 4,
            ..default()
        }
        .build(),
    );
    commands
        .spawn((RouteArrow, Transform::default(), Visibility::Hidden))
        .with_children(|arrow| {
            arrow.spawn((
                Mesh3d(shaft),
                MeshMaterial3d(gold.clone()),
                Transform::from_xyz(0.0, 0.0, 0.5),
                NotShadowCaster,
            ));
            // A cone points up its own Y; this turns the tip to the arrow's forward, -Z.
            arrow.spawn((
                Mesh3d(head),
                MeshMaterial3d(gold),
                Transform::from_xyz(0.0, 0.0, -1.0)
                    .with_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
                NotShadowCaster,
            ));
        });
}

/// The turn that points the arrow's tip along the ground toward `to`.
fn arrow_turn(to: Vec3) -> Quat {
    Quat::from_rotation_y(yaw_toward(flat(to)))
}

/// Hangs the arrow over the hero, turned to the next stop, and words the readout. Both hide off a
/// route, once it is walked, and while the hero is down.
pub(super) fn show_route(
    run: Res<Run>,
    hero: Query<&Transform, With<Hero>>,
    mut arrows: Query<(&mut Transform, &mut Visibility), (With<RouteArrow>, Without<Hero>)>,
    mut labels: Query<(&mut Text, &mut Visibility), (With<RouteText>, Without<RouteArrow>)>,
) {
    let aim = match (run.route, hero.single()) {
        (Some(route), Ok(h)) if run.dead_t.is_none() => {
            route.get(run.stops).map(|&zone| (zone, h.translation))
        }
        _ => None,
    };
    let Some((zone, at)) = aim else {
        for (_, mut vis) in &mut arrows {
            *vis = Visibility::Hidden;
        }
        for (_, mut vis) in &mut labels {
            *vis = Visibility::Hidden;
        }
        return;
    };
    let to = flat(zone_centre(zone) - at);
    let zones = run.route.map_or(0, <[&str]>::len);
    for (mut tf, mut vis) in &mut arrows {
        tf.translation = at + Vec3::Y * ARROW_HOVER;
        tf.rotation = arrow_turn(to);
        *vis = Visibility::Visible;
    }
    for (mut text, mut vis) in &mut labels {
        let line = format!(
            "Next: {zone} · {:.0} yd · {} of {zones} zones",
            to.length(),
            run.stops
        );
        if text.0 != line {
            text.0 = line;
        }
        *vis = Visibility::Inherited;
    }
}

/// The route a map starts, if it is a continent run.
pub(super) fn route_of(map: &MapDef) -> Option<&'static [&'static str]> {
    ROUTES
        .iter()
        .find(|(name, _)| *name == map.name)
        .map(|(_, zones)| *zones)
}

/// The index in [`data::MAPS`] of the run the picker offers: the first route's start.
pub(super) fn run_start() -> usize {
    data::MAPS
        .iter()
        .position(|m| m.name == ROUTES[0].0)
        .expect("the run's start is in MAPS")
}

/// A route zone's roster index and definition; the test checks every route names one.
fn zone_def(zone: &str) -> (usize, &'static MapDef) {
    data::zone(zone).unwrap_or_else(|| panic!("route zone {zone} is in no zone table"))
}

fn zone_centre(zone: &str) -> Vec3 {
    wow_to_bevy(zone_def(zone).1.center)
}

/// The zone the hero stands in: the last stop reached.
fn current_zone(route: &'static [&'static str], stops: usize) -> &'static str {
    route[stops.clamp(1, route.len()) - 1]
}

/// The roster the horde is drawn from: the zone the hero is crossing on a route, else the picked
/// battleground's.
pub(super) fn roster_for(run: &Run, picked: usize) -> &'static Roster {
    match run.route {
        Some(route) => data::roster(zone_def(current_zone(route, run.stops)).0),
        None => data::roster(picked),
    }
}

/// The boss a route owes next: one per zone entered, the first of that zone's roster. The start
/// zone and the last bring none.
pub(super) fn next_boss(run: &Run) -> Option<&'static EnemyDef> {
    let route = run.route?;
    let owed = (run.stops - 1).min(route.len() - 2);
    if run.bosses as usize >= owed {
        return None;
    }
    data::roster(zone_def(route[run.bosses as usize + 1]).0)
        .bosses
        .first()
}

/// The bearing from `at` to the next stop, as `atan2(z, x)` on the ground; `None` off a route or
/// once the route is walked.
pub(super) fn heading(run: &Run, at: Vec3) -> Option<f32> {
    let next = *run.route?.get(run.stops)?;
    let d = flat(zone_centre(next) - at);
    Some(d.z.atan2(d.x))
}

/// Name the route when a run starts.
pub(super) fn begin(run: &mut Run) {
    if let Some(route) = run.route {
        let last = route[route.len() - 1];
        run.banner = Some((format!("Cross the continent to {last}"), 5.0));
    }
}

impl Run {
    /// Where the run stands for its difficulty: the time, or on a route the share of the route
    /// walked, on the same scale as [`data::VICTORY_SECS`].
    pub(super) fn pace(&self) -> f32 {
        match self.route {
            Some(_) => self.route_frac * data::VICTORY_SECS,
            None => self.elapsed,
        }
    }

    /// The run is won: at the last stop of a route, or at the time limit.
    pub(super) fn finished(&self) -> bool {
        match self.route {
            Some(route) => self.stops >= route.len(),
            None => self.elapsed >= data::VICTORY_SECS,
        }
    }

    /// Speed over the class's, before haste: a route is run faster than a mount.
    pub(super) fn run_speed(&self) -> f32 {
        if self.route.is_some() {
            RUN_SPEED
        } else {
            1.0
        }
    }
}

/// The share of the route walked to `at`: the legs behind the last stop reached, plus the way along
/// the leg toward the next.
fn route_frac(points: &[Vec3], reached: usize, at: Vec3) -> f32 {
    let leg = |w: &[Vec3]| w[0].distance(w[1]);
    let total: f32 = points.windows(2).map(leg).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let behind: f32 = points[..reached].windows(2).map(leg).sum();
    let along = match (
        reached.checked_sub(1).and_then(|i| points.get(i)),
        points.get(reached),
    ) {
        (Some(&a), Some(&b)) => {
            let ab = b - a;
            let t = ((at - a).dot(ab) / ab.length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
            t * ab.length()
        }
        _ => 0.0,
    };
    (behind + along) / total
}

/// Reach the stops in turn, announce each, and keep the share of the route walked.
pub(super) fn track_route(mut run: ResMut<Run>, hero: Query<&Transform, With<Hero>>) {
    let Some(route) = run.route else {
        return;
    };
    if run.dead_t.is_some() || run.finished() {
        return;
    }
    let Ok(h) = hero.single() else {
        return;
    };
    let at = flat(h.translation);
    while !run.finished() {
        let zone = route[run.stops];
        if flat(zone_centre(zone) - at).length() > STOP_RADIUS {
            break;
        }
        run.stops += 1;
        if !run.finished() {
            let next = route[run.stops];
            run.banner = Some((format!("Entering {zone}. Next: {next}"), 4.0));
        }
    }
    if run.finished() {
        run.route_frac = 1.0;
        return;
    }
    let points: Vec<Vec3> = route.iter().map(|&z| flat(zone_centre(z))).collect();
    run.route_frac = run.route_frac.max(route_frac(&points, run.stops, at));
}

/// Horde left far behind is removed. Bosses stay, to be fought on the way.
pub(super) fn cull_behind(
    mut commands: Commands,
    run: Res<Run>,
    hero: Query<&Transform, With<Hero>>,
    enemies: Query<(Entity, &Transform, &Enemy)>,
) {
    if run.route.is_none() || run.dead_t.is_some() {
        return;
    }
    let Ok(h) = hero.single() else {
        return;
    };
    let at = flat(h.translation);
    for (e, tf, enemy) in &enemies {
        if !enemy.boss && flat(tf.translation - at).length() > CULL_YD {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_resolve_to_zones_with_hordes_bosses_and_leash() {
        for &(name, zones) in ROUTES {
            let start = data::MAPS
                .iter()
                .find(|m| m.name == name)
                .expect("the route's map");
            assert!(zones.len() >= 3, "{name} needs stops between its ends");
            assert_eq!(start.center, zone_def(zones[0]).1.center);
            for zone in zones {
                assert!(data::zone(zone).is_some(), "{zone} is not a zone");
                let roster = data::roster(zone_def(zone).0);
                assert!(roster.enemies.len() >= 3, "{zone} needs a horde");
                assert!(!roster.bosses.is_empty(), "{zone} needs a boss");
                let from_start = flat(zone_centre(zone) - zone_centre(zones[0])).length();
                assert!(from_start < start.leash, "{zone} lies beyond the leash");
            }
        }
    }

    #[test]
    fn arrow_points_at_the_next_stop() {
        for to in [
            Vec3::X,
            -Vec3::Z,
            Vec3::new(1.0, 9.0, -2.0),
            Vec3::new(-3.0, 0.0, 4.0),
        ] {
            let tip = arrow_turn(to) * Vec3::NEG_Z;
            assert!(tip.distance(flat(to).normalize()) < 1e-5, "{to:?}");
        }
    }

    #[test]
    fn route_frac_counts_legs_behind_and_along() {
        let p = [
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 100.0),
        ];
        assert!((route_frac(&p, 1, Vec3::new(50.0, 0.0, 0.0)) - 0.25).abs() < 1e-4);
        assert!((route_frac(&p, 2, Vec3::new(100.0, 0.0, 50.0)) - 0.75).abs() < 1e-4);
    }
}
