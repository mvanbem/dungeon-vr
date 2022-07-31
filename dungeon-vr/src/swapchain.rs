use std::marker::PhantomData;

use ash::vk::{self, DebugUtilsObjectNameInfoEXT, Handle};
use cstr::cstr;
use openxr as xr;

use crate::render_data::RenderData;
use crate::vk_handles::VkHandles;
use crate::xr_handles::XrHandles;
use crate::xr_session::XrSession;
use crate::{COLOR_FORMAT, DEPTH_FORMAT, VIEW_COUNT, VIEW_TYPE};

pub struct Swapchain<'a> {
    phantom_lifetime: PhantomData<&'a VkHandles>,
    handle: xr::Swapchain<xr::Vulkan>,
    buffers: Vec<Framebuffer>,
    dimensions: vk::Extent2D,
    color_image: vk::Image,
    color_memory: vk::DeviceMemory,
    depth_image: vk::Image,
    depth_memory: vk::DeviceMemory,
}

impl<'a> Swapchain<'a> {
    pub fn new(xr: &XrHandles, vk: &'a VkHandles, xrs: &XrSession, render: &RenderData) -> Self {
        let views = xr
            .instance
            .enumerate_view_configuration_views(xr.system, VIEW_TYPE)
            .unwrap();
        assert_eq!(views.len(), VIEW_COUNT as usize);

        // Dimensions must match for multiview rendering.
        assert_eq!(views[0], views[1]);
        let dimensions = vk::Extent2D {
            width: views[0].recommended_image_rect_width,
            height: views[0].recommended_image_rect_height,
        };

        let handle = xrs
            .session
            .create_swapchain(&xr::SwapchainCreateInfo {
                create_flags: xr::SwapchainCreateFlags::EMPTY,
                usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                    | xr::SwapchainUsageFlags::SAMPLED,
                format: COLOR_FORMAT.as_raw() as _,
                sample_count: 1,
                width: dimensions.width,
                height: dimensions.height,
                face_count: 1,
                array_size: VIEW_COUNT,
                mip_count: 1,
            })
            .unwrap();

        let color_image = unsafe {
            vk.device().create_image(
                &vk::ImageCreateInfo::builder()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(COLOR_FORMAT)
                    .extent(vk::Extent3D {
                        width: dimensions.width,
                        height: dimensions.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(VIEW_COUNT)
                    .samples(vk::SampleCountFlags::TYPE_4)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .queue_family_indices(&[vk.queue_family_index()])
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
        }
        .unwrap();
        unsafe {
            vk.debug_utils()
                .debug_utils_set_object_name(
                    vk.device().handle(),
                    &DebugUtilsObjectNameInfoEXT::builder()
                        .object_type(vk::ObjectType::IMAGE)
                        .object_handle(color_image.as_raw())
                        .object_name(cstr!(b"color buffer image")),
                )
                .unwrap()
        }
        let memory_requirements = unsafe { vk.device().get_image_memory_requirements(color_image) };
        let color_memory = unsafe {
            vk.device().allocate_memory(
                &vk::MemoryAllocateInfo::builder()
                    .allocation_size(memory_requirements.size)
                    .memory_type_index(vk.find_memory_type(
                        memory_requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )),
                None,
            )
        }
        .unwrap();
        unsafe {
            vk.debug_utils().debug_utils_set_object_name(
                vk.device().handle(),
                &DebugUtilsObjectNameInfoEXT::builder()
                    .object_type(vk::ObjectType::DEVICE_MEMORY)
                    .object_handle(color_memory.as_raw())
                    .object_name(cstr!(b"color buffer memory")),
            )
        }
        .unwrap();
        unsafe { vk.device().bind_image_memory(color_image, color_memory, 0) }.unwrap();

        let depth_image = unsafe {
            vk.device().create_image(
                &vk::ImageCreateInfo::builder()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(DEPTH_FORMAT)
                    .extent(vk::Extent3D {
                        width: dimensions.width,
                        height: dimensions.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(VIEW_COUNT)
                    .samples(vk::SampleCountFlags::TYPE_4)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .queue_family_indices(&[vk.queue_family_index()])
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
        }
        .unwrap();
        unsafe {
            vk.debug_utils().debug_utils_set_object_name(
                vk.device().handle(),
                &DebugUtilsObjectNameInfoEXT::builder()
                    .object_type(vk::ObjectType::IMAGE)
                    .object_handle(depth_image.as_raw())
                    .object_name(cstr!(b"depth buffer image")),
            )
        }
        .unwrap();
        let memory_requirements = unsafe { vk.device().get_image_memory_requirements(depth_image) };
        let depth_memory = unsafe {
            vk.device().allocate_memory(
                &vk::MemoryAllocateInfo::builder()
                    .allocation_size(memory_requirements.size)
                    .memory_type_index(vk.find_memory_type(
                        memory_requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::DEVICE_LOCAL,
                    )),
                None,
            )
        }
        .unwrap();
        unsafe {
            vk.debug_utils().debug_utils_set_object_name(
                vk.device().handle(),
                &DebugUtilsObjectNameInfoEXT::builder()
                    .object_type(vk::ObjectType::DEVICE_MEMORY)
                    .object_handle(depth_memory.as_raw())
                    .object_name(cstr!(b"depth buffer memory")),
            )
        }
        .unwrap();
        unsafe { vk.device().bind_image_memory(depth_image, depth_memory, 0) }.unwrap();

        let mut buffers = Vec::new();
        for swapchain_color_image in handle.enumerate_images().unwrap() {
            // Attachment 0: the multisample color buffer for rendering.
            let color_image_view = unsafe {
                vk.device().create_image_view(
                    &vk::ImageViewCreateInfo::builder()
                        .image(color_image)
                        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                        .format(COLOR_FORMAT)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: VIEW_COUNT,
                        }),
                    None,
                )
            }
            .unwrap();
            unsafe {
                vk.debug_utils().debug_utils_set_object_name(
                    vk.device().handle(),
                    &DebugUtilsObjectNameInfoEXT::builder()
                        .object_type(vk::ObjectType::IMAGE_VIEW)
                        .object_handle(color_image_view.as_raw())
                        .object_name(if buffers.is_empty() {
                            cstr!(b"left color buffer view")
                        } else {
                            cstr!(b"right color buffer view")
                        }),
                )
            }
            .unwrap();

            // Attachment 1: the multisample depth buffer for rendering.
            let depth_image_view = unsafe {
                vk.device().create_image_view(
                    &vk::ImageViewCreateInfo::builder()
                        .image(depth_image)
                        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
                        .format(DEPTH_FORMAT)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::DEPTH,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: VIEW_COUNT,
                        }),
                    None,
                )
            }
            .unwrap();
            unsafe {
                vk.debug_utils().debug_utils_set_object_name(
                    vk.device().handle(),
                    &DebugUtilsObjectNameInfoEXT::builder()
                        .object_type(vk::ObjectType::IMAGE_VIEW)
                        .object_handle(depth_image_view.as_raw())
                        .object_name(if buffers.is_empty() {
                            cstr!(b"left depth buffer view")
                        } else {
                            cstr!(b"right depth buffer view")
                        }),
                )
            }
            .unwrap();

            // Not attached: the 1-sample color image from the swapchain.
            let swapchain_color_image = vk::Image::from_raw(swapchain_color_image);

            let framebuffer = unsafe {
                vk.device().create_framebuffer(
                    &vk::FramebufferCreateInfo::builder()
                        .render_pass(render.render_pass)
                        .width(dimensions.width)
                        .height(dimensions.height)
                        .attachments(&[color_image_view, depth_image_view])
                        .layers(1), // Multiview handles addressing multiple layers
                    None,
                )
            }
            .unwrap();
            unsafe {
                vk.debug_utils().debug_utils_set_object_name(
                    vk.device().handle(),
                    &DebugUtilsObjectNameInfoEXT::builder()
                        .object_type(vk::ObjectType::FRAMEBUFFER)
                        .object_handle(framebuffer.as_raw())
                        .object_name(if buffers.is_empty() {
                            cstr!(b"left framebuffer")
                        } else {
                            cstr!(b"right framebuffer")
                        }),
                )
            }
            .unwrap();
            buffers.push(Framebuffer {
                framebuffer,
                color_image_view,
                depth_image_view,
                swapchain_color_image,
            });
        }

        Self {
            phantom_lifetime: PhantomData,
            handle,
            dimensions,
            color_image,
            color_memory,
            depth_image,
            depth_memory,
            buffers,
        }
    }

    pub fn handle(&self) -> &xr::Swapchain<xr::Vulkan> {
        &self.handle
    }

    pub fn handle_mut(&mut self) -> &mut xr::Swapchain<xr::Vulkan> {
        &mut self.handle
    }

    pub fn buffers(&self) -> &[Framebuffer] {
        &self.buffers
    }

    pub fn dimensions(&self) -> vk::Extent2D {
        self.dimensions
    }

    pub fn color_image(&self) -> vk::Image {
        self.color_image
    }

    pub unsafe fn destroy(self, device: &ash::Device) {
        for buffer in self.buffers {
            buffer.destroy(device);
        }
        device.destroy_image(self.color_image, None);
        device.free_memory(self.color_memory, None);
        device.destroy_image(self.depth_image, None);
        device.free_memory(self.depth_memory, None);
    }
}

pub struct Framebuffer {
    framebuffer: vk::Framebuffer,
    color_image_view: vk::ImageView,
    depth_image_view: vk::ImageView,
    swapchain_color_image: vk::Image,
}

impl Framebuffer {
    pub fn framebuffer(&self) -> vk::Framebuffer {
        self.framebuffer
    }

    pub fn swapchain_color_image(&self) -> vk::Image {
        self.swapchain_color_image
    }

    unsafe fn destroy(self, device: &ash::Device) {
        device.destroy_framebuffer(self.framebuffer, None);
        device.destroy_image_view(self.color_image_view, None);
        device.destroy_image_view(self.depth_image_view, None);
    }
}
