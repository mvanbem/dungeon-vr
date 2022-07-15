use openxr as xr;

use crate::VIEW_TYPE;

pub struct XrHandles {
    pub instance: xr::Instance,
    pub system: xr::SystemId,
    pub environment_blend_mode: xr::EnvironmentBlendMode,
}

impl XrHandles {
    pub fn new() -> Self {
        let instance = create_xr_instance();
        let system = get_system(&instance);
        let environment_blend_mode = get_environment_blend_mode(&instance, system);
        Self {
            instance,
            system,
            environment_blend_mode,
        }
    }
}

fn create_xr_instance() -> xr::Instance {
    let entry = xr::Entry::linked();

    let available_extensions = entry.enumerate_extensions().unwrap();
    assert!(available_extensions.khr_vulkan_enable2);

    let mut enabled_extensions = xr::ExtensionSet::default();
    enabled_extensions.khr_vulkan_enable2 = true;
    let xr_instance = entry
        .create_instance(
            &xr::ApplicationInfo {
                application_name: "vrgame",
                application_version: 0,
                engine_name: "",
                engine_version: 0,
            },
            &enabled_extensions,
            &[],
        )
        .unwrap();
    let instance_props = xr_instance.properties().unwrap();
    println!(
        "OpenXR runtime: {} {}",
        instance_props.runtime_name, instance_props.runtime_version,
    );
    xr_instance
}

fn get_system(xr_instance: &xr::Instance) -> xr::SystemId {
    xr_instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .unwrap()
}

fn get_environment_blend_mode(
    instance: &xr::Instance,
    system: xr::SystemId,
) -> xr::EnvironmentBlendMode {
    instance
        .enumerate_environment_blend_modes(system, VIEW_TYPE)
        .unwrap()[0]
}
