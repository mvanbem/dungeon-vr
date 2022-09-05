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
    pub view: xr::Space,
    pub action_set: xr::ActionSet,
    pub hands: [XrSessionHand<'a>; 2],
}

pub struct XrSessionHand<'a> {
    phantom_lifetime: PhantomData<&'a XrSession<'a>>,

    pub pose_action: xr::Action<xr::Posef>,
    pub pose_space: xr::Space,
    pub squeeze_action: xr::Action<f32>,
    pub squeeze_force_action: xr::Action<f32>,
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
        let stage = session
            .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
            .unwrap();
        let view = session
            .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
            .unwrap();
        let action_set = xr
            .instance
            .create_action_set("input", "input pose information", 0)
            .unwrap();

        let hands = [
            XrSessionHand::new(&session, &action_set, 0),
            XrSessionHand::new(&session, &action_set, 1),
        ];

        // Bind our actions to input devices using the given profile
        // If you want to access inputs specific to a particular device you may specify a different
        // interaction profile
        xr.instance
            .suggest_interaction_profile_bindings(
                xr.instance
                    .string_to_path("/interaction_profiles/valve/index_controller")
                    .unwrap(),
                &[
                    hands[0].pose_binding(xr, 0),
                    hands[1].pose_binding(xr, 1),
                    hands[0].squeeze_binding(xr, 0),
                    hands[1].squeeze_binding(xr, 1),
                    hands[0].squeeze_force_binding(xr, 0),
                    hands[1].squeeze_force_binding(xr, 1),
                ],
            )
            .unwrap();

        // Attach the action set to the session
        session.attach_action_sets(&[&action_set]).unwrap();

        Self {
            phantom_lifetime: PhantomData,

            session,
            frame_wait,
            frame_stream,

            stage,
            view,
            action_set,
            hands,
        }
    }
}

impl<'a> XrSessionHand<'a> {
    pub fn new(
        session: &xr::Session<xr::Vulkan>,
        action_set: &xr::ActionSet,
        index: usize,
    ) -> Self {
        let pose_action = action_set
            .create_action::<xr::Posef>(
                ["left_hand", "right_hand"][index],
                ["Left Hand Controller", "Right Hand Controller"][index],
                &[],
            )
            .unwrap();
        let pose_space = pose_action
            .create_space(session.clone(), xr::Path::NULL, xr::Posef::IDENTITY)
            .unwrap();

        let squeeze_action = action_set
            .create_action::<f32>(
                ["left_squeeze", "right_squeeze"][index],
                ["Left Hand Squeeze", "Right Hand Squeeze"][index],
                &[],
            )
            .unwrap();
        let squeeze_force_action = action_set
            .create_action::<f32>(
                ["left_squeeze_force", "right_squeeze_force"][index],
                ["Left Hand Squeeze Force", "Right Hand Squeeze Force"][index],
                &[],
            )
            .unwrap();

        Self {
            phantom_lifetime: PhantomData,

            pose_action,
            pose_space,
            squeeze_action,
            squeeze_force_action,
        }
    }

    fn pose_binding<'b>(&'b self, xr: &'b XrHandles, index: usize) -> xr::Binding<'b> {
        xr::Binding::new(
            &self.pose_action,
            xr.instance
                .string_to_path(
                    [
                        "/user/hand/left/input/grip/pose",
                        "/user/hand/right/input/grip/pose",
                    ][index],
                )
                .unwrap(),
        )
    }

    fn squeeze_binding<'b>(&'b self, xr: &'b XrHandles, index: usize) -> xr::Binding<'b> {
        xr::Binding::new(
            &self.squeeze_action,
            xr.instance
                .string_to_path(
                    [
                        "/user/hand/left/input/squeeze/value",
                        "/user/hand/right/input/squeeze/value",
                    ][index],
                )
                .unwrap(),
        )
    }

    fn squeeze_force_binding<'b>(&'b self, xr: &'b XrHandles, index: usize) -> xr::Binding<'b> {
        xr::Binding::new(
            &self.squeeze_force_action,
            xr.instance
                .string_to_path(
                    [
                        "/user/hand/left/input/squeeze/force",
                        "/user/hand/right/input/squeeze/force",
                    ][index],
                )
                .unwrap(),
        )
    }
}
