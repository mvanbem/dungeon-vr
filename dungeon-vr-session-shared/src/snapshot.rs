use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::convert::Infallible;

use bevy_ecs::prelude::*;
use bevy_ecs::world::EntityMut;
use dungeon_vr_stream_codec::{ReadBoolError, ReadError, ReadStringError, StreamCodec};
use rapier3d::na::Isometry3;

use slotmap::Key;
use thiserror::Error;

use crate::core::{Authority, NetId, ReadNetIdError, Synchronized, TransformComponent};
use crate::fly_around::FlyAroundComponent;
use crate::physics::PhysicsComponent;
use crate::physics::{NetPhysicsMode, PhysicsResource};
use crate::render::{ModelHandle, RenderComponent};
use crate::resources::EntitiesByNetId;

#[derive(Error, Debug)]
pub enum ReadSnapshotError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    ReadBoolError(#[from] ReadBoolError),

    #[error("{0}")]
    ReadStringError(#[from] ReadStringError),

    #[error("{0}")]
    ReadNetIdError(#[from] ReadNetIdError),

    #[error("invalid game state token: 0x{0:02x}")]
    InvalidGameStateToken(u8),

    #[error("invalid net physics mode: 0x{0:02x}")]
    InvalidNetPhysicsMode(u8),
}

pub fn write_snapshot(w: &mut Vec<u8>, world: &mut World) -> Result<(), Infallible> {
    // Gather a sorted list entities by net ID.
    let mut entities_by_net_id = world
        .query::<(&Synchronized, Entity)>()
        .iter(world)
        .map(|(&synchronized, entity)| (synchronized.net_id, entity))
        .collect::<Vec<_>>();
    entities_by_net_id.sort_unstable_by_key(|&(net_id, _)| net_id);

    (entities_by_net_id.len() as u32).write_to(w)?;
    for (net_id, entity) in entities_by_net_id {
        let entity = world.entity(entity);
        net_id.write_to(w)?;
        let authority = entity.get::<Authority>().unwrap();
        authority.write_to(w)?;
        if let Some(transform) = entity.get::<TransformComponent>() {
            1u8.write_to(w)?;
            transform.0.write_to(w)?;
        }
        if let Some(render) = entity.get::<RenderComponent>() {
            2u8.write_to(w)?;
            render.model_name.write_to(w)?;
        }
        if let Some(physics) = entity.get::<PhysicsComponent>() {
            3u8.write_to(w)?;
            physics.collider_name.write_to(w)?;
            match physics.mode {
                NetPhysicsMode::Static => 0u8,
                NetPhysicsMode::Dynamic { ccd_enabled: false } => 1u8,
                NetPhysicsMode::Dynamic { ccd_enabled: true } => 2u8,
            }
            .write_to(w)?;
        }
        if let Some(_) = entity.get::<FlyAroundComponent>() {
            0xffu8.write_to(w)?;
        }
        0u8.write_to(w)?;
    }
    Ok(())
}

pub fn apply_snapshot(r: &mut &[u8], world: &mut World) -> Result<(), ReadSnapshotError> {
    world.resource_scope(|world, mut physics_resource: Mut<PhysicsResource>| {
        let count = u32::read_from(r)?;
        let entities_by_net_id = world
            .query::<(&Synchronized, Entity)>()
            .iter(world)
            .map(|(&net_id, entity)| (net_id.net_id, entity))
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
                    entity.insert(Synchronized { net_id });
                    entity
                }
            };

            // Update the authority component.
            let authority = Authority::read_from(r)?;
            entity.insert(authority);

            // Decode and update this entity's other components.
            let mut transform = None;
            let mut render = None;
            let mut physics = None;
            let mut flies_around = None;
            loop {
                match u8::read_from(r)? {
                    0 => break,
                    1 => {
                        let isometry = Isometry3::<f32>::read_from(r)?;
                        transform = Some(TransformComponent(isometry));
                    }
                    2 => {
                        let name = String::read_from(r)?;
                        render = Some(RenderComponent {
                            model_name: name,
                            model_handle: ModelHandle::null(),
                        });
                    }
                    3 => {
                        let collider_name = String::read_from(r)?;
                        let mode = match u8::read_from(r)? {
                            0 => NetPhysicsMode::Static,
                            1 => NetPhysicsMode::Dynamic { ccd_enabled: false },
                            2 => NetPhysicsMode::Dynamic { ccd_enabled: true },
                            x => return Err(ReadSnapshotError::InvalidNetPhysicsMode(x)),
                        };
                        physics = Some(PhysicsComponent {
                            collider_name,
                            mode,
                            collider: None,
                            rigid_body: None,
                        });
                    }
                    0xff => {
                        flies_around = Some(FlyAroundComponent);
                    }
                    token => return Err(ReadSnapshotError::InvalidGameStateToken(token)),
                }
            }
            update_component(entity.borrow_mut(), transform);
            update_component(entity.borrow_mut(), render);
            update_component_with_on_destroy(entity.borrow_mut(), physics, |physics| {
                physics.destroy(&mut physics_resource);
            });
            update_component(entity.borrow_mut(), flies_around);
        }
        Ok(())
    })
}

fn update_component<T: Component>(entity: &mut EntityMut, new_value: Option<T>) {
    update_component_with_on_destroy(entity, new_value, |_| ());
}

fn update_component_with_on_destroy<T: Component>(
    entity: &mut EntityMut,
    new_value: Option<T>,
    on_destroy: impl FnOnce(T),
) {
    match (entity.get_mut(), new_value) {
        (Some(mut component), Some(new_value)) => *component = new_value,
        (Some(_), None) => on_destroy(entity.remove::<T>().unwrap()),
        (None, Some(new_value)) => drop(entity.insert(new_value)),
        (None, None) => (),
    }
}
