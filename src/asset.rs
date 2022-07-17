use std::collections::HashMap;

use slotmap::{new_key_type, SlotMap};

use crate::mesh::Mesh;
use crate::vk_handles::VkHandles;

new_key_type! { pub struct MeshAssetKey; }

pub struct MeshAssets<'a> {
    vk: &'a VkHandles,
    meshes: SlotMap<MeshAssetKey, Mesh>,
    mesh_keys_by_name: HashMap<String, MeshAssetKey>,
    placeholder_key: MeshAssetKey,
}

impl<'a> MeshAssets<'a> {
    pub fn new(vk: &'a VkHandles) -> Self {
        let mut meshes = SlotMap::with_key();
        let mut mesh_keys_by_name = HashMap::new();

        let placeholder_key = meshes.insert(crate::mesh::create_debug_mesh(vk));
        mesh_keys_by_name.insert(String::new(), placeholder_key);

        Self {
            vk,
            meshes,
            mesh_keys_by_name,
            placeholder_key,
        }
    }

    pub fn load(&mut self, name: &str) -> MeshAssetKey {
        if let Some(mesh) = self.mesh_keys_by_name.get(name) {
            return *mesh;
        }

        let path = format!("assets/{name}.glb");
        let mesh_key = match Mesh::load(self.vk, &path) {
            Ok(mesh) => self.meshes.insert(mesh),
            Err(e) => {
                eprintln!("Error loading mesh {}: {}", path, e);
                self.placeholder_key
            }
        };
        self.mesh_keys_by_name.insert(name.to_string(), mesh_key);
        mesh_key
    }

    pub fn get(&self, key: MeshAssetKey) -> &Mesh {
        &self.meshes[key]
    }

    pub unsafe fn destroy(mut self) {
        for (_, mesh) in self.meshes.drain() {
            mesh.destroy(self.vk.device());
        }
    }
}
