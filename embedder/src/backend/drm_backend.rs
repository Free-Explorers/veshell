use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use rustix::fs::OFlags;
use smithay::backend::allocator::dmabuf::{AnyError, AsDmabuf, Dmabuf, DmabufAllocator};
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags};
use smithay::backend::allocator::{Allocator, Fourcc, Slot, Swapchain};
use smithay::backend::drm::compositor::DrmCompositor;
use smithay::backend::drm::{
    CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmNode,
    NodeType,
};
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::gles::ffi::Gles2;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportAll, ImportDma, ImportEgl, Renderer};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{libseat, Event as SessionEvent, Session};
use smithay::backend::udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent};
use smithay::backend::{egl, SwapBuffersError};
use smithay::delegate_dmabuf;
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Mode;
use smithay::output::{Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::channel::Event;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::{EventLoop, LoopHandle};
use smithay::reexports::drm::control::{connector, crtc, Device, ModeTypeFlags};
use smithay::reexports::drm::Device as _;
use smithay::reexports::input::Libinput;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::reexports::wayland_server::protocol::wl_shm;
use smithay::reexports::wayland_server::Display;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::{DeviceFd, IsAlive, Rectangle};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
};
use smithay::wayland::drm_lease::DrmLease;
use tracing::{debug, error, info, warn};

use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use smithay_drm_extras::edid::EdidInfo;

use crate::flutter_engine::FlutterEngine;
use crate::state;
use crate::{flutter_engine::EmbedderChannels, send_frames_surface_tree, State};

use super::render::{get_render_elements, CLEAR_COLOR};
use super::Backend;

pub struct DrmBackend {
    pub session: LibSeatSession,
    gpus: HashMap<DrmNode, GpuData>,
    primary_gpu: DrmNode,
    //gpu_manager: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    highest_hz_crtc: Option<(i32, crtc::Handle)>,
}

impl Backend for DrmBackend {
    const HAS_RELATIVE_MOTION: bool = true;

    fn seat_name(&self) -> String {
        self.session.seat()
    }

    fn get_session(&self) -> LibSeatSession {
        self.session.clone()
    }

    fn with_primary_renderer<T>(&mut self, f: impl FnOnce(&GlesRenderer) -> T) -> Option<T> {
        /*  let renderer = self.gpu_manager.single_renderer(&self.primary_gpu).ok()?;
        Some(f(renderer.as_gles_renderer())) */
        Some(f(&self.get_gpu_data_mut().renderer))
    }

    fn with_primary_renderer_mut<T>(
        &mut self,
        f: impl FnOnce(&mut GlesRenderer) -> T,
    ) -> Option<T> {
        /* let mut renderer = self.g.single_renderer(&self.primary_gpu).ok()?;
        Some(f(renderer.as_gles_renderer_mut())) */
        Some(f(&mut self.get_gpu_data_mut().renderer))
    }
}

impl DrmBackend {
    fn get_gpu_data_mut(&mut self) -> &mut GpuData {
        let primary_node = self
            .primary_gpu
            .node_with_type(NodeType::Primary)
            .and_then(|x| x.ok())
            .unwrap();

        self.gpus.get_mut(&primary_node).unwrap()
    }
}

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

// we cannot simply pick the first supported format of the intersection of *all* formats, because:
// - we do not want something like Abgr4444, which looses color information, if something better is available
// - some formats might perform terribly
// - we might need some work-arounds, if one supports modifiers, but the other does not
//
// So lets just pick `ARGB2101010` (10-bit) or `ARGB8888` (8-bit) for now, they are widely supported.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
const SUPPORTED_FORMATS_8BIT_ONLY: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

impl DmabufHandler for State<DrmBackend> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        self.dmabuf_state.as_mut().unwrap()
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        /* if self
            .backend_data
            .gpu_manager
            .single_renderer(&self.backend_data.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .is_ok()
        {
            dmabuf.set_node(self.backend_data.primary_gpu);
            let _ = notifier.successful::<State<DrmBackend>>();
        } else {
            notifier.failed();
        } */
        if self
            .backend_data
            .get_gpu_data_mut()
            .renderer
            .import_dmabuf(&dmabuf, None)
            .is_ok()
        {
            dmabuf.set_node(self.backend_data.primary_gpu);
            let _ = notifier.successful::<State<DrmBackend>>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State<DrmBackend>);

pub fn run_drm_backend() {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display: Display<State<DrmBackend>> = Display::new().unwrap();
    let mut display_handle = display.handle();

    let (session, notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            error!("Could not initialize a session: {}", err);
            return;
        }
    };

    let primary_gpu = if let Ok(var) = std::env::var("DRM_DEVICE") {
        DrmNode::from_path(var).expect("Invalid drm device path")
    } else {
        primary_gpu(session.seat())
            .unwrap()
            .and_then(|x| {
                DrmNode::from_path(x)
                    .ok()?
                    .node_with_type(NodeType::Render)?
                    .ok()
            })
            .unwrap_or_else(|| {
                all_gpus(session.seat())
                    .unwrap()
                    .into_iter()
                    .find_map(|x| DrmNode::from_path(x).ok())
                    .expect("No GPU!")
            })
    };
    info!("Using {} as primary gpu.", primary_gpu);

    let udev_backend = match UdevBackend::new(session.seat()) {
        Ok(ret) => ret,
        Err(err) => {
            error!(error = ?err, "Failed to initialize udev backend");
            return;
        }
    };

    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context.udev_assign_seat(&session.seat()).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /* let gpu_manager =
    GpuManager::new(GbmGlesBackend::with_context_priority(ContextPriority::High)).unwrap(); */

    let mut state = State::new(
        display,
        event_loop.handle(),
        DrmBackend {
            session,
            gpus: HashMap::new(),
            //gpu_manager: gpu_manager,
            primary_gpu,
            highest_hz_crtc: None,
        },
        None,
    );

    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), data| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                info!("pausing session");

                for backend in data.backend_data.gpus.values_mut() {
                    backend.drm_device.pause();
                    backend.active_leases.clear();
                    /* if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.suspend();
                    } */
                }
            }
            SessionEvent::ActivateSession => {
                info!("resuming session");

                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }
                for (node, backend) in data
                    .backend_data
                    .gpus
                    .iter_mut()
                    .map(|(handle, backend)| (*handle, backend))
                {
                    // if we do not care about flicking (caused by modesetting) we could just
                    // pass true for disable connectors here. this would make sure our drm
                    // device is in a known state (all connectors and planes disabled).
                    // but for demonstration we choose a more optimistic path by leaving the
                    // state as is and assume it will just work. If this assumption fails
                    // we will try to reset the state when trying to queue a frame.
                    backend
                        .drm_device
                        .activate(false)
                        .expect("failed to activate drm backend");
                    /* if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.resume::<State<DrmBackend>>();
                    } */
                    for surface in backend.surfaces.values_mut() {
                        if let Err(err) = surface.compositor.reset_state() {
                            warn!("Failed to reset drm surface state: {}", err);
                        }
                    }
                    data.loop_handle
                        .insert_idle(move |data| data.render(node, None));
                }
            }
        })
        .unwrap();

    // Initialize GPU state.
    /* for (device_id, path) in udev_backend.device_list() {
        if let Err(err) = DrmNode::from_dev_id(device_id)
            .map_err(DeviceAddError::DrmNode)
            .and_then(|node| state.device_added(node, path))
        {
            error!("Skipping device {device_id}: {err}");
        }
    } */

    let primary_node = primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|x| x.ok())
        .unwrap();
    state
        .device_added(primary_node, &primary_node.dev_path().unwrap())
        .unwrap();

    /*     let mut renderer = match state.backend_data.gpu_manager.single_renderer(&primary_gpu) {
        Ok(renderer) => renderer,
        Err(err) => {
            error!("Failed to create renderer: {:?}", err);
            return;
        }
    }; */

    let renderer = &mut state.backend_data.get_gpu_data_mut().renderer;
    let egl_context = renderer.egl_context();

    let dmabuf_formats = egl_context
        .dmabuf_texture_formats()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let dmabuf_default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();

    let mut dmabuf_state = DmabufState::new();
    let _dmabuf_global = dmabuf_state.create_global_with_default_feedback::<State<DrmBackend>>(
        &display_handle,
        &dmabuf_default_feedback,
    );

    state.dmabuf_state = Some(dmabuf_state);

    info!(
        ?primary_gpu,
        "Trying to initialize EGL Hardware Acceleration",
    );
    match renderer.bind_wl_display(&state.display_handle) {
        Ok(_) => info!("EGL hardware-acceleration enabled"),
        Err(err) => info!(?err, "Failed to initialize EGL hardware-acceleration"),
    }

    state.gl = Some(Gles2::load_with(
        |s| unsafe { egl::get_proc_address(s) } as *const _
    ));

    let dmabuf_formats = renderer.dmabuf_formats();

    let dmabuf_default_feedback =
        DmabufFeedbackBuilder::new(state.backend_data.primary_gpu.dev_id(), dmabuf_formats)
            .build()
            .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let _dmabuf_global = dmabuf_state.create_global_with_default_feedback::<State<DrmBackend>>(
        &state.display_handle,
        &dmabuf_default_feedback,
    );

    state.dmabuf_state = Some(dmabuf_state);
    // Start the Flutter engine.
    let (
        flutter_engine,
        EmbedderChannels {
            rx_present,
            rx_request_fbo,
            tx_fbo,
            tx_output_height: _,
            rx_baton,
        },
    ) = FlutterEngine::new(&mut state).unwrap();
    state.tx_fbo = Some(tx_fbo.clone());
    state.flutter_engine = Some(flutter_engine);

    let nodes_available: Vec<DrmNode> = state.backend_data.gpus.keys().copied().collect();

    // Initialize already present connectors.
    for node in nodes_available {
        state.device_changed(node);
    }

    // Mandatory formats by the Wayland spec.
    // TODO: Add more formats based on the GLES version.
    state
        .shm_state
        .update_formats([wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]);

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, data| {
            let _dh = data.display_handle.clone();
            data.handle_input(&event);
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, data| match event {
            UdevEvent::Added {
                device_id: _,
                path: _,
            } => {
                /* debug!("UdevEvent::Added {{ device_id: {device_id}");
                if let Err(err) = DrmNode::from_dev_id(device_id)
                    .map_err(DeviceAddError::DrmNode)
                    .and_then(|node| data.device_added(node, &path))
                {
                    error!("Skipping device {device_id}: {err}");
                } */
            }
            UdevEvent::Changed { device_id } => {
                debug!("UdevEvent::Changed {{ device_id: {device_id} }}");
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_changed(node)
                }
            }
            UdevEvent::Removed { device_id: _ } => {
                /* debug!("UdevEvent::Removed {{ device_id: {device_id} }}");
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_removed(node)
                } */
            }
        })
        .unwrap();

    // Mandatory formats by the Wayland spec.
    // TODO: Add more formats based on the GLES version.
    state
        .shm_state
        .update_formats([wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]);

    event_loop
        .handle()
        .insert_source(rx_baton, move |baton, _, data| {
            if let Event::Msg(baton) = baton {
                data.batons.push(baton);
            }
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(rx_request_fbo, move |_, _, data| {
            let gpu_data = data.backend_data.get_gpu_data_mut();
            let slot = gpu_data
                .swapchain
                .as_mut()
                .unwrap()
                .acquire()
                .ok()
                .flatten()
                .unwrap();
            let dmabuf = slot.export().unwrap();
            gpu_data.current_slot = Some(slot);
            data.tx_fbo.as_ref().unwrap().send(Some(dmabuf)).unwrap();
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(rx_present, move |_, _, data| {
            let gpu_data = data.backend_data.get_gpu_data_mut();
            gpu_data.last_rendered_slot = gpu_data.current_slot.take();

            let gpu_data = data.backend_data.get_gpu_data_mut();
            if let Some(ref slot) = gpu_data.last_rendered_slot {
                gpu_data.swapchain.as_mut().unwrap().submitted(slot);
            }
        })
        .unwrap();

    state::State::<DrmBackend>::start_xwayland(&mut state);

    while state.running.load(Ordering::SeqCst) {
        let result = event_loop.dispatch(None, &mut state);
        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            display_handle.flush_clients().unwrap();
        }
    }

    // Avoid indefinite hang in the Flutter render thread waiting for new rbo.
    drop(tx_fbo);
}

impl State<DrmBackend> {
    fn monitor_layout_changed(&mut self) {
        let monitors = self.space.outputs().cloned().collect::<Vec<_>>();
        self.flutter_engine_mut().monitor_layout_changed(monitors);
    }
    fn determine_highest_hz_crtc(&mut self) {
        self.backend_data.highest_hz_crtc = self
            .space
            .outputs()
            // Ignore outputs that don't have a mode.
            .filter_map(|output| output.current_mode().map(|mode| (mode.refresh, output)))
            // Take the one with the highest refresh rate.
            .max_by_key(|(refresh, _)| *refresh)
            .map(|(refresh, output)| {
                (
                    refresh,
                    output.user_data().get::<UdevOutputId>().unwrap().crtc,
                )
            });
    }
}

#[allow(dead_code)]
struct GpuData {
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    non_desktop_connectors: Vec<(connector::Handle, crtc::Handle)>,
    active_leases: Vec<DrmLease>,
    gbm_device: GbmDevice<DrmDeviceFd>,
    gbm_allocator: GbmAllocator<DrmDeviceFd>,
    drm_device: DrmDevice,
    drm_scanner: DrmScanner,
    render_node: DrmNode,
    registration_token: RegistrationToken,
    swapchain: Option<Swapchain<Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError> + 'static>>>,
    current_slot: Option<Slot<Dmabuf>>,
    last_rendered_slot: Option<Slot<Dmabuf>>,
    renderer: GlesRenderer,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
enum DeviceAddError {
    #[error("Failed to open device using libseat: {0}")]
    DeviceOpen(libseat::Error),
    #[error("Failed to initialize drm device: {0}")]
    DrmDevice(DrmError),
    #[error("Failed to initialize gbm device: {0}")]
    GbmDevice(std::io::Error),
    #[error("Failed to access drm node: {0}")]
    DrmNode(CreateDrmNodeError),
    #[error("Failed to add device to GpuManager: {0}")]
    AddNode(egl::Error),
}

impl State<DrmBackend> {
    fn connector_connected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        let device = if let Some(device) = self.backend_data.gpus.get_mut(&node) {
            device
        } else {
            return;
        };

        let output_name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        info!(?crtc, "Trying to setup connector {}", output_name,);

        let non_desktop = device
            .drm_device
            .get_properties(connector.handle())
            .ok()
            .and_then(|props| {
                let (info, value) = props
                    .into_iter()
                    .filter_map(|(handle, value)| {
                        let info = device.drm_device.get_property(handle).ok()?;
                        Some((info, value))
                    })
                    .find(|(info, _)| info.name().to_str() == Ok("non-desktop"))?;

                info.value_type().convert_value(value).as_boolean()
            })
            .unwrap_or(false);

        let (make, model) = EdidInfo::for_connector(&device.drm_device, connector.handle())
            .map(|info| (info.manufacturer, info.model))
            .unwrap_or_else(|| ("Unknown".into(), "Unknown".into()));

        if non_desktop {
            info!(
                "Connector {} is non-desktop, setting up for leasing",
                output_name
            );
            device
                .non_desktop_connectors
                .push((connector.handle(), crtc));
            return;
        }

        // check if there is a file in xdgConfigHome/veshell/persistence/Monitor/<output_name>.json and if so, get mode from there
        // if not, get the preferred mode from the connector
        info!("output_name: {}", output_name);
        let mode_id = get_mode_id_for_monitor_from_file(&output_name).unwrap_or_else(|| {
            connector
                .modes()
                .iter()
                .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(0)
        });

        let drm_mode = connector.modes()[mode_id];
        let wl_mode = Mode::from(drm_mode);

        let surface = match device
            .drm_device
            .create_surface(crtc, drm_mode, &[connector.handle()])
        {
            Ok(surface) => surface,
            Err(err) => {
                warn!("Failed to create drm surface: {}", err);
                return;
            }
        };

        let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            output_name,
            PhysicalProperties {
                size: (phys_w as i32, phys_h as i32).into(),
                subpixel: Subpixel::Unknown,
                make,
                model,
            },
        );
        let global = output.create_global::<State<DrmBackend>>(&self.display_handle);

        // Put the new output at the right of the last one.
        let x = self.space.outputs().fold(0, |acc, o| {
            acc + self.space.output_geometry(o).unwrap().size.w
        });
        let position = (x, 0).into();

        output.set_preferred(wl_mode);
        output.change_current_state(Some(wl_mode), None, None, Some(position));
        self.space.map_output(&output, position);

        output.user_data().insert_if_missing(|| UdevOutputId {
            crtc,
            device_id: node,
        });

        let color_formats = if std::env::var("ANVIL_DISABLE_10BIT").is_ok() {
            SUPPORTED_FORMATS_8BIT_ONLY
        } else {
            SUPPORTED_FORMATS
        };

        let renderer = &mut device.renderer;

        let render_formats = renderer.egl_context().dmabuf_render_formats().clone();

        let driver = match device.drm_device.get_driver() {
            Ok(driver) => driver,
            Err(err) => {
                warn!("Failed to query drm driver: {}", err);
                return;
            }
        };

        let mut planes = surface.planes().clone();

        // Using an overlay plane on a nvidia card breaks
        if driver
            .name()
            .to_string_lossy()
            .to_lowercase()
            .contains("nvidia")
            || driver
                .description()
                .to_string_lossy()
                .to_lowercase()
                .contains("nvidia")
        {
            planes.overlay = vec![];
        }

        let compositor = match DrmCompositor::new(
            &output,
            surface,
            Some(planes),
            device.gbm_allocator.clone(),
            device.gbm_device.clone(),
            color_formats,
            render_formats,
            device.drm_device.cursor_size(),
            Some(device.gbm_device.clone()),
        ) {
            Ok(compositor) => compositor,
            Err(err) => {
                warn!("Failed to create drm compositor: {}", err);
                return;
            }
        };

        let surface = SurfaceData {
            dh: self.display_handle.clone(),
            device_id: node,
            crtc,
            render_node: device.render_node,
            global: Some(global),
            compositor,
        };

        device.surfaces.insert(crtc, surface);

        let bounding_box = self
            .space
            .outputs()
            .map(|output| self.space.output_geometry(output).unwrap())
            .reduce(|first, second| first.merge(second))
            .unwrap();
        if device.swapchain.is_some() {
            device
                .swapchain
                .as_mut()
                .unwrap()
                .resize(bounding_box.size.w as u32, bounding_box.size.h as u32);
        }
        self.flutter_engine()
            .send_window_metrics((bounding_box.size.w as u32, bounding_box.size.h as u32).into())
            .unwrap();

        self.determine_highest_hz_crtc();
        self.monitor_layout_changed();
        self.schedule_initial_render(node, crtc, self.loop_handle.clone());
    }

    fn connector_disconnected(
        &mut self,
        node: DrmNode,
        connector: connector::Info,
        crtc: crtc::Handle,
    ) {
        let device = if let Some(device) = self.backend_data.gpus.get_mut(&node) {
            device
        } else {
            return;
        };

        if let Some(pos) = device
            .non_desktop_connectors
            .iter()
            .position(|(handle, _)| *handle == connector.handle())
        {
            let _ = device.non_desktop_connectors.remove(pos);
        } else {
            device.surfaces.remove(&crtc);
        }

        let output = self
            .space
            .outputs()
            .find(|o| {
                o.user_data()
                    .get::<UdevOutputId>()
                    .map(|id| id.device_id == node && id.crtc == crtc)
                    .unwrap_or(false)
            })
            .cloned();

        if let Some(output) = output {
            self.space.unmap_output(&output);
        }

        let bounding_box = self
            .space
            .outputs()
            .map(|output| self.space.output_geometry(output).unwrap())
            .reduce(|first, second| first.merge(second))
            .unwrap_or(Rectangle::default());
        if device.swapchain.is_some() {
            device
                .swapchain
                .as_mut()
                .unwrap()
                .resize(bounding_box.size.w as u32, bounding_box.size.h as u32);
        }
        self.flutter_engine()
            .send_window_metrics((bounding_box.size.w as u32, bounding_box.size.h as u32).into())
            .unwrap();

        self.determine_highest_hz_crtc();
        self.monitor_layout_changed();
    }

    fn device_added(&mut self, node: DrmNode, path: &Path) -> Result<(), DeviceAddError> {
        info!("device_added");
        // Try to open the device
        let fd = self
            .backend_data
            .session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(DeviceAddError::DeviceOpen)?;

        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, notifier) =
            DrmDevice::new(fd.clone(), true).map_err(DeviceAddError::DrmDevice)?;

        let registration_token = self
            .loop_handle
            .insert_source(
                notifier,
                move |event, _metadata, data: &mut State<_>| match event {
                    DrmEvent::VBlank(crtc) => {
                        data.frame_finish(node, crtc);
                    }
                    DrmEvent::Error(error) => {
                        error!("{:?}", error);
                    }
                },
            )
            .unwrap();

        let gbm_device = GbmDevice::new(fd.clone()).map_err(DeviceAddError::GbmDevice)?;
        let egl_display =
            unsafe { EGLDisplay::new(gbm_device.clone()) }.expect("Failed to create EGLDisplay");

        let render_node = EGLDevice::device_for_display(&egl_display)
            .ok()
            .and_then(|x| x.try_get_render_node().ok().flatten())
            .unwrap_or(node);

        let renderer =
            unsafe { GlesRenderer::new(EGLContext::new(&egl_display).unwrap()) }.unwrap();

        /*         self.backend_data
        .gpu_manager
        .as_mut()
        .add_node(render_node, gbm_device.clone())
        .map_err(DeviceAddError::AddNode)?; */

        let gbm_allocator = GbmAllocator::new(
            gbm_device.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );

        let swapchain: Option<Swapchain<Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>>> =
            if render_node == self.backend_data.primary_gpu {
                let dmabuf_allocator: Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>> = {
                    let gbm_allocator =
                        GbmAllocator::new(gbm_device.clone(), GbmBufferFlags::RENDERING);
                    Box::new(DmabufAllocator(gbm_allocator))
                };
                let modifiers = renderer
                    .egl_context()
                    .dmabuf_texture_formats()
                    .iter()
                    .map(|format| format.modifier)
                    .collect::<Vec<_>>();
                Some(Swapchain::new(
                    dmabuf_allocator,
                    0,
                    0,
                    Fourcc::Argb8888,
                    modifiers,
                ))
            } else {
                None
            };

        self.backend_data.gpus.insert(
            node,
            GpuData {
                registration_token,
                gbm_device,
                gbm_allocator,
                drm_device: drm,
                drm_scanner: DrmScanner::new(),
                non_desktop_connectors: Vec::new(),
                render_node,
                surfaces: HashMap::new(),
                active_leases: Vec::new(),
                swapchain,
                current_slot: None,
                last_rendered_slot: None,
                renderer,
            },
        );

        Ok(())
    }

    fn device_changed(&mut self, node: DrmNode) {
        let device = if let Some(device) = self.backend_data.gpus.get_mut(&node) {
            device
        } else {
            return;
        };

        for event in device.drm_scanner.scan_connectors(&device.drm_device) {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_connected(node, connector, crtc),
                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => self.connector_disconnected(node, connector, crtc),
                _ => {}
            }
        }
    }

    fn frame_finish(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let gpu_data = match self.backend_data.gpus.get_mut(&node) {
            Some(gpu_data) => gpu_data,
            None => {
                error!("Trying to finish frame on non-existent gpu {:?}", node);
                return;
            }
        };

        let surface = match gpu_data.surfaces.get_mut(&crtc) {
            Some(surface) => surface,
            None => {
                error!("Trying to finish frame on non-existent crtc {:?}", crtc);
                return;
            }
        };

        let _output = if let Some(output) = self.space.outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: surface.device_id,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        let (mhz, highest_hz_crtc) = match self.backend_data.highest_hz_crtc {
            Some(highest_hz_crtc) => highest_hz_crtc,
            None => return,
        };

        if highest_hz_crtc != crtc {
            return;
        }

        let schedule_render = match surface
            .compositor
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into)
        {
            Ok(_user_data) => true,
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    // If the device has been deactivated do not reschedule, this will be done
                    // by session resume
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(
                            err.downcast_ref::<DrmError>(),
                            Some(&DrmError::DeviceInactive)
                        ) =>
                    {
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError {
                            source,
                            ..
                        })) if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => {
                        panic!("Rendering loop lost: {}", err)
                    }
                }
            }
        };
        if schedule_render {
            self.render(node, Some(crtc));
        }

        let drained: Vec<_> = self.batons.drain(..).collect(); // Mutable borrow ends here

        for baton in drained {
            self.flutter_engine().on_vsync(baton, mhz as u32).unwrap();
        }
        let start_time = std::time::Instant::now();
        for surface in self.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(
                surface.wl_surface(),
                start_time.elapsed().as_millis() as u32,
            );
        }
        for surface in self.xdg_popups.values() {
            send_frames_surface_tree(
                surface.wl_surface(),
                start_time.elapsed().as_millis() as u32,
            );
        }
        for surface in self.x11_surface_per_wl_surface.keys() {
            send_frames_surface_tree(surface, start_time.elapsed().as_millis() as u32);
        }
        let cursor_status = {
            let cursor_image_status = self.cursor_image_status.lock().unwrap();

            match *cursor_image_status {
                CursorImageStatus::Surface(ref surface) if !surface.alive() => {
                    CursorImageStatus::default_named()
                }
                ref status => status.clone(),
            }
        };

        if let CursorImageStatus::Surface(wl_surface) = cursor_status {
            send_frames_surface_tree(&wl_surface, start_time.elapsed().as_millis() as u32)
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>) {
        //let primary_gpu = self.backend_data.primary_gpu;

        let device_backend = match self.backend_data.gpus.get_mut(&node) {
            Some(backend) => backend,
            None => {
                error!("Trying to render on non-existent backend {}", node);
                return;
            }
        };

        if let Some(crtc) = crtc {
            self.render_surface(node, crtc);
        } else {
            let crtcs: Vec<_> = device_backend.surfaces.keys().copied().collect();
            for crtc in crtcs {
                self.render_surface(node, crtc);
            }
        };
    }

    // TODO: I don't think this method should be here.
    // It should probably be in GpuData or SurfaceData.
    pub fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let gpu_data = self.backend_data.gpus.get_mut(&node);
        let gpu_data = if let Some(gpu_data) = gpu_data {
            gpu_data
        } else {
            error!("Trying to render without gpu_data {}", node);
            return;
        };

        let surface = match gpu_data.surfaces.get_mut(&crtc) {
            Some(surface) => surface,
            None => return,
        };

        let _render_node = surface.render_node;
        /* let primary_gpu = self.backend_data.primary_gpu;
        let mut renderer = if primary_gpu == render_node {
            self.backend_data.gpu_manager.single_renderer(&render_node)
        } else {
            let format = surface.compositor.format();
            self.backend_data
                .gpu_manager
                .renderer(&primary_gpu, &render_node, format)
        }
        .unwrap(); */
        let renderer = &mut gpu_data.renderer;

        let last_rendered_slot = gpu_data.last_rendered_slot.as_mut();

        let slot = if let Some(ref slot) = last_rendered_slot {
            slot
        } else {
            // Flutter hasn't rendered anything yet. Render a solid color to schedule the next VBLANK.
            initial_render(surface, renderer).unwrap();
            return;
        };

        let output = self.space.outputs().find(|output| {
            output
                .user_data()
                .get::<UdevOutputId>()
                .map(|id| id.device_id == surface.device_id && id.crtc == surface.crtc)
                .unwrap_or(false)
        });

        let output = match output {
            Some(output) => output,
            None => return,
        };

        let geometry = match self.space.output_geometry(output) {
            Some(geometry) => geometry.to_f64(),
            None => return,
        };

        let elements = get_render_elements(
            renderer,
            output,
            slot,
            geometry,
            self.clock.now(),
            &self.cursor_image_status,
            &self.cursor_state,
            self.pointer.current_location(),
            self.surface_id_under_cursor.is_some(),
        );

        let rendered = surface
            .compositor
            .render_frame(renderer, &elements, [0.0, 0.0, 0.0, 0.0]);

        match rendered {
            Ok(_frame_result) => match surface.compositor.queue_frame(None) {
                Ok(()) => {}
                Err(err) => {
                    warn!("error queueing frame: {err}");
                    warn!("drm active: {:?}", gpu_data.drm_device.is_active());
                }
            },
            Err(err) => {
                eprintln!("Failed to render frame: {}", err);
            }
        }
    }

    fn schedule_initial_render(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        evt_handle: LoopHandle<'static, State<DrmBackend>>,
    ) {
        let device = if let Some(device) = self.backend_data.gpus.get_mut(&node) {
            device
        } else {
            return;
        };

        let surface = if let Some(surface) = device.surfaces.get_mut(&crtc) {
            surface
        } else {
            return;
        };

        let node = surface.render_node;
        let result = {
            let renderer = &mut device.renderer;
            initial_render(surface, renderer)
        };

        if let Err(err) = result {
            match err {
                SwapBuffersError::AlreadySwapped => {}
                SwapBuffersError::TemporaryFailure(err) => {
                    // TODO dont reschedule after 3(?) retries
                    warn!("Failed to submit page_flip: {}", err);
                    let handle = evt_handle.clone();
                    evt_handle
                        .insert_idle(move |data| data.schedule_initial_render(node, crtc, handle));
                }
                SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {}", err),
            }
        }
    }
}

#[allow(dead_code)]
struct SurfaceData {
    dh: DisplayHandle,
    device_id: DrmNode,
    crtc: crtc::Handle,
    render_node: DrmNode,
    global: Option<GlobalId>,
    compositor: GbmDrmCompositor,
}

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

fn get_mode_id_for_monitor_from_file(output_name: &str) -> Option<usize> {
    let path = std::env::var("XDG_CONFIG_HOME")
        .unwrap_or_else(|_| std::env::var("HOME").unwrap() + "/.config")
        + "/veshell/persistence/Monitor/"
        + output_name
        + ".json";

    info!("path: {}", path);

    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            error!("Failed to open file: {}", err);
            return None;
        }
    };
    let reader = std::io::BufReader::new(file);
    let json: serde_json::Value = match serde_json::from_reader(reader) {
        Ok(json) => json,
        Err(err) => {
            error!("Failed to parse JSON: {}", err);
            return None;
        }
    };
    info!("json: {:?}", json);
    let mode = json["selectedMode"].as_u64();

    match mode {
        Some(mode) => Some(mode as usize),
        None => {
            error!("selectedMode not found in JSON");
            None
        }
    }
}

fn initial_render<R>(surface: &mut SurfaceData, renderer: &mut R) -> Result<(), SwapBuffersError>
where
    R: Renderer
        + ImportAll
        + smithay::backend::renderer::Bind<smithay::backend::allocator::dmabuf::Dmabuf>,
    <R as Renderer>::TextureId: Clone + 'static,
{
    let render = surface
        .compositor
        .render_frame::<_, TextureRenderElement<_>>(renderer, &[], CLEAR_COLOR);
    if let Err(_err) = render {
        return Err(SwapBuffersError::TemporaryFailure(
            "Failed to render".into(),
        ));
    }

    surface.compositor.queue_frame(None)?;
    surface.compositor.reset_buffers();

    Ok(())
}
