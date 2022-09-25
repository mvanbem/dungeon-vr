use std::collections::HashMap;
use std::convert::Infallible;

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, ReadStringError, StreamCodec};
use rapier3d::na::Isometry3;

use thiserror::Error;

use crate::components::net::{Authority, NetId, ReadNetIdError};
use crate::components::physics::Physics;
use crate::components::render::ModelName;
use crate::components::spatial::{FliesAround, Transform};
use crate::physics::GamePhysics;
use crate::resources::EntitiesByNetId;

#[derive(Error, Debug)]
pub enum ReadSnapshotError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    ReadStringError(#[from] ReadStringError),

    #[error("{0}")]
    ReadNetIdError(#[from] ReadNetIdError),

    #[error("invalid game state token: 0x{0:02x}")]
    InvalidGameStateToken(u8),

    #[error("invalid rigid body type: 0x{0:02x}")]
    InvalidRigidBodyType(u8),
}

pub fn write_snapshot(w: &mut Vec<u8>, world: &mut World) -> Result<(), Infallible> {
    // Gather a sorted list entities by net ID.
    let mut entities_by_net_id = world
        .query::<(&NetId, Entity)>()
        .iter(world)
        .map(|(&net_id, entity)| (net_id, entity))
        .collect::<Vec<_>>();
    entities_by_net_id.sort_unstable_by_key(|&(net_id, _)| net_id);

    (entities_by_net_id.len() as u32).write_to(w)?;
    for (net_id, entity) in entities_by_net_id {
        let entity = world.entity(entity);
        net_id.write_to(w)?;
        let authority = entity.get::<Authority>().unwrap();
        authority.write_to(w)?;
        if let Some(transform) = entity.get::<Transform>() {
            1u8.write_to(w)?;
            transform.0.write_to(w)?;
        }
        if let Some(model_name) = entity.get::<ModelName>() {
            2u8.write_to(w)?;
            model_name.0.write_to(w)?;
        }
        // if let Some(physics) = entity.get::<Physics>() {
        //     3u8.write_to(w)?;
        //     match rigid_body {
        //         RigidBody::Static => 0u8.write_to(w)?,
        //         RigidBody::Dynamic { .. } => 1u8.write_to(w)?,
        //     }
        // }
        if let Some(_) = entity.get::<FliesAround>() {
            0xffu8.write_to(w)?;
        }
        0u8.write_to(w)?;
    }
    Ok(())
}

pub fn apply_snapshot(r: &mut &[u8], world: &mut World) -> Result<(), ReadSnapshotError> {
    world.resource_scope(|world, mut game_physics: Mut<GamePhysics>| {
        let GamePhysics {
            bodies,
            colliders,
            islands,
            impulse_joints,
            multibody_joints,
            ..
        } = &mut *game_physics;

        let count = u32::read_from(r)?;
        let entities_by_net_id = world
            .query::<(&NetId, Entity)>()
            .iter(world)
            .map(|(&net_id, entity)| (net_id, entity))
            .collect::<HashMap<_, _>>();

        for _ in 0..count {
            // Get or create the referenced entity.
            let net_id = NetId::read_from(r)?;
            let mut entity = match entities_by_net_id.get(&net_id).copied() {
                Some(entity) => world.entity_mut(entity),
                None => {
                    let entity = world.spawn().id();
                    world
                        .resource_mut::<EntitiesByNetId>()
                        .0
                        .insert(net_id, entity);

                    let mut entity = world.entity_mut(entity);
                    entity.insert(net_id);
                    entity
                }
            };

            // Update the authority component.
            let authority = Authority::read_from(r)?;
            entity.insert(authority);

            // Remove all components in scope. They will be added back below if present in the snapshot.
            // TODO: This seems unnecessarily expensive. Consider computing and applying a delta.
            entity.remove::<Transform>();
            entity.remove::<ModelName>();
            entity.remove::<FliesAround>();
            if let Some(physics) = entity.remove::<Physics>() {
                colliders.remove(physics.collider, islands, bodies, true);
                if let Some(rigid_body) = physics.rigid_body {
                    bodies.remove(
                        rigid_body,
                        islands,
                        colliders,
                        impulse_joints,
                        multibody_joints,
                        true,
                    );
                }
            }

            // Update any other components defined in the snapshot.
            loop {
                match u8::read_from(r)? {
                    0 => break,
                    1 => {
                        let transform = Isometry3::<f32>::read_from(r)?;
                        entity.insert(Transform(transform));
                    }
                    2 => {
                        let name = String::read_from(r)?;
                        entity.insert(ModelName(name));
                    }
                    // 3 => match u8::read_from(r)? {
                    //     0 => {
                    //         entity.insert(RigidBody::Static);
                    //         colliders.insert(
                    //             collider_cache
                    //                 .get(BorrowedColliderCacheKey::TriangleMesh(&format!(
                    //                     "{name}_col"
                    //                 )))
                    //                 .position(transform),
                    //         );
                    //     }
                    //     1 => {
                    //         entity
                    //             .insert(Transform(transform))
                    //             .insert(ModelName(name.to_string()));
                    //         self.colliders.insert(
                    //             self.collider_cache
                    //                 .get(BorrowedColliderCacheKey::TriangleMesh(&format!(
                    //                     "{name}_col"
                    //                 )))
                    //                 .position(transform),
                    //         );
                    //     }
                    //     x => Err(ReadSnapshotError::InvalidRigidBodyType(x)),
                    // },
                    0xff => {
                        entity.insert(FliesAround);
                    }
                    token => return Err(ReadSnapshotError::InvalidGameStateToken(token)),
                }
            }
        }
        Ok(())
    })
}
