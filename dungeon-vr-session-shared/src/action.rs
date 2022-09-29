use std::convert::Infallible;

use bevy_ecs::prelude::*;
use dungeon_vr_stream_codec::{ReadError, StreamCodec};
use thiserror::Error;

use crate::core::{Authority, NetId, ReadNetIdError};
use crate::interaction::{GrabbableComponent, HandComponent, HandGrabState};
use crate::resources::{AllActions, EntitiesByNetId};
use crate::PlayerId;

/// Things players can do.
#[derive(Clone, Copy, Debug)]
pub enum Action {
    Grab { hand_index: usize, target: NetId },
    Drop { hand_index: usize },
}

#[derive(Error, Debug)]
pub enum ReadActionError {
    #[error("{0}")]
    ReadError(#[from] ReadError),

    #[error("{0}")]
    ReadNetIdError(#[from] ReadNetIdError),

    #[error("invalid type: 0x{0:02x}")]
    InvalidType(u8),
}

impl StreamCodec for Action {
    type ReadError = ReadActionError;
    type WriteError = Infallible;

    fn read_from(r: &mut &[u8]) -> Result<Self, ReadActionError> {
        match u8::read_from(r)? {
            0 => {
                let hand_index = u8::read_from(r)? as usize;
                let target = NetId::read_from(r)?;
                Ok(Self::Grab { hand_index, target })
            }
            1 => {
                let hand_index = u8::read_from(r)? as usize;
                Ok(Self::Drop { hand_index })
            }
            x => Err(ReadActionError::InvalidType(x)),
        }
    }

    fn write_to(&self, w: &mut Vec<u8>) -> Result<(), Infallible> {
        match *self {
            Self::Grab { hand_index, target } => {
                0u8.write_to(w)?;
                (hand_index as u8).write_to(w)?;
                target.write_to(w)?;
            }
            Self::Drop { hand_index } => {
                1u8.write_to(w)?;
                (hand_index as u8).write_to(w)?;
            }
        }
        Ok(())
    }
}

pub fn apply_actions(
    actions: Res<AllActions>,
    mut hand_query: Query<(&Authority, &mut HandComponent), Without<GrabbableComponent>>,
    mut grabbable_query: Query<(&mut Authority, &mut GrabbableComponent), Without<HandComponent>>,
    entities_by_net_id: Res<EntitiesByNetId>,
) {
    for (&player_id, actions) in &actions.0 {
        for action in actions.iter().copied() {
            match apply_action(
                player_id,
                action,
                &mut hand_query,
                &mut grabbable_query,
                &*entities_by_net_id,
            ) {
                Ok(()) => (),
                Err(e) => {
                    log::warn!("Failed to apply action {action:?}: {e}");
                }
            }
        }
    }
}

#[derive(Debug, Error)]
enum ApplyActionError {
    #[error("hand not found")]
    HandNotFound,

    #[error("hand must be empty but was {0:?}")]
    GrabBadHandGrabState(HandGrabState),

    #[error("target not found")]
    GrabTargetNotFound,

    #[error("target authority must be server or local but was {0:?}")]
    GrabBadTargetAuthority(Authority),

    #[error("target must be ungrabbed")]
    GrabBadTargetGrabbed,

    #[error("hand must be grabbing but was empty")]
    DropBadHandGrabState,
}

fn apply_action(
    player_id: PlayerId,
    action: Action,
    hand_query: &mut Query<(&Authority, &mut HandComponent), Without<GrabbableComponent>>,
    grabbable_query: &mut Query<(&mut Authority, &mut GrabbableComponent), Without<HandComponent>>,
    entities_by_net_id: &EntitiesByNetId,
) -> Result<(), ApplyActionError> {
    match action {
        Action::Grab { hand_index, target } => {
            let (_, mut hand) = hand_query
                .iter_mut()
                .filter(|(&hand_authority, hand)| {
                    hand_authority == Authority::Client(player_id) && hand.index == hand_index
                })
                .next()
                .ok_or_else(|| ApplyActionError::HandNotFound)?;
            if !matches!(hand.grab_state, HandGrabState::Empty) {
                return Err(ApplyActionError::GrabBadHandGrabState(hand.grab_state));
            }

            let target_entity = entities_by_net_id
                .0
                .get(&target)
                .copied()
                .ok_or_else(|| ApplyActionError::GrabTargetNotFound)?;
            let (mut grabbable_authority, mut grabbable) =
                grabbable_query
                    .get_mut(target_entity)
                    .map_err(|_| ApplyActionError::GrabTargetNotFound)?;
            if let Authority::Client(grabbable_player_id) = *grabbable_authority {
                if grabbable_player_id != player_id {
                    return Err(ApplyActionError::GrabBadTargetAuthority(
                        *grabbable_authority,
                    ));
                }
            }
            if grabbable.grabbed {
                return Err(ApplyActionError::GrabBadTargetGrabbed);
            }

            hand.grab_state = HandGrabState::Grabbing(target);
            *grabbable_authority = Authority::Client(player_id);
            grabbable.grabbed = true;
            Ok(())
        }
        Action::Drop { hand_index } => {
            let (_, mut hand) = hand_query
                .iter_mut()
                .filter(|(&hand_authority, hand)| {
                    hand_authority == Authority::Client(player_id) && hand.index == hand_index
                })
                .next()
                .ok_or_else(|| ApplyActionError::HandNotFound)?;
            let target = hand
                .grab_state
                .grab_target()
                .ok_or_else(|| ApplyActionError::DropBadHandGrabState)?;

            let target_entity = entities_by_net_id.0.get(&target).copied().unwrap();
            let (mut grabbable_authority, mut grabbable) =
                grabbable_query.get_mut(target_entity).unwrap();
            assert_eq!(*grabbable_authority, Authority::Client(player_id));
            assert!(grabbable.grabbed);

            hand.grab_state = HandGrabState::Empty;
            *grabbable_authority = Authority::Server;
            grabbable.grabbed = false;
            Ok(())
        }
    }
}
