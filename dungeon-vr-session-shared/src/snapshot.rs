use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::convert::Infallible;
use std::num::NonZeroU32;

use bevy_ecs::prelude::*;
use bevy_ecs::world::EntityMut;
use dungeon_vr_stream_codec::{ReadBoolError, ReadError, ReadStringError, StreamCodec};
use rapier3d::prelude::*;

use slotmap::Key;
use thiserror::Error;

use crate::core::{Authority, NetId, ReadNetIdError, SynchronizedComponent, TransformComponent};
use crate::fly_around::FlyAroundComponent;
use crate::interaction::{GrabbableComponent, HandComponent, HandGrabState};
use crate::physics::PhysicsComponent;
use crate::physics::{NetPhysicsMode, PhysicsResource};
use crate::render::{ModelHandle, RenderComponent};
use crate::resources::EntitiesByNetIdResource;
use crate::{NetComponent, NetComponentDestroyContext};

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

    #[error("invalid hand grab mode: 0x{0:02x}")]
    InvalidHandGrabMode(u8),
}

pub fn write_snapshot(w: &mut Vec<u8>, world: &mut World) -> Result<(), Infallible> {
    // Gather a sorted list entities by net ID.
    let mut entities_by_net_id = world
        .query::<(&SynchronizedComponent, Entity)>()
        .iter(world)
        .map(|(synchronized, entity)| (synchronized, entity))
        .collect::<Vec<_>>();
    entities_by_net_id.sort_unstable_by_key(|(synchronized, _)| synchronized.net_id);

    (entities_by_net_id.len() as u32).write_to(w)?;
    for (synchronized, entity) in entities_by_net_id {
        let entity = world.entity(entity);
        synchronized.net_id.write_to(w)?;
        synchronized.authority.write_to(w)?;
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
        if let Some(hand) = entity.get::<HandComponent>() {
            4u8.write_to(w)?;
            u8::try_from(hand.index).unwrap().write_to(w)?;
            match hand.grab_state {
                HandGrabState::Empty => 0,
                HandGrabState::Grabbing(net_id) => net_id.0.get(),
            }
            .write_to(w)?;
        }
        if let Some(grabbable) = entity.get::<GrabbableComponent>() {
            5u8.write_to(w)?;
            grabbable.grabbed.write_to(w)?;
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
            .query::<(&SynchronizedComponent, Entity)>()
            .iter(world)
            .map(|(synchronized, entity)| (synchronized.net_id, entity))
            .collect::<HashMap<_, _>>();

        for _ in 0..count {
            // Get or create the referenced entity.
            let net_id = NetId::read_from(r)?;
            let authority = Authority::read_from(r)?;
            let mut entity = match entities_by_net_id.get(&net_id).copied() {
                Some(entity) => {
                    let mut entity = world.entity_mut(entity);
                    entity.get_mut::<SynchronizedComponent>().unwrap().authority = authority;
                    entity
                }
                None => {
                    let entity = world.spawn().id();
                    world
                        .resource_mut::<EntitiesByNetIdResource>()
                        .0
                        .insert(net_id, entity);

                    let mut entity = world.entity_mut(entity);
                    entity.insert(SynchronizedComponent { net_id, authority });
                    entity
                }
            };

            // Decode and update this entity's other components.
            let mut transform = None;
            let mut render = None;
            let mut physics = None;
            let mut flies_around = None;
            let mut hand = None;
            let mut grabbable = None;
            loop {
                match u8::read_from(r)? {
                    0 => break,
                    1 => {
                        let isometry = Isometry::<f32>::read_from(r)?;
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
                    4 => {
                        let index = u8::read_from(r)? as usize;
                        let grab_state = match NonZeroU32::new(u32::read_from(r)?) {
                            Some(net_id) => HandGrabState::Grabbing(NetId(net_id)),
                            None => HandGrabState::Empty,
                        };
                        hand = Some(HandComponent { index, grab_state });
                    }
                    5 => {
                        let grabbed = bool::read_from(r)?;
                        grabbable = Some(GrabbableComponent { grabbed });
                    }
                    0xff => {
                        flies_around = Some(FlyAroundComponent);
                    }
                    token => return Err(ReadSnapshotError::InvalidGameStateToken(token)),
                }
            }
            let mut ctx = NetComponentDestroyContext {
                physics: &mut physics_resource,
            };
            update_component(entity.borrow_mut(), transform, ctx.borrow_mut());
            update_component(entity.borrow_mut(), render, ctx.borrow_mut());
            update_component(entity.borrow_mut(), physics, ctx.borrow_mut());
            update_component(entity.borrow_mut(), hand, ctx.borrow_mut());
            update_component(entity.borrow_mut(), grabbable, ctx.borrow_mut());
            update_component(entity.borrow_mut(), flies_around, ctx.borrow_mut());
        }
        Ok(())
    })
}

fn update_component<T: NetComponent>(
    entity: &mut EntityMut,
    new_value: Option<T>,
    ctx: NetComponentDestroyContext,
) {
    match (entity.get_mut::<T>(), new_value) {
        (Some(mut component), Some(new_value)) => {
            component.apply_snapshot(new_value);
        }
        (Some(_), None) => {
            entity.remove::<T>().unwrap().destroy(ctx);
        }
        (None, Some(new_value)) => {
            entity.insert(new_value);
        }
        (None, None) => (),
    }
}
