use std::marker::PhantomData;
use std::mem::size_of;

use ash::vk;
use rapier3d::na::Matrix4;

use crate::vk_handles::VkHandles;
use crate::{textured, untextured, COLOR_FORMAT, DEPTH_FORMAT, RENDER_CONCURRENCY, VIEW_COUNT};

#[repr(C)]
struct ViewProjMatrixUbo {
    pub view_proj: [Matrix4<f32>; 2],
}

pub struct RenderData<'a> {
    phantom_lifetime: PhantomData<&'a VkHandles>,

    pub render_pass: vk::RenderPass,
    pub per_frame_descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_pool: vk::DescriptorPool,

    pub cmd_pool: vk::CommandPool,
    frame_resources: Vec<FrameResources>,

    pub untextured_pipeline_layout: vk::PipelineLayout,
    pub untextured_pipeline: vk::Pipeline,
    pub textured_descriptor_set_layout: vk::DescriptorSetLayout,
    pub textured_pipeline_layout: vk::PipelineLayout,
    pub textured_pipeline: vk::Pipeline,
}

impl<'a> RenderData<'a> {
    pub fn new(vk: &'a VkHandles) -> Self {
        let render_pass = create_render_pass(vk);
        let per_frame_descriptor_set_layout = create_per_frame_descriptor_set_layout(vk);
        let descriptor_pool = create_descriptor_pool(vk);

        let cmd_pool = create_cmd_pool(vk);
        let frame_resources = (0..RENDER_CONCURRENCY)
            .map(|_| {
                FrameResources::new(
                    vk,
                    cmd_pool,
                    descriptor_pool,
                    per_frame_descriptor_set_layout,
                )
            })
            .collect();

        let (untextured_pipeline_layout, untextured_pipeline) = unsafe {
            untextured::create_pipeline(vk, per_frame_descriptor_set_layout, render_pass)
        };
        let (textured_descriptor_set_layout, textured_pipeline_layout, textured_pipeline) =
            unsafe { textured::create_pipeline(vk, per_frame_descriptor_set_layout, render_pass) };

        Self {
            phantom_lifetime: PhantomData,

            render_pass,
            per_frame_descriptor_set_layout,
            descriptor_pool,

            cmd_pool,
            frame_resources,

            untextured_pipeline_layout,
            untextured_pipeline,
            textured_descriptor_set_layout,
            textured_pipeline_layout,
            textured_pipeline,
        }
    }

    pub fn frame_resources(&self, frame: usize) -> &FrameResources {
        &self.frame_resources[frame]
    }

    pub fn wait_for_fences(&self, vk: &VkHandles) {
        let fences = self
            .frame_resources
            .iter()
            .map(|x| x.fence)
            .collect::<Vec<_>>();
        unsafe { vk.device().wait_for_fences(&fences, true, !0) }.unwrap();
    }

    pub unsafe fn destroy(self, device: &ash::Device) {
        device.destroy_pipeline(self.untextured_pipeline, None);
        device.destroy_pipeline_layout(self.untextured_pipeline_layout, None);
        device.destroy_pipeline(self.textured_pipeline, None);
        device.destroy_pipeline_layout(self.textured_pipeline_layout, None);
        device.destroy_descriptor_set_layout(self.textured_descriptor_set_layout, None);
        for frame_resources in self.frame_resources {
            frame_resources.destroy(device);
        }
        device.destroy_command_pool(self.cmd_pool, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.per_frame_descriptor_set_layout, None);
        device.destroy_render_pass(self.render_pass, None);
    }
}

pub struct FrameResources {
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    per_frame_descriptor_set: vk::DescriptorSet,
    view_proj_matrix_buffer: vk::Buffer,
    view_proj_matrix_memory: vk::DeviceMemory,
}

impl FrameResources {
    fn new(
        vk: &VkHandles,
        cmd_pool: vk::CommandPool,
        descriptor_pool: vk::DescriptorPool,
        per_frame_descriptor_set_layout: vk::DescriptorSetLayout,
    ) -> Self {
        let cmd = unsafe {
            vk.device().allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(cmd_pool)
                    .command_buffer_count(1),
            )
        }
        .unwrap()[0];

        let fence = unsafe {
            vk.device().create_fence(
                &vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED),
                None,
            )
        }
        .unwrap();

        let per_frame_descriptor_set = unsafe {
            vk.device().allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&[per_frame_descriptor_set_layout]),
            )
        }
        .unwrap()[0];

        let view_proj_matrix_buffer = unsafe {
            vk.device().create_buffer(
                &vk::BufferCreateInfo::builder()
                    .size(size_of::<ViewProjMatrixUbo>() as u64)
                    .usage(vk::BufferUsageFlags::UNIFORM_BUFFER)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .unwrap();
        let memory_requirements = unsafe {
            vk.device()
                .get_buffer_memory_requirements(view_proj_matrix_buffer)
        };
        let view_proj_matrix_memory = unsafe {
            vk.device().allocate_memory(
                &vk::MemoryAllocateInfo::builder()
                    .allocation_size(memory_requirements.size)
                    .memory_type_index(vk.find_memory_type(
                        memory_requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )),
                None,
            )
        }
        .unwrap();
        unsafe {
            vk.device()
                .bind_buffer_memory(view_proj_matrix_buffer, view_proj_matrix_memory, 0)
        }
        .unwrap();

        unsafe {
            vk.device().update_descriptor_sets(
                &[vk::WriteDescriptorSet::builder()
                    .dst_set(per_frame_descriptor_set)
                    .dst_binding(0)
                    .dst_array_element(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&[vk::DescriptorBufferInfo::builder()
                        .buffer(view_proj_matrix_buffer)
                        .offset(0)
                        .range(size_of::<ViewProjMatrixUbo>() as u64)
                        .build()])
                    .build()],
                &[],
            )
        }

        Self {
            cmd,
            fence,
            per_frame_descriptor_set,
            view_proj_matrix_buffer,
            view_proj_matrix_memory,
        }
    }

    pub fn cmd(&self) -> vk::CommandBuffer {
        self.cmd
    }

    pub fn fence(&self) -> vk::Fence {
        self.fence
    }

    pub fn per_frame_descriptor_set(&self) -> vk::DescriptorSet {
        self.per_frame_descriptor_set
    }

    pub fn write_view_proj_matrix(
        &self,
        vk: &VkHandles,
        view_proj: [Matrix4<f32>; VIEW_COUNT as usize],
    ) {
        unsafe {
            let data = vk
                .device()
                .map_memory(
                    self.view_proj_matrix_memory,
                    0,
                    size_of::<ViewProjMatrixUbo>() as u64,
                    vk::MemoryMapFlags::empty(),
                )
                .unwrap();
            (data as *mut ViewProjMatrixUbo).write(ViewProjMatrixUbo { view_proj });
            vk.device().unmap_memory(self.view_proj_matrix_memory);
        }
    }

    unsafe fn destroy(self, device: &ash::Device) {
        device.destroy_buffer(self.view_proj_matrix_buffer, None);
        device.free_memory(self.view_proj_matrix_memory, None);
        device.destroy_fence(self.fence, None);
    }
}

#[derive(Clone, Copy)]
pub struct Ubo {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
}

fn create_render_pass(vk: &VkHandles) -> vk::RenderPass {
    let view_mask = !(!0 << VIEW_COUNT);
    unsafe {
        vk.device().create_render_pass(
            &vk::RenderPassCreateInfo::builder()
                .attachments(&[
                    vk::AttachmentDescription {
                        format: COLOR_FORMAT,
                        samples: vk::SampleCountFlags::TYPE_4,
                        load_op: vk::AttachmentLoadOp::CLEAR,
                        store_op: vk::AttachmentStoreOp::STORE,
                        initial_layout: vk::ImageLayout::UNDEFINED,
                        final_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                        ..Default::default()
                    },
                    vk::AttachmentDescription {
                        format: DEPTH_FORMAT,
                        samples: vk::SampleCountFlags::TYPE_4,
                        load_op: vk::AttachmentLoadOp::CLEAR,
                        store_op: vk::AttachmentStoreOp::DONT_CARE,
                        initial_layout: vk::ImageLayout::UNDEFINED,
                        final_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                        ..Default::default()
                    },
                ])
                .subpasses(&[vk::SubpassDescription::builder()
                    .color_attachments(&[vk::AttachmentReference {
                        attachment: 0,
                        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    }])
                    .depth_stencil_attachment(&vk::AttachmentReference {
                        attachment: 1,
                        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                    })
                    .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                    .build()])
                .dependencies(&[vk::SubpassDependency {
                    src_subpass: vk::SUBPASS_EXTERNAL,
                    dst_subpass: 0,
                    src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                    dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                        | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                    dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                        | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                    ..Default::default()
                }])
                .push_next(
                    &mut vk::RenderPassMultiviewCreateInfo::builder()
                        .view_masks(&[view_mask])
                        .correlation_masks(&[view_mask]),
                ),
            None,
        )
    }
    .unwrap()
}

fn create_per_frame_descriptor_set_layout(vk: &VkHandles) -> vk::DescriptorSetLayout {
    unsafe {
        vk.device().create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::builder().bindings(&[
                vk::DescriptorSetLayoutBinding::builder()
                    .binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::VERTEX)
                    .build(),
                vk::DescriptorSetLayoutBinding::builder()
                    .binding(1)
                    .descriptor_count(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                    .build(),
            ]),
            None,
        )
    }
    .unwrap()
}

fn create_descriptor_pool(vk: &VkHandles) -> vk::DescriptorPool {
    unsafe {
        vk.device().create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::builder()
                .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
                .pool_sizes(&[
                    vk::DescriptorPoolSize::builder()
                        .ty(vk::DescriptorType::UNIFORM_BUFFER)
                        .descriptor_count(RENDER_CONCURRENCY)
                        .build(),
                    // TODO: This is a scaling limitation for the number of concurrently loaded
                    // materials.
                    vk::DescriptorPoolSize::builder()
                        .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(100)
                        .build(),
                ])
                .max_sets(RENDER_CONCURRENCY + 100),
            None,
        )
    }
    .unwrap()
}

fn create_cmd_pool(vk: &VkHandles) -> vk::CommandPool {
    unsafe {
        vk.device().create_command_pool(
            &vk::CommandPoolCreateInfo::builder()
                .queue_family_index(vk.queue_family_index())
                .flags(
                    vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER
                        | vk::CommandPoolCreateFlags::TRANSIENT,
                ),
            None,
        )
    }
    .unwrap()
}
