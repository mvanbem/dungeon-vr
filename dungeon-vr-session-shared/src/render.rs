use bevy_ecs::prelude::*;
use slotmap::{new_key_type, Key};

new_key_type! { pub struct ModelHandle; }

/// Component for rendering a model with an entity's transform.
#[derive(Component)]
pub struct RenderComponent {
    /// The name of the model to render. Synchronized from server to client.
    pub model_name: String,

    /// Either null or a handle to the loaded model. A client system keeps this up to date with the
    /// name.
    pub model_handle: ModelHandle,
}

impl RenderComponent {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            model_name: name.into(),
            model_handle: ModelHandle::null(),
        }
    }
}
