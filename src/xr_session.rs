use std::marker::PhantomData;

use ash::vk::Handle;
use openxr as xr;

use crate::vk_handles::VkHandles;
use crate::xr_handles::XrHandles;

pub struct XrSession<'a> {
    phantom_lifetime: PhantomData<(&'a XrHandles, &'a VkHandles)>,

    pub session: xr::Session<xr::Vulkan>,
    pub frame_wait: xr::FrameWaiter,
    pub frame_stream: xr::FrameStream<xr::Vulkan>,

    pub stage: xr::Space,
    pub action_set: xr::ActionSet,
    pub left_action: xr::Action<xr::Posef>,
    pub right_action: xr::Action<xr::Posef>,
    pub left_space: xr::Space,
    pub right_space: xr::Space,
}

impl<'a> XrSession<'a> {
    pub fn new(xr: &'a XrHandles, vk: &'a VkHandles) -> Self {
        let (session, frame_wait, frame_stream) = unsafe {
            xr.instance.create_session::<xr::Vulkan>(
                xr.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: vk.instance().handle().as_raw() as _,
                    physical_device: vk.physical_device().as_raw() as _,
                    device: vk.device().handle().as_raw() as _,
                    queue_family_index: vk.queue_family_index(),
                    queue_index: 0,
                },
            )
        }
        .unwrap();

        // Create an action set to encapsulate our actions
        let action_set = xr
            .instance
            .create_action_set("input", "input pose information", 0)
            .unwrap();

        let left_action = action_set
            .create_action::<xr::Posef>("left_hand", "Left Hand Controller", &[])
            .unwrap();
        let right_action = action_set
            .create_action::<xr::Posef>("right_hand", "Right Hand Controller", &[])
            .unwrap();

        // Bind our actions to input devices using the given profile
        // If you want to access inputs specific to a particular device you may specify a different
        // interaction profile
        xr.instance
            .suggest_interaction_profile_bindings(
                xr.instance
                    .string_to_path("/interaction_profiles/valve/index_controller")
                    .unwrap(),
                &[
                    xr::Binding::new(
                        &left_action,
                        xr.instance
                            .string_to_path("/user/hand/left/input/grip/pose")
                            .unwrap(),
                    ),
                    xr::Binding::new(
                        &right_action,
                        xr.instance
                            .string_to_path("/user/hand/right/input/grip/pose")
                            .unwrap(),
                    ),
                ],
            )
            .unwrap();

        // Attach the action set to the session
        session.attach_action_sets(&[&action_set]).unwrap();

        // Create an action space for each device we want to locate
        let left_space = left_action
            .create_space(session.clone(), xr::Path::NULL, xr::Posef::IDENTITY)
            .unwrap();
        let right_space = right_action
            .create_space(session.clone(), xr::Path::NULL, xr::Posef::IDENTITY)
            .unwrap();

        // OpenXR uses a couple different types of reference frames for positioning content; we need
        // to choose one for displaying our content! STAGE would be relative to the center of your
        // guardian system's bounds, and LOCAL would be relative to your device's starting location.
        let stage = session
            .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
            .unwrap();

        Self {
            phantom_lifetime: PhantomData,

            session,
            frame_wait,
            frame_stream,

            stage,
            action_set,
            left_action,
            right_action,
            left_space,
            right_space,
        }
    }
}
