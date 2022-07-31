use std::io::Cursor;
use std::mem::size_of;

use ash::util::read_spv;
use ash::vk;
use memoffset::offset_of;

use crate::model::TexturedVertex;
use crate::vk_handles::VkHandles;
use crate::{PushConstants, NOOP_STENCIL_STATE};

const VERT_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders/untextured.vert.spv"));
const FRAG_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/shaders/untextured.frag.spv"));

pub unsafe fn create_pipeline(
    vk: &VkHandles,
    per_frame_descriptor_set_layout: vk::DescriptorSetLayout,
    render_pass: vk::RenderPass,
) -> (vk::PipelineLayout, vk::Pipeline) {
    let pipeline_layout = vk
        .device()
        .create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::builder()
                .set_layouts(&[per_frame_descriptor_set_layout])
                .push_constant_ranges(&[vk::PushConstantRange {
                    stage_flags: vk::ShaderStageFlags::VERTEX,
                    offset: 0,
                    size: size_of::<PushConstants>() as u32,
                }]),
            None,
        )
        .unwrap();

    let vert = read_spv(&mut Cursor::new(VERT_SPV)).unwrap();
    let frag = read_spv(&mut Cursor::new(FRAG_SPV)).unwrap();
    let vert = vk
        .device()
        .create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&vert), None)
        .unwrap();
    let frag = vk
        .device()
        .create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&frag), None)
        .unwrap();

    let pipeline = vk
        .device()
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &[vk::GraphicsPipelineCreateInfo::builder()
                .stages(&[
                    vk::PipelineShaderStageCreateInfo {
                        stage: vk::ShaderStageFlags::VERTEX,
                        module: vert,
                        p_name: b"main\0".as_ptr() as _,
                        ..Default::default()
                    },
                    vk::PipelineShaderStageCreateInfo {
                        stage: vk::ShaderStageFlags::FRAGMENT,
                        module: frag,
                        p_name: b"main\0".as_ptr() as _,
                        ..Default::default()
                    },
                ])
                .vertex_input_state(
                    &vk::PipelineVertexInputStateCreateInfo::builder()
                        .vertex_binding_descriptions(&[vk::VertexInputBindingDescription::builder(
                        )
                        .binding(0)
                        .stride(size_of::<TexturedVertex>() as u32)
                        .input_rate(vk::VertexInputRate::VERTEX)
                        .build()])
                        .vertex_attribute_descriptions(&[
                            vk::VertexInputAttributeDescription::builder()
                                .location(0)
                                .binding(0)
                                .format(vk::Format::R32G32B32_SFLOAT)
                                .offset(offset_of!(TexturedVertex, position) as u32)
                                .build(),
                            vk::VertexInputAttributeDescription::builder()
                                .location(1)
                                .binding(0)
                                .format(vk::Format::R32G32B32_SFLOAT)
                                .offset(offset_of!(TexturedVertex, normal) as u32)
                                .build(),
                            vk::VertexInputAttributeDescription::builder()
                                .location(2)
                                .binding(0)
                                .format(vk::Format::R32G32_SFLOAT)
                                .offset(offset_of!(TexturedVertex, texcoord) as u32)
                                .build(),
                        ]),
                )
                .input_assembly_state(
                    &vk::PipelineInputAssemblyStateCreateInfo::builder()
                        .topology(vk::PrimitiveTopology::TRIANGLE_LIST),
                )
                .viewport_state(
                    &vk::PipelineViewportStateCreateInfo::builder()
                        .scissor_count(1)
                        .viewport_count(1),
                )
                .rasterization_state(
                    &vk::PipelineRasterizationStateCreateInfo::builder()
                        .cull_mode(vk::CullModeFlags::NONE)
                        .polygon_mode(vk::PolygonMode::FILL)
                        .line_width(1.0),
                )
                .multisample_state(
                    &vk::PipelineMultisampleStateCreateInfo::builder()
                        .rasterization_samples(vk::SampleCountFlags::TYPE_4),
                )
                .depth_stencil_state(
                    &vk::PipelineDepthStencilStateCreateInfo::builder()
                        .depth_test_enable(true)
                        .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL)
                        .depth_write_enable(true)
                        .front(NOOP_STENCIL_STATE)
                        .back(NOOP_STENCIL_STATE),
                )
                .color_blend_state(
                    &vk::PipelineColorBlendStateCreateInfo::builder().attachments(&[
                        vk::PipelineColorBlendAttachmentState {
                            blend_enable: vk::TRUE,
                            src_color_blend_factor: vk::BlendFactor::ONE,
                            dst_color_blend_factor: vk::BlendFactor::ZERO,
                            color_blend_op: vk::BlendOp::ADD,
                            color_write_mask: vk::ColorComponentFlags::R
                                | vk::ColorComponentFlags::G
                                | vk::ColorComponentFlags::B,
                            ..Default::default()
                        },
                    ]),
                )
                .dynamic_state(
                    &vk::PipelineDynamicStateCreateInfo::builder()
                        .dynamic_states(&[vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR]),
                )
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0)
                .build()],
            None,
        )
        .map_err(|(_, result)| result)
        .unwrap()[0];

    vk.device().destroy_shader_module(vert, None);
    vk.device().destroy_shader_module(frag, None);

    (pipeline_layout, pipeline)
}
