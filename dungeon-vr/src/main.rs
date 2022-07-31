use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use ash::vk;
use bytemuck::{Pod, Zeroable};
use clap::Parser;
use dungeon_vr_client::Client;
use openxr as xr;
use rapier3d::na as nalgebra;
use rapier3d::na::{self, matrix, vector, Matrix4};
use slotmap::Key;

use crate::asset::{MaterialAssetKey, MaterialAssets, ModelAssets};
use crate::interop::xr_posef_to_na_isometry;
use crate::local_game::{LocalGame, VrHand, VrTracking};
use crate::model::Primitive;
use crate::render_data::RenderData;
use crate::swapchain::Swapchain;
use crate::vk_handles::VkHandles;
use crate::xr_handles::XrHandles;
use crate::xr_session::{XrSession, XrSessionHand};

mod asset;
mod collider_cache;
mod interop;
mod local_game;
mod material;
mod model;
mod render_data;
mod swapchain;
mod textured;
mod untextured;
mod vk_handles;
mod xr_handles;
mod xr_session;

const RENDER_CONCURRENCY: u32 = 2;

const COLOR_FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;
const DEPTH_FORMAT: vk::Format = vk::Format::D32_SFLOAT;
const VIEW_COUNT: u32 = 2;
const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Exits after one trip through the main loop.
    #[clap(long)]
    abort_after_first_frame: bool,

    /// Enable the Vulkan validation layer.
    #[clap(long)]
    vulkan_validation: bool,

    /// Connects to a remote server at this address. Port 7777 if unspecified.
    #[clap(long)]
    connect: Option<String>,
}

#[tokio::main]
pub async fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_target(false)
        .init();
    let args = Args::parse();
    if let Some(host) = &args.connect {
        let client = Client::spawn(SocketAddr::from_str(host).unwrap())
            .await
            .unwrap();
        // TODO
        Box::leak(Box::new(client));
    }

    let running = set_ctrlc_handler();

    let xr = XrHandles::new();
    let vk = VkHandles::new(&args, &xr);
    let mut xrs = XrSession::new(&xr, &vk);
    let render = RenderData::new(&vk);
    let mut material_assets = MaterialAssets::new(&vk, &render, &mut ());
    let mut model_assets = ModelAssets::new(&vk, &render, &mut material_assets);
    let mut game = LocalGame::new(&vk, &render, &mut material_assets, &mut model_assets);

    let mut swapchain = None;
    main_loop(
        &args,
        running,
        &vk,
        &xr,
        &mut xrs,
        &render,
        &mut swapchain,
        &mut material_assets,
        &mut model_assets,
        &mut game,
    );

    drop(xrs);
    render.wait_for_fences(&vk);
    unsafe {
        model_assets.destroy(&vk, &render);
        material_assets.destroy(&vk, &render);
        render.destroy(vk.device());
        if let Some(swapchain) = swapchain {
            swapchain.destroy(vk.device());
        }
        vk.destroy();
    }
    Ok(())
}

fn set_ctrlc_handler() -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(true));
    ctrlc::set_handler({
        let running = Arc::clone(&running);
        move || {
            running.store(false, Ordering::Relaxed);
        }
    })
    .unwrap();
    running
}

fn build_view_matrix(view: xr::View) -> Matrix4<f32> {
    xr_posef_to_na_isometry(view.pose).inverse().to_matrix()
}

fn build_projection_matrix(fov: xr::Fovf) -> Matrix4<f32> {
    let near_z = 0.01;
    let far_z = 1000.0;

    let right = fov.angle_right.tan();
    let left = fov.angle_left.tan();
    let down = fov.angle_down.tan();
    let up = fov.angle_up.tan();
    frustum(
        near_z * left,
        near_z * right,
        near_z * down,
        near_z * up,
        near_z,
        far_z,
    ) * Matrix4::from_diagonal(&vector![1.0, -1.0, 1.0, 1.0])
}

// Like glFrustum.
fn frustum(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Matrix4<f32> {
    matrix![(2.0 * near) / (right - left), 0.0, (right + left) / (right - left), 0.0;
            0.0, (2.0 * near) / (top - bottom), (top + bottom) / (top - bottom), 0.0;
            0.0, 0.0, -(far + near) / (far - near), -(2.0 * far * near) / (far - near);
            0.0, 0.0, -1.0, 0.0]
}

fn main_loop<'a>(
    args: &Args,
    running: Arc<AtomicBool>,
    vk: &'a VkHandles,
    xr: &XrHandles,
    xrs: &mut XrSession,
    render: &RenderData,
    swapchain: &mut Option<Swapchain<'a>>,
    material_assets: &mut MaterialAssets,
    model_assets: &mut ModelAssets,
    game: &mut LocalGame,
) {
    let mut event_storage = xr::EventDataBuffer::new();
    let mut session_running = false;
    let mut frame = 0;
    'main_loop: loop {
        if !running.load(Ordering::Relaxed) {
            match xrs.session.request_exit() {
                Ok(()) => {}
                Err(xr::sys::Result::ERROR_SESSION_NOT_RUNNING) => break,
                Err(e) => panic!("{}", e),
            }
        }

        while let Some(event) = xr.instance.poll_event(&mut event_storage).unwrap() {
            use xr::Event::*;
            match event {
                SessionStateChanged(e) => {
                    println!("XR session state: {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            xrs.session.begin(VIEW_TYPE).unwrap();
                            session_running = true;
                        }
                        xr::SessionState::STOPPING => {
                            xrs.session.end().unwrap();
                            session_running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            break 'main_loop;
                        }
                        _ => {}
                    }
                }
                InstanceLossPending(_) => {
                    break 'main_loop;
                }
                EventsLost(e) => {
                    println!("lost {} events", e.lost_event_count());
                }
                _ => {}
            }
        }

        if !session_running {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }

        // Prepare to render if XR says we should.
        let xr_frame_state = xrs.frame_wait.wait().unwrap();
        xrs.frame_stream.begin().unwrap();
        if !xr_frame_state.should_render {
            xrs.frame_stream
                .end(
                    xr_frame_state.predicted_display_time,
                    xr.environment_blend_mode,
                    &[],
                )
                .unwrap();
            continue;
        }

        // Acquire the next swapchain image.
        let swapchain = swapchain.get_or_insert_with(|| Swapchain::new(xr, vk, xrs, render));
        let image_index = swapchain.handle_mut().acquire_image().unwrap();
        swapchain
            .handle_mut()
            .wait_image(xr::Duration::INFINITE)
            .unwrap();

        // Wait for this set of render resources to be available.
        let frame_resources = render.frame_resources(frame);
        unsafe {
            vk.device()
                .wait_for_fences(&[frame_resources.fence()], true, u64::MAX)
                .unwrap();
            vk.device()
                .reset_fences(&[frame_resources.fence()])
                .unwrap();
        }

        // Begin the render pass.
        let cmd = frame_resources.cmd();
        unsafe {
            vk.device()
                .begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::builder()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .unwrap();
            vk.device().cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::builder()
                    .render_pass(render.render_pass)
                    .framebuffer(swapchain.buffers()[image_index as usize].framebuffer())
                    .render_area(vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent: swapchain.dimensions(),
                    })
                    .clear_values(&[
                        vk::ClearValue {
                            color: vk::ClearColorValue {
                                float32: [0.0, 0.0, 0.0, 1.0],
                            },
                        },
                        vk::ClearValue {
                            depth_stencil: vk::ClearDepthStencilValue {
                                depth: 1.0,
                                stencil: 0,
                            },
                        },
                    ]),
                vk::SubpassContents::INLINE,
            )
        }

        // Set dynamic state.
        unsafe {
            vk.device().cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: swapchain.dimensions().width as f32,
                    height: swapchain.dimensions().height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            vk.device().cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: swapchain.dimensions(),
                }],
            );
        }

        // Build view-projection matrices and write them to the uniform buffer.
        let (_, views) = xrs
            .session
            .locate_views(VIEW_TYPE, xr_frame_state.predicted_display_time, &xrs.stage)
            .unwrap();
        frame_resources.write_view_proj_matrix(
            vk,
            [
                build_projection_matrix(views[0].fov) * build_view_matrix(views[0]),
                build_projection_matrix(views[1].fov) * build_view_matrix(views[1]),
            ],
        );
        unsafe {
            vk.device().cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                render.untextured_pipeline_layout,
                0,
                &[frame_resources.per_frame_descriptor_set()],
                &[],
            );
            vk.device().cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                render.textured_pipeline_layout,
                0,
                &[frame_resources.per_frame_descriptor_set()],
                &[],
            );
        }

        // Read inputs.
        xrs.session
            .sync_actions(&[(&xrs.action_set).into()])
            .unwrap();
        let capture_hand = |hand: &XrSessionHand| VrHand {
            pose: if hand
                .pose_action
                .is_active(&xrs.session, xr::Path::NULL)
                .unwrap()
            {
                xr_posef_to_na_isometry(
                    hand.pose_space
                        .locate(&xrs.stage, xr_frame_state.predicted_display_time)
                        .unwrap()
                        .pose,
                )
            } else {
                na::one()
            },
            squeeze: hand
                .squeeze_action
                .state(&xrs.session, xr::Path::NULL)
                .unwrap()
                .current_state,
            squeeze_force: hand
                .squeeze_force_action
                .state(&xrs.session, xr::Path::NULL)
                .unwrap()
                .current_state,
        };
        let vr_tracking = VrTracking {
            head: xr::Posef::IDENTITY,
            hands: [capture_hand(&xrs.hands[0]), capture_hand(&xrs.hands[1])],
        };

        // Step the game and extract rendering data.
        let models = game.update(vr_tracking);

        // Group primitive instances.
        let mut transforms_by_primitive_by_material: HashMap<
            MaterialAssetKey,
            HashMap<RefEq<Primitive>, Vec<Matrix4<f32>>>,
        > = Default::default();
        for (model_key, transforms) in models {
            let model = model_assets.get(model_key);
            for primitive in &model.primitives {
                transforms_by_primitive_by_material
                    .entry(primitive.material)
                    .or_default()
                    .entry(RefEq(primitive))
                    .or_default()
                    .extend(transforms.clone());
            }
        }

        // Draw all primitives.
        for (material_key, transforms_by_primitive) in transforms_by_primitive_by_material {
            let pipeline_layout = if material_key.is_null() {
                unsafe {
                    vk.device().cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        render.untextured_pipeline,
                    );
                }
                render.untextured_pipeline_layout
            } else {
                unsafe {
                    vk.device().cmd_bind_pipeline(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        render.textured_pipeline,
                    );
                    vk.device().cmd_bind_descriptor_sets(
                        cmd,
                        vk::PipelineBindPoint::GRAPHICS,
                        render.textured_pipeline_layout,
                        1,
                        &[material_assets.get(material_key).descriptor_set],
                        &[],
                    );
                }
                render.textured_pipeline_layout
            };

            for (RefEq(primitive), transforms) in transforms_by_primitive {
                unsafe {
                    vk.device()
                        .cmd_bind_vertex_buffers(cmd, 0, &[primitive.vertex_buffer], &[0]);
                    vk.device().cmd_bind_index_buffer(
                        cmd,
                        primitive.index_buffer,
                        0,
                        primitive.index_type,
                    );
                }
                for model in &transforms {
                    unsafe {
                        vk.device().cmd_push_constants(
                            cmd,
                            pipeline_layout,
                            vk::ShaderStageFlags::VERTEX,
                            0,
                            bytemuck::bytes_of(&PushConstants {
                                model: *model.as_ref(),
                            }),
                        );
                        vk.device()
                            .cmd_draw_indexed(cmd, primitive.count as u32, 1, 0, 0, 0);
                    }
                }
            }
        }

        // Finish rendering and release the swapchain image.
        unsafe {
            vk.device().cmd_end_render_pass(cmd);
            vk.device().cmd_resolve_image(
                cmd,
                swapchain.color_image(),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                swapchain.buffers()[image_index as usize].swapchain_color_image(),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                &[vk::ImageResolve {
                    src_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: VIEW_COUNT,
                    },
                    src_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                    dst_subresource: vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: VIEW_COUNT,
                    },
                    dst_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                    extent: vk::Extent3D {
                        width: swapchain.dimensions().width,
                        height: swapchain.dimensions().height,
                        depth: 1,
                    },
                }],
            );
            vk.device().end_command_buffer(cmd).unwrap();
            vk.device()
                .queue_submit(
                    vk.queue(),
                    &[vk::SubmitInfo::builder().command_buffers(&[cmd]).build()],
                    frame_resources.fence(),
                )
                .unwrap();
        }
        swapchain.handle_mut().release_image().unwrap();

        // Report back to XR.
        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: swapchain.dimensions().width as _,
                height: swapchain.dimensions().height as _,
            },
        };
        xrs.frame_stream
            .end(
                xr_frame_state.predicted_display_time,
                xr.environment_blend_mode,
                &[&xr::CompositionLayerProjection::new()
                    .space(&xrs.stage)
                    .views(&[
                        xr::CompositionLayerProjectionView::new()
                            .pose(views[0].pose)
                            .fov(views[0].fov)
                            .sub_image(
                                xr::SwapchainSubImage::new()
                                    .swapchain(swapchain.handle())
                                    .image_array_index(0)
                                    .image_rect(rect),
                            ),
                        xr::CompositionLayerProjectionView::new()
                            .pose(views[1].pose)
                            .fov(views[1].fov)
                            .sub_image(
                                xr::SwapchainSubImage::new()
                                    .swapchain(swapchain.handle())
                                    .image_array_index(1)
                                    .image_rect(rect),
                            ),
                    ])],
            )
            .unwrap();

        if args.abort_after_first_frame {
            panic!("end of first frame");
        }

        frame = (frame + 1) % RENDER_CONCURRENCY as usize;
    }
}

#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
struct PushConstants {
    model: [[f32; 4]; 4],
}

const NOOP_STENCIL_STATE: vk::StencilOpState = vk::StencilOpState {
    fail_op: vk::StencilOp::KEEP,
    pass_op: vk::StencilOp::KEEP,
    depth_fail_op: vk::StencilOp::KEEP,
    compare_op: vk::CompareOp::ALWAYS,
    compare_mask: 0,
    write_mask: 0,
    reference: 0,
};

#[derive(Clone, Copy)]
struct RefEq<'a, T>(&'a T);

impl<'a, T> PartialEq for RefEq<'a, T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 as *const T == other.0 as *const T
    }
}

impl<'a, T> Eq for RefEq<'a, T> {}

impl<'a, T> Hash for RefEq<'a, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.0 as *const T).hash(state);
    }
}
