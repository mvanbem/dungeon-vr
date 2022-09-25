use bevy_ecs::prelude::*;

use crate::asset::ModelAssetKey;

#[derive(Component)]
pub struct ModelRenderer {
    pub model_key: ModelAssetKey,
}
