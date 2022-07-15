use std::fmt::{self, Display, Formatter};
use std::intrinsics::copy_nonoverlapping;

use ash::extensions::ext::DebugUtils;
use ash::vk::{self, Handle};
use openxr as xr;

use crate::xr_handles::XrHandles;

const TARGET_VK_MAJOR_VERSION: u16 = 1;
const TARGET_VK_MINOR_VERSION: u16 = 1;
const TARGET_API_VERSION: VkVersion = VkVersion(vk::make_api_version(
    0,
    TARGET_VK_MAJOR_VERSION as u32,
    TARGET_VK_MINOR_VERSION as u32,
    0,
));
const ENABLE_VK_VALIDATION_LAYER: bool = false;

struct VkVersion(u32);

impl Display for VkVersion {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}",
            vk::api_version_major(self.0),
            vk::api_version_minor(self.0),
            vk::api_version_patch(self.0),
        )
    }
}

unsafe extern "system" fn debug_utils_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _p_user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let severity = match message_severity {
        vk::DebugUtilsMessageSeverityFlagsEXT::VERBOSE => "[Verbose]",
        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING => "[Warning]",
        vk::DebugUtilsMessageSeverityFlagsEXT::ERROR => "[Error]",
        vk::DebugUtilsMessageSeverityFlagsEXT::INFO => "[Info]",
        _ => "[Unknown]",
    };
    let types = match message_type {
        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL => "[General]",
        vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE => "[Performance]",
        vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION => "[Validation]",
        _ => "[Unknown]",
    };
    let message = std::ffi::CStr::from_ptr((*p_callback_data).p_message);
    println!("[Debug]{}{}{:?}", severity, types, message);

    vk::FALSE
}

pub struct VkHandles {
    instance: ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
    device: ash::Device,
    queue: vk::Queue,
    debug_utils: DebugUtils,
}

impl VkHandles {
    pub fn new(xr: &XrHandles) -> Self {
        verify_version(xr);
        let entry = unsafe { ash::Entry::load() }.unwrap();
        let instance = create_instance(xr, &entry);
        let physical_device = create_physical_device(xr, &instance);
        let queue_family_index = get_queue_family_index(&instance, physical_device);
        let device = create_device(xr, &entry, &instance, physical_device, queue_family_index);
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let debug_utils = DebugUtils::new(&entry, &instance);
        Self {
            instance,
            physical_device,
            queue_family_index,
            device,
            queue,
            debug_utils,
        }
    }

    pub fn instance(&self) -> &ash::Instance {
        &self.instance
    }

    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    pub fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    pub fn device(&self) -> &ash::Device {
        &self.device
    }

    pub fn queue(&self) -> vk::Queue {
        self.queue
    }

    pub fn debug_utils(&self) -> &DebugUtils {
        &self.debug_utils
    }

    pub fn find_memory_type(&self, type_filter: u32, properties: vk::MemoryPropertyFlags) -> u32 {
        let memory_properties = unsafe {
            self.instance
                .get_physical_device_memory_properties(self.physical_device)
        };
        for i in 0..memory_properties.memory_type_count {
            if type_filter & (1 << i) != 0
                && memory_properties.memory_types[i as usize]
                    .property_flags
                    .contains(properties)
            {
                return i;
            }
        }
        panic!();
    }

    pub fn create_initialized_buffer(
        &self,
        data: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> (vk::Buffer, vk::DeviceMemory) {
        unsafe {
            let buffer = self
                .device()
                .create_buffer(
                    &vk::BufferCreateInfo::builder()
                        .size(data.len() as u64)
                        .usage(usage)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )
                .unwrap();
            let memory_requirements = self.device().get_buffer_memory_requirements(buffer);
            let memory = self
                .device()
                .allocate_memory(
                    &vk::MemoryAllocateInfo::builder()
                        .allocation_size(memory_requirements.size)
                        .memory_type_index(self.find_memory_type(
                            memory_requirements.memory_type_bits,
                            vk::MemoryPropertyFlags::HOST_VISIBLE
                                | vk::MemoryPropertyFlags::HOST_COHERENT,
                        )),
                    None,
                )
                .unwrap();
            self.device().bind_buffer_memory(buffer, memory, 0).unwrap();

            let mapped = self
                .device()
                .map_memory(memory, 0, data.len() as u64, vk::MemoryMapFlags::empty())
                .unwrap();
            copy_nonoverlapping(data.as_ptr(), mapped as *mut _, data.len());
            self.device().unmap_memory(memory);

            (buffer, memory)
        }
    }

    pub unsafe fn destroy(self) {
        self.device.destroy_device(None);
        self.instance.destroy_instance(None);
    }
}

fn verify_version(xr: &XrHandles) {
    let reqs = xr
        .instance
        .graphics_requirements::<xr::Vulkan>(xr.system)
        .unwrap();
    let target = xr::Version::new(TARGET_VK_MAJOR_VERSION, TARGET_VK_MINOR_VERSION, 0);
    if target < reqs.min_api_version_supported || target > reqs.max_api_version_supported {
        panic!(
            "XrGraphicsRequirementsVulkan2KHR Vulkan API version range [{}, {}.{}.*] does not \
            contain the target {}",
            reqs.min_api_version_supported,
            reqs.max_api_version_supported.major(),
            reqs.max_api_version_supported.minor(),
            target,
        );
    }
}

fn create_instance(xr: &XrHandles, entry: &ash::Entry) -> ash::Instance {
    let app_info = vk::ApplicationInfo::builder()
        .application_version(0)
        .engine_version(0)
        .api_version(TARGET_API_VERSION.0);
    let mut instance_create_info = vk::InstanceCreateInfo::builder()
        .application_info(&app_info)
        .enabled_extension_names(&[b"VK_EXT_debug_utils\0" as *const u8 as *const i8]);
    if ENABLE_VK_VALIDATION_LAYER {
        instance_create_info = instance_create_info
            .enabled_layer_names(&[b"VK_LAYER_KHRONOS_validation\0" as *const u8 as *const i8]);
    }

    let instance = unsafe {
        xr.instance.create_vulkan_instance(
            xr.system,
            std::mem::transmute(entry.static_fn().get_instance_proc_addr),
            &instance_create_info.push_next(
                &mut vk::DebugUtilsMessengerCreateInfoEXT::builder()
                    .message_severity(
                        vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                            | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                    )
                    .message_type(
                        vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                            | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE
                            | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
                    )
                    .pfn_user_callback(Some(debug_utils_callback)),
            ) as *const _ as *const _,
        )
    }
    .unwrap()
    .map_err(vk::Result::from_raw)
    .unwrap();
    unsafe { ash::Instance::load(entry.static_fn(), vk::Instance::from_raw(instance as _)) }
}

fn create_physical_device(xr: &XrHandles, instance: &ash::Instance) -> vk::PhysicalDevice {
    let physical_device = vk::PhysicalDevice::from_raw(
        xr.instance
            .vulkan_graphics_device(xr.system, instance.handle().as_raw() as _)
            .unwrap() as _,
    );
    let properties = unsafe { instance.get_physical_device_properties(physical_device) };
    if properties.api_version < TARGET_API_VERSION.0 {
        panic!(
            "VkPhysicalDevice API version {} is below the target {}",
            VkVersion(properties.api_version),
            TARGET_API_VERSION,
        );
    }
    physical_device
}

fn get_queue_family_index(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> u32 {
    unsafe { instance.get_physical_device_queue_family_properties(physical_device) }
        .into_iter()
        .enumerate()
        .find_map(|(queue_family_index, info)| {
            if info.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                Some(queue_family_index as u32)
            } else {
                None
            }
        })
        .unwrap()
}

fn create_device(
    xr: &XrHandles,
    entry: &ash::Entry,
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> ash::Device {
    let device = unsafe {
        xr.instance.create_vulkan_device(
            xr.system,
            std::mem::transmute(entry.static_fn().get_instance_proc_addr),
            physical_device.as_raw() as _,
            &vk::DeviceCreateInfo::builder()
                .queue_create_infos(&[vk::DeviceQueueCreateInfo::builder()
                    .queue_family_index(queue_family_index)
                    .queue_priorities(&[1.0])
                    .build()])
                .push_next(&mut vk::PhysicalDeviceMultiviewFeatures {
                    multiview: vk::TRUE,
                    ..Default::default()
                }) as *const _ as *const _,
        )
    }
    .unwrap()
    .map_err(vk::Result::from_raw)
    .unwrap();
    unsafe { ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(device as _)) }
}
