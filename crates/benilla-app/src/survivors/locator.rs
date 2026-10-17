//! The hero's locator: a gold ring on the ground around his feet, drawn as UI over the world so a
//! tree crown, a roof or a hill never hides where he is. The camera sweep (`hero::place_camera`)
//! keeps solid geometry out of the way, but canopies and most small doodads carry no camera
//! collision, and doodads merge into shared meshes, so fading them one by one is no option.
//!
//! The ring is the ground circle seen from the camera: its four rim points ±right and ±ahead of
//! the hero are projected, so it flattens with the tilt and shrinks with the zoom like a selection
//! circle would.

use benilla_world::view::WorldCamera;
use bevy::prelude::*;

use super::hero::Hero;
use super::{Phase, Run};

/// The ring's radius on the ground, yards.
const RADIUS_YD: f32 = 1.1;
const BORDER_PX: f32 = 2.0;
const COLOR: Color = Color::srgba(1.0, 0.82, 0.2, 0.75);

#[derive(Component)]
pub(super) struct Locator;

pub(super) fn setup(mut commands: Commands) {
    commands.spawn((
        Locator,
        Node {
            position_type: PositionType::Absolute,
            border: UiRect::all(Val::Px(BORDER_PX)),
            border_radius: BorderRadius::all(Val::Percent(50.0)),
            display: Display::None,
            ..default()
        },
        BorderColor::all(COLOR),
        // Over the world, under the run HUD (`hud::setup_hud`'s 400).
        GlobalZIndex(390),
    ));
}

/// Seat the ring on this frame's camera, after `place_camera`.
pub(super) fn place(
    phase: Res<State<Phase>>,
    run: Res<Run>,
    hero: Query<&Transform, (With<Hero>, Without<WorldCamera>)>,
    cams: Query<(&Camera, &Transform), With<WorldCamera>>,
    mut ring: Query<&mut Node, With<Locator>>,
) {
    let Ok(mut node) = ring.single_mut() else {
        return;
    };
    let shown = matches!(
        *phase.get(),
        Phase::Playing | Phase::LevelUp | Phase::Paused
    ) && run.dead_t.is_none();
    let rim = (|| {
        if !shown {
            return None;
        }
        let feet = hero.single().ok()?.translation;
        let (cam, cam_tf) = cams.single().ok()?;
        let cam_g = GlobalTransform::from(*cam_tf);
        let right = cam_tf.right().with_y(0.0).normalize_or_zero();
        let ahead = cam_tf.forward().with_y(0.0).normalize_or_zero();
        let at = |p: Vec3| cam.world_to_viewport(&cam_g, p).ok();
        let center = at(feet)?;
        let width = at(feet + right * RADIUS_YD)?.distance(at(feet - right * RADIUS_YD)?);
        let height = at(feet + ahead * RADIUS_YD)?.distance(at(feet - ahead * RADIUS_YD)?);
        Some((center, Vec2::new(width, height.max(BORDER_PX * 3.0))))
    })();
    let Some((center, size)) = rim else {
        if node.display != Display::None {
            node.display = Display::None;
        }
        return;
    };
    let want = (
        Val::Px(center.x - size.x / 2.0),
        Val::Px(center.y - size.y / 2.0),
        Val::Px(size.x),
        Val::Px(size.y),
    );
    if node.display != Display::Flex || (node.left, node.top, node.width, node.height) != want {
        node.display = Display::Flex;
        (node.left, node.top, node.width, node.height) = want;
    }
}
