use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

use anyhow::Result;
use dungeon_vr_session_shared::render::ModelHandle;
use slotmap::{new_key_type, SlotMap};

use crate::material::Material;
use crate::model::Model;
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

new_key_type! { pub struct MaterialHandle; }
pub type MaterialAssets = Assets<Material, MaterialHandle>;

pub type ModelAssets = Assets<Model, ModelHandle>;

pub trait Asset {
    unsafe fn destroy(self, vk: &VkHandles, render: &RenderData);
}

pub trait Loader {
    type Asset: Asset;
    type ID: Borrow<Self::BorrowedID>;
    type BorrowedID: ?Sized + ToOwned<Owned = Self::ID>;
    type Context;

    fn load_placeholder(
        vk: &VkHandles,
        render: &RenderData,
        ctx: &mut Self::Context,
    ) -> Option<Self::Asset>;

    fn load(
        vk: &VkHandles,
        render: &RenderData,
        ctx: &mut Self::Context,
        id: &Self::BorrowedID,
    ) -> Result<Self::Asset>;
}

pub struct Assets<L: Loader, K: slotmap::Key> {
    assets: SlotMap<K, L::Asset>,
    keys_by_id: HashMap<L::ID, K>,
    placeholder_key: K,
}

impl<L: Loader, K: slotmap::Key> Assets<L, K> {
    pub fn new(vk: &VkHandles, render: &RenderData, ctx: &mut L::Context) -> Self {
        let mut assets = SlotMap::with_key();
        let placeholder_key = match L::load_placeholder(vk, render, ctx) {
            Some(asset) => assets.insert(asset),
            None => K::null(),
        };

        Self {
            assets,
            keys_by_id: HashMap::new(),
            placeholder_key,
        }
    }

    pub fn load(
        &mut self,
        vk: &VkHandles,
        render: &RenderData,
        ctx: &mut L::Context,
        id: &L::BorrowedID,
    ) -> K
    where
        L::ID: Eq + Hash,
        L::BorrowedID: Debug + Eq + Hash,
    {
        if let Some(asset) = self.keys_by_id.get(id) {
            return *asset;
        }

        let key = match L::load(vk, render, ctx, id) {
            Ok(asset) => self.assets.insert(asset),
            Err(e) => {
                if self.placeholder_key.is_null() {
                    panic!("Error loading asset {:?}: {}", id, e);
                } else {
                    eprintln!("Error loading asset {:?}: {}", id, e);
                    self.placeholder_key
                }
            }
        };
        self.keys_by_id.insert(id.to_owned(), key);
        key
    }

    pub fn get(&self, key: K) -> &L::Asset {
        &self.assets[key]
    }

    pub unsafe fn destroy(mut self, vk: &VkHandles, render: &RenderData) {
        for (_, asset) in self.assets.drain() {
            asset.destroy(vk, render);
        }
    }
}
