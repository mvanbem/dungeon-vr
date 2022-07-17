use anyhow::{bail, Result};
use ash::vk;
use bytemuck::{Pod, Zeroable};
use gltf::mesh::util::{ReadIndices, ReadTexCoords};

use crate::vk_handles::VkHandles;
use crate::VertexFormat;

#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct TexturedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
}

pub struct Mesh {
    pub vertex_format: VertexFormat,
    pub index_type: vk::IndexType,
    pub mode: vk::PrimitiveTopology,
    pub count: usize,
    pub vertex_buffer: vk::Buffer,
    pub vertex_memory: vk::DeviceMemory,
    pub index_buffer: vk::Buffer,
    pub index_memory: vk::DeviceMemory,
}

impl Mesh {
    pub fn load(vk: &VkHandles, path: &str) -> Result<Self> {
        let (document, buffers, _) = gltf::import(path)?;

        assert_eq!(document.meshes().len(), 1);
        let mesh = document.meshes().next().unwrap();

        assert_eq!(mesh.primitives().len(), 1);
        let primitive = mesh.primitives().next().unwrap();

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

        Ok(Self {
            vertex_format: VertexFormat::Textured,
            index_type: vk::IndexType::UINT16,
            mode,
            count: index_data.len(),
            vertex_buffer,
            vertex_memory,
            index_buffer,
            index_memory,
        })
    }

    pub unsafe fn destroy(self, vk_device: &ash::Device) {
        vk_device.destroy_buffer(self.vertex_buffer, None);
        vk_device.free_memory(self.vertex_memory, None);
        vk_device.destroy_buffer(self.index_buffer, None);
        vk_device.free_memory(self.index_memory, None);
    }
}

#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct FlatColorVertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}
static VERTEX_DATA: [FlatColorVertex; 4] = [
    FlatColorVertex {
        position: [-0.1, -0.1, -0.1],
        color: [0.5, 0.5, 0.5],
    },
    FlatColorVertex {
        position: [0.1, -0.1, -0.1],
        color: [1.0, 0.0, 0.0],
    },
    FlatColorVertex {
        position: [-0.1, 0.1, -0.1],
        color: [0.0, 1.0, 0.0],
    },
    FlatColorVertex {
        position: [-0.1, -0.1, 0.1],
        color: [0.0, 0.0, 1.0],
    },
];
static INDEX_DATA: [u16; 9] = [0, 1, 2, 0, 2, 3, 0, 3, 1];

pub fn create_debug_mesh(vk: &VkHandles) -> Mesh {
    let (vertex_buffer, vertex_memory) = vk.create_initialized_buffer(
        bytemuck::cast_slice(&VERTEX_DATA),
        vk::BufferUsageFlags::VERTEX_BUFFER,
    );

    let (index_buffer, index_memory) = vk.create_initialized_buffer(
        bytemuck::cast_slice(&INDEX_DATA),
        vk::BufferUsageFlags::INDEX_BUFFER,
    );

    Mesh {
        vertex_format: VertexFormat::FlatColor,
        index_type: vk::IndexType::UINT16,
        mode: vk::PrimitiveTopology::TRIANGLE_LIST,
        count: INDEX_DATA.len(),
        vertex_buffer,
        vertex_memory,
        index_buffer,
        index_memory,
    }
}
