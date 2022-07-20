use bevy_ecs::prelude::*;
use cgmath::{vec3, Decomposed, Deg, InnerSpace, Matrix4, One, Quaternion, Rotation3, Vector3};
use ordered_float::NotNan;
use slotmap::SecondaryMap;

use crate::asset::{MeshAssetKey, MeshAssets};

#[derive(Component)]
struct Transform(Decomposed<Vector3<f32>, Quaternion<f32>>);

impl Default for Transform {
    fn default() -> Self {
        Self(One::one())
    }
}

#[derive(Component)]
struct MeshRenderer {
    mesh_key: MeshAssetKey,
}

#[derive(Component)]
struct Hand {
    index: usize,
    grab_state: HandGrabState,
}

enum HandGrabState {
    Empty,
    Grabbing(Entity),
}

#[derive(Component)]
struct Grabbable {
    grabbed: bool,
}

struct VrTrackingState {
    current: VrTracking,
    prev: VrTracking,
}

#[derive(Clone, Copy, Default)]
pub struct VrTracking {
    pub head: openxr::Posef,
    pub hands: [VrHand; 2],
}

#[derive(Clone, Copy, Default)]
pub struct VrHand {
    pub pose: openxr::Posef,
    pub squeeze: f32,
    pub squeeze_force: f32,
}

impl VrHand {
    fn to_decomposed(&self) -> Decomposed<Vector3<f32>, Quaternion<f32>> {
        Decomposed {
            scale: 1.0,
            rot: Quaternion::new(
                self.pose.orientation.w,
                self.pose.orientation.x,
                self.pose.orientation.y,
                self.pose.orientation.z,
            ),
            disp: vec3(
                self.pose.position.x,
                self.pose.position.y,
                self.pose.position.z,
            ),
        }
    }
}

#[derive(Default)]
struct Meshes(SecondaryMap<MeshAssetKey, Vec<Matrix4<f32>>>);

pub struct Game {
    world: World,
    schedule: Schedule,
    prev_vr_tracking: VrTracking,
}

impl Game {
    pub fn new(mesh_assets: &mut MeshAssets) -> Self {
        let mut world = World::new();

        for index in 0..2 {
            world
                .spawn()
                .insert(Transform::default())
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load(["left_hand", "right_hand"][index]),
                })
                .insert(Hand {
                    index,
                    grab_state: HandGrabState::Empty,
                });
        }

        world
            .spawn()
            .insert(Transform(Decomposed {
                disp: vec3(0.0, 0.0, 0.0),
                ..One::one()
            }))
            .insert(MeshRenderer {
                mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Custom_Center"),
            });
        for side in 0..4 {
            let rot = Quaternion::from_angle_y(Deg(90.0) * side as f32);
            world
                .spawn()
                .insert(Transform(Decomposed {
                    rot,
                    disp: rot * vec3(0.0, 0.0, -4.0),
                    ..One::one()
                }))
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Custom_Border_Flat"),
                });
            world
                .spawn()
                .insert(Transform(Decomposed {
                    rot,
                    disp: rot * vec3(0.0, 0.0, -4.0),
                    ..One::one()
                }))
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Wall_Var1"),
                });

            world
                .spawn()
                .insert(Transform(Decomposed {
                    rot,
                    disp: rot * vec3(4.0, 0.0, 4.0),
                    ..One::one()
                }))
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Custom_Corner_Flat"),
                });
            world
                .spawn()
                .insert(Transform(Decomposed {
                    rot,
                    disp: rot * vec3(-4.0, 0.0, -4.0),
                    ..One::one()
                }))
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Wall_Var1"),
                });
            world
                .spawn()
                .insert(Transform(Decomposed {
                    rot,
                    disp: rot * vec3(4.0, 0.0, -4.0),
                    ..One::one()
                }))
                .insert(MeshRenderer {
                    mesh_key: mesh_assets.load("LowPolyDungeon/Dungeon_Wall_Var1"),
                });
        }

        world
            .spawn()
            .insert(Transform(Decomposed {
                disp: vec3(0.0, 1.0, 0.0),
                ..One::one()
            }))
            .insert(MeshRenderer {
                mesh_key: mesh_assets.load("LowPolyDungeon/Key_Silver"),
            })
            .insert(Grabbable { grabbed: false });

        let mut schedule = Schedule::default();
        schedule.add_stage("update", SystemStage::parallel().with_system(update_hands));
        schedule.add_stage_after(
            "update",
            "render",
            SystemStage::parallel().with_system(gather_meshes),
        );

        Self {
            world,
            schedule,
            prev_vr_tracking: Default::default(),
        }
    }

    pub fn update(
        &mut self,
        vr_tracking: VrTracking,
    ) -> SecondaryMap<MeshAssetKey, Vec<Matrix4<f32>>> {
        self.world.insert_resource(Meshes::default());
        self.world.insert_resource(VrTrackingState {
            current: vr_tracking,
            prev: self.prev_vr_tracking,
        });

        self.schedule.run(&mut self.world);

        self.prev_vr_tracking = vr_tracking;
        self.world.remove_resource::<Meshes>().unwrap().0
    }
}

fn update_hands(
    mut query: Query<(&mut Transform, &mut Hand), Without<Grabbable>>,
    vr_tracking: Res<VrTrackingState>,
    mut grabbable_query: Query<(Entity, &mut Transform, &mut Grabbable)>,
) {
    for (mut transform, mut hand) in query.iter_mut() {
        let vr_hand = &vr_tracking.current.hands[hand.index];
        let prev_vr_hand = &vr_tracking.prev.hands[hand.index];
        transform.0 = vr_hand.to_decomposed()
            * Decomposed {
                rot: Quaternion::from_angle_x(Deg(25.0)),
                ..One::one()
            };

        // Step the grab state machine.
        match hand.grab_state {
            HandGrabState::Empty => {
                if vr_hand.squeeze_force > 0.2 && prev_vr_hand.squeeze_force <= 0.2 {
                    if let Some((_, entity, _, mut grabbable)) = grabbable_query
                        .iter_mut()
                        .filter_map(|(entity, grabbable_transform, grabbable)| {
                            if grabbable.grabbed {
                                return None;
                            }
                            let dist = (grabbable_transform.0.disp - transform.0.disp).magnitude();
                            if dist <= 0.1 {
                                Some((dist, entity, grabbable_transform, grabbable))
                            } else {
                                None
                            }
                        })
                        .min_by_key(|(dist, _, _, _)| NotNan::new(*dist).unwrap())
                    {
                        // Start grabbing.
                        hand.grab_state = HandGrabState::Grabbing(entity);
                        grabbable.grabbed = true;
                    }
                }
            }
            HandGrabState::Grabbing(entity) => {
                if vr_hand.squeeze < 0.8 {
                    // End grabbing.
                    hand.grab_state = HandGrabState::Empty;
                    grabbable_query
                        .get_component_mut::<Grabbable>(entity)
                        .unwrap()
                        .grabbed = false;
                }
            }
        }

        // Update a held object.
        if let HandGrabState::Grabbing(entity) = hand.grab_state {
            grabbable_query
                .get_component_mut::<Transform>(entity)
                .unwrap()
                .0 = transform.0;
        }
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
