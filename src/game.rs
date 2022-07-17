use bevy_ecs::prelude::*;
use cgmath::{vec3, Decomposed, Matrix4, Quaternion, Vector3, Zero};
use slotmap::SecondaryMap;

use crate::asset::{MeshAssetKey, MeshAssets};

#[derive(Component)]
struct Transform(Decomposed<Vector3<f32>, Quaternion<f32>>);

impl Default for Transform {
    fn default() -> Self {
        Self(Decomposed {
            scale: 1.0,
            rot: Quaternion::zero(),
            disp: Vector3::zero(),
        })
    }
}

#[derive(Component)]
struct MeshRenderer {
    mesh_key: MeshAssetKey,
}

#[derive(Component)]
struct Hand {
    index: usize,
}

pub struct VrTracking {
    pub head: openxr::Posef,
    pub hands: [VrHand; 2],
}

pub struct VrHand {
    pub pose: openxr::Posef,
}

#[derive(Default)]
struct Meshes(SecondaryMap<MeshAssetKey, Vec<Matrix4<f32>>>);

pub struct Game {
    world: World,
    schedule: Schedule,
}

impl Game {
    pub fn new(mesh_assets: &mut MeshAssets) -> Self {
        let mut world = World::new();

        for index in 0..2 {
            world
                .spawn()
                .insert(Transform::default())
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("placeholder"),
                })
                .insert(Hand { index });
        }

        for x in -1..=1 {
            for z in -1..=1 {
                world
                    .spawn()
                    .insert(Transform(Decomposed {
                        scale: 1.0,
                        rot: Quaternion::zero(),
                        disp: vec3(4.0 * x as f32, 0.0, 4.0 * z as f32),
                    }))
                    .insert(MeshRenderer {
                        mesh_key: mesh_assets.load("LowPolyDungeon/FloorTile"),
                    });
            }
        }

        let mut schedule = Schedule::default();
        schedule.add_stage("update", SystemStage::parallel().with_system(update_hands));
        schedule.add_stage_after(
            "update",
            "render",
            SystemStage::parallel().with_system(gather_meshes),
        );

        Self { world, schedule }
    }

    pub fn update(
        &mut self,
        vr_tracking: VrTracking,
    ) -> SecondaryMap<MeshAssetKey, Vec<Matrix4<f32>>> {
        self.world.insert_resource(Meshes::default());
        self.world.insert_resource(vr_tracking);

        self.schedule.run(&mut self.world);

        self.world.remove_resource::<Meshes>().unwrap().0
    }
}

fn update_hands(mut query: Query<(&mut Transform, &Hand)>, vr_tracking: Res<VrTracking>) {
    for (mut transform, hand) in query.iter_mut() {
        let pose = &vr_tracking.hands[hand.index].pose;
        transform.0 = Decomposed {
            scale: 1.0,
            rot: Quaternion::new(
                pose.orientation.w,
                pose.orientation.x,
                pose.orientation.y,
                pose.orientation.z,
            ),
            disp: vec3(pose.position.x, pose.position.y, pose.position.z),
        };
    }
}

fn gather_meshes(query: Query<(&Transform, &MeshRenderer)>, mut meshes: ResMut<Meshes>) {
    for (transform, mesh_renderer) in query.iter() {
        meshes
            .0
            .entry(mesh_renderer.mesh_key)
            .unwrap()
            .or_default()
            .push(transform.0.into());
    }
}
