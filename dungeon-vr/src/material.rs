use std::fs::read;

use anyhow::{bail, Context, Result};
use ash::vk;

use crate::asset::{Asset, Loader};
use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;

pub struct Material {
    pub image: vk::Image,
    pub image_memory: vk::DeviceMemory,
    pub image_view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub descriptor_set: vk::DescriptorSet,
}

impl Loader for Material {
    type Asset = Self;
    type ID = String;
    type BorrowedID = str;
    type Context = ();

    fn load_placeholder(_vk: &VkHandles, _render: &RenderData, _ctx: &mut ()) -> Option<Self> {
        None
    }

    fn load(vk: &VkHandles, render: &RenderData, _ctx: &mut (), id: &str) -> Result<Self> {
        log::info!("Loading material {id}");
        let data = read(id)?;
        let (image, image_memory, image_view) =
            load_image(vk, &data).with_context(|| format!("opening image {id:?}"))?;
        let sampler = unsafe {
            vk.device()
                .create_sampler(
                    &vk::SamplerCreateInfo::builder()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT)
                        .anisotropy_enable(false)
                        .border_color(vk::BorderColor::INT_OPAQUE_BLACK)
                        .unnormalized_coordinates(false)
                        .compare_enable(false)
                        .compare_op(vk::CompareOp::ALWAYS)
                        .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                        .mip_lod_bias(0.0)
                        .min_lod(0.0)
                        .max_lod(0.0),
                    None,
                )
                .unwrap()
        };
        let descriptor_set = unsafe {
            vk.device().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(render.descriptor_pool)
                    .set_layouts(&[render.textured_descriptor_set_layout]),
            )
        }
        .unwrap()[0];
        unsafe {
            vk.device().update_descriptor_sets(
                &[vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_set)
                    .dst_binding(0)
                    .dst_array_element(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&[vk::DescriptorImageInfo::builder()
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                        .image_view(image_view)
                        .sampler(sampler)
                        .build()])
                    .build()],
                &[],
            );
        }

        Ok(Self {
            image,
            image_memory,
            image_view,
            sampler,
            descriptor_set,
        })
    }
}

impl Asset for Material {
    unsafe fn destroy(self, vk: &VkHandles, render: &RenderData) {
        vk.device()
            .free_descriptor_sets(render.descriptor_pool, &[self.descriptor_set])
            .unwrap();
        vk.device().destroy_sampler(self.sampler, None);
        vk.device().destroy_image_view(self.image_view, None);
        vk.device().destroy_image(self.image, None);
        vk.device().free_memory(self.image_memory, None);
    }
}

fn load_image(vk: &VkHandles, data: &[u8]) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let mut reader = png::Decoder::new(data).read_info()?;
    let width = reader.info().width;
    let height = reader.info().height;
    let mut data = Vec::with_capacity(4 * width as usize * height as usize);
    match (reader.info().color_type, reader.info().bit_depth) {
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            while let Some(row) = reader.next_row()? {
                for rgb in row.data().chunks_exact(3) {
                    data.push(rgb[0]);
                    data.push(rgb[1]);
                    data.push(rgb[2]);
                    data.push(255);
                }
            }
        }
        (png::ColorType::Rgba, png::BitDepth::Eight) => {
            while let Some(row) = reader.next_row()? {
                data.extend(row.data());
            }
        }
        x => bail!("unsupported PNG color type: {x:?}"),
    }
    let (image, image_memory) = vk.stage_image(
        &data,
        width,
        height,
        vk::Format::R8G8B8A8_SRGB,
        vk::ImageUsageFlags::SAMPLED,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    );
    let image_view = vk.create_image_view(image, vk::Format::R8G8B8A8_SRGB);
    Ok((image, image_memory, image_view))
}
