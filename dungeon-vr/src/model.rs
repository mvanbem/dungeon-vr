use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use gltf::mesh::util::{ReadIndices, ReadTexCoords};
use slotmap::Key;

use crate::asset::{Asset, Loader, MaterialAssetKey, MaterialAssets};
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct TexturedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
}

pub struct Model {
    pub primitives: Vec<Primitive>,
}

pub struct Primitive {
    pub index_type: vk::IndexType,
    pub mode: vk::PrimitiveTopology,
    pub count: usize,
    pub vertex_buffer: vk::Buffer,
    pub vertex_memory: vk::DeviceMemory,
    pub index_buffer: vk::Buffer,
    pub index_memory: vk::DeviceMemory,
    pub material: MaterialAssetKey,
}

impl Loader for Model {
    type Asset = Self;
    type ID = String;
    type BorrowedID = str;
    type Context = MaterialAssets;

    fn load_placeholder(
        vk: &VkHandles,
        render: &RenderData,
        ctx: &mut MaterialAssets,
    ) -> Option<Self> {
        Some(Self::load(vk, render, ctx, "error").unwrap())
    }

    fn load(
        vk: &VkHandles,
        render: &RenderData,
        ctx: &mut MaterialAssets,
        id: &str,
    ) -> Result<Self> {
        let path = format!("assets/{id}.gltf");
        let (document, buffers, _) = gltf::import(&path)?;

        assert_eq!(document.meshes().len(), 1);
        let mesh = document.meshes().next().unwrap();

        let mut primitives = Vec::new();
        for primitive in mesh.primitives() {
            let mode = match primitive.mode() {
                gltf::mesh::Mode::Triangles => vk::PrimitiveTopology::TRIANGLE_LIST,
                x => bail!("unsupported GLTF primitive mode: {:?}", x),
            };

            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let mut vertex_data = Vec::new();
            let positions = reader.read_positions().unwrap();
            let normals = reader.read_normals().unwrap();
            let texcoords = match reader.read_tex_coords(0).unwrap() {
                ReadTexCoords::F32(reader) => reader,
                x => bail!("unsupported texcoords: {:?}", x),
            };
            for ((position, normal), texcoord) in positions.zip(normals).zip(texcoords) {
                vertex_data.push(TexturedVertex {
                    position,
                    normal,
                    texcoord,
                });
            }

            let (vertex_buffer, vertex_memory) = vk.create_initialized_buffer(
                bytemuck::cast_slice(&vertex_data),
                vk::BufferUsageFlags::VERTEX_BUFFER,
            );

            let mut index_data = Vec::new();
            match reader.read_indices().unwrap() {
                ReadIndices::U16(reader) => {
                    for index in reader {
                        index_data.push(index);
                    }
                }
                x => bail!("unsupported index format: {:?}", x),
            }

            let (index_buffer, index_memory) = vk.create_initialized_buffer(
                bytemuck::cast_slice(&index_data),
                vk::BufferUsageFlags::INDEX_BUFFER,
            );

            let material = match primitive
                .material()
                .pbr_metallic_roughness()
                .base_color_texture()
            {
                Some(info) => match info.texture().source().source() {
                    gltf::image::Source::View { .. } => {
                        bail!("Images from views are not supported")
                    }
                    gltf::image::Source::Uri { uri, .. } => {
                        let mut path = match Path::new(&path).parent() {
                            Some(dir) => PathBuf::from(dir),
                            None => PathBuf::new(),
                        };
                        path.push(uri);
                        ctx.load(vk, render, &mut (), path.to_str().unwrap())
                    }
                },
                None => MaterialAssetKey::null(),
            };

            primitives.push(Primitive {
                index_type: vk::IndexType::UINT16,
                mode,
                count: index_data.len(),
                vertex_buffer,
                vertex_memory,
                index_buffer,
                index_memory,
                material,
            });
        }

        Ok(Self { primitives })
    }
}

impl Asset for Model {
    unsafe fn destroy(self, vk: &VkHandles, _render_data: &RenderData) {
        for primitive in self.primitives {
            primitive.destroy(vk.device());
        }
    }
}

impl Primitive {
    pub unsafe fn destroy(self, device: &ash::Device) {
        device.destroy_buffer(self.vertex_buffer, None);
        device.free_memory(self.vertex_memory, None);
        device.destroy_buffer(self.index_buffer, None);
        device.free_memory(self.index_memory, None);
    }
}
