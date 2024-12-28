use embedder::{
    FlutterKeyEvent, FlutterKeyEventType_kFlutterKeyEventTypeDown,
    FlutterKeyEventType_kFlutterKeyEventTypeRepeat, FlutterKeyEventType_kFlutterKeyEventTypeUp,
};
use log::debug;
use mouse_cursor::mouse_cursor_channel_method_call_handler;
use platform_channels::encodable_value::EncodableValue;
use platform_channels::standard_method_codec::StandardMethodCodec;
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_int, c_void, CString};
use std::mem::{size_of, MaybeUninit};
use std::ops::DerefMut;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::time::Duration;

use smithay::backend::renderer::gles::ffi::RGBA8;
use smithay::output::Output;
use smithay::reexports::calloop;
use smithay::reexports::calloop::channel::Event::Msg;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Dispatcher, LoopHandle};
use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        egl::{
            self,
            context::{GlAttributes, GlProfile, PixelFormatRequirements},
            EGLContext,
        },
        renderer::gles::ffi::Gles2,
    },
    reexports::calloop::channel,
    utils::{Physical, Size},
};

use crate::backend::Backend;
use crate::flutter_engine::callbacks::{
    gl_external_texture_frame_callback, platform_message_callback, populate_existing_damage,
    post_task_callback, runs_task_on_current_thread_callback, vsync_callback,
};
use crate::flutter_engine::embedder::{
    FlutterCustomTaskRunners, FlutterEngineAOTData, FlutterEngineAOTDataSource,
    FlutterEngineAOTDataSourceType_kFlutterEngineAOTDataSourceTypeElfPath,
    FlutterEngineAOTDataSource__bindgen_ty_1, FlutterEngineCreateAOTData, FlutterEngineInitialize,
    FlutterEngineMarkExternalTextureFrameAvailable, FlutterEngineRegisterExternalTexture,
    FlutterEngineRunInitialized, FlutterEngineRunTask, FlutterEngineSendKeyEvent,
    FlutterEngineSendPointerEvent, FlutterPointerEvent, FlutterTaskRunnerDescription,
};
use crate::flutter_engine::platform_channel_callbacks::platform_channel_method_handler;
use crate::flutter_engine::platform_channels::basic_message_channel::BasicMessageChannel;
use crate::flutter_engine::platform_channels::binary_messenger_impl::BinaryMessengerImpl;
use crate::flutter_engine::platform_channels::json_message_codec::JsonMessageCodec;
use crate::flutter_engine::platform_channels::json_method_codec::JsonMethodCodec;
use crate::flutter_engine::platform_channels::message_codec::MessageCodec;
use crate::flutter_engine::platform_channels::method_call::MethodCall;
use crate::flutter_engine::platform_channels::method_channel::MethodChannel;
use crate::flutter_engine::platform_channels::method_result::MethodResult;
use crate::flutter_engine::task_runner::TaskRunner;
use crate::flutter_engine::text_input::{text_input_channel_method_call_handler, TextInput};
use crate::flutter_engine::wayland_messages::{EnvironmentVariables, MonitorsMessage, MyOutput};
use crate::gles_framebuffer_importer::GlesFramebufferImporter;
use crate::keyboard::key_mapping::{GTK_KEYVAL_TO_LOGICAL_KEY_MAP, XKB_TO_PHYSICAL_KEY_MAP};
use crate::keyboard::{post_flutter_handle_key_event, VeshellKeyEvent};
use crate::mouse_button_tracker::MouseButtonTracker;
use crate::{
    flutter_engine::{
        callbacks::{
            clear_current, fbo_callback, make_current, make_resource_current, present_with_info,
            surface_transformation,
        },
        embedder::{
            FlutterEngine as FlutterEngineHandle, FlutterEngineGetCurrentTime,
            FlutterEngineOnVsync, FlutterEngineSendWindowMetricsEvent, FlutterEngineShutdown,
            FlutterOpenGLRendererConfig, FlutterProjectArgs, FlutterRendererConfig,
            FlutterRendererConfig__bindgen_ty_1, FlutterWindowMetricsEvent, FLUTTER_ENGINE_VERSION,
        },
    },
    State,
};

use smithay::backend::input::KeyState;

mod callbacks;
pub mod embedder;
mod mouse_cursor;
pub mod platform_channel_callbacks;
pub mod platform_channels;
pub mod task_runner;
mod text_input;
pub mod wayland_messages;

/// Wrap the handle for various safety reasons:
/// - Clone & Copy is willingly not implemented to avoid using the engine after being shut down.
/// - Send is not implemented because all its methods must be called from the thread the engine was created.
pub struct FlutterEngine<BackendData: Backend + 'static> {
    loop_handle: LoopHandle<'static, State<BackendData>>,
    pub handle: FlutterEngineHandle,
    data: FlutterEngineData,
    pub task_runner: TaskRunner,
    current_thread_id: std::thread::ThreadId,
    pub(crate) mouse_button_tracker: MouseButtonTracker,
    pub binary_messenger: Rc<RefCell<BinaryMessengerImpl>>,
    pub platform_method_channel: MethodChannel<serde_json::Value>,
    pub key_event_channel: BasicMessageChannel<serde_json::Value>,
    pub text_input: TextInput,
    rx_request_external_texture_name_registration_token: calloop::RegistrationToken,
}

/// I don't want people to clone it because it's UB to call [FlutterEngine::on_vsync] multiple times
/// with the same baton, which will most probably segfault.
pub struct Baton(isize);

impl<BackendData: Backend + 'static> FlutterEngine<BackendData> {
    pub fn new(
        server_state: &mut State<BackendData>,
    ) -> Result<(Box<Self>, EmbedderChannels), Box<dyn std::error::Error>> {
        let (tx_present, rx_present) = channel::channel::<()>();
        let (tx_request_fbo, rx_request_fbo) = channel::channel::<()>();
        let (tx_fbo, rx_fbo) = channel::channel::<Option<Dmabuf>>();
        let (tx_output_height, rx_output_height) = channel::channel::<u16>();
        let (tx_baton, rx_baton) = channel::channel::<Baton>();
        let (tx_reschedule_task_runner_timer, rx_reschedule_task_runner_timer) =
            channel::channel::<Duration>();
        let (tx_request_external_texture_name, rx_request_external_texture_name) =
            channel::channel::<i64>();
        let (tx_external_texture_name, rx_external_texture_name) = channel::channel::<(u32, u32)>();

        let (tx_flutter_handled_key_event, rx_flutter_handled_key_event) =
            channel::channel::<(VeshellKeyEvent, bool)>();
        let flutter_engine_channels = FlutterEngineChannels {
            tx_present,
            tx_request_fbo,
            rx_fbo,
            rx_output_height,
            tx_baton,
            tx_request_external_texture_name,
            rx_external_texture_name,
            tx_flutter_handled_key_event,
        };

        let embedder_channels = EmbedderChannels {
            rx_present,
            rx_request_fbo,
            tx_fbo,
            tx_output_height,
            rx_baton,
        };

        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            panic!("Unsupported architecture");
        };

        // Default build is debug if not specified.
        let flutter_engine_build = option_env!("FLUTTER_ENGINE_BUILD").unwrap_or("debug");

        let executable_path = std::fs::canonicalize("/proc/self/exe")?;
        let bundle_root = if option_env!("BUNDLE").is_some() {
            executable_path.parent().unwrap().display().to_string()
        } else {
            format!("../shell/build/linux/{arch}/{flutter_engine_build}/bundle")
        };

        let host = "127.0.0.1";
        let socket_number: i32 = server_state
            .wayland_socket_name
            .as_deref()
            .and_then(|s| s.split('-').last())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        debug!("ECHO SOCKET NAME {:?}", socket_number);
        let port = 12345 + socket_number;
        propagate_vm_service(host, port)?;
        let assets_path = CString::new(format!("{bundle_root}/data/flutter_assets"))?;
        let icu_data_path = CString::new(format!("{bundle_root}/data/icudtl.dat"))?;
        let executable_path = CString::new(executable_path.as_os_str().as_bytes())?;
        let observatory_host = CString::new(format!("--vm-service-host={}", host))?;
        let observatory_port = CString::new(format!("--vm-service-port={}", port))?;
        let disable_service_auth_codes = CString::new("--disable-service-auth-codes")?;

        let command_line_argv = [
            executable_path.as_ptr(),
            observatory_host.as_ptr(),
            observatory_port.as_ptr(),
            disable_service_auth_codes.as_ptr(),
        ];

        let elf_path = CString::new(format!("{bundle_root}/lib/libapp.so"))?;
        let mut aot_data: FlutterEngineAOTData = null_mut();

        if flutter_engine_build != "debug" {
            let aot_data_source = FlutterEngineAOTDataSource {
                type_: FlutterEngineAOTDataSourceType_kFlutterEngineAOTDataSourceTypeElfPath,
                __bindgen_anon_1: FlutterEngineAOTDataSource__bindgen_ty_1 {
                    elf_path: elf_path.as_ptr(),
                },
            };
            let result = unsafe {
                FlutterEngineCreateAOTData(&aot_data_source as *const _, &mut aot_data as *mut _)
            };
            if result != 0 {
                return Err(format!("Could not create AOT data, error {result}").into());
            }
        }

        server_state
            .backend_data
            .with_primary_renderer(|renderer| {
                let root_egl_context = renderer.egl_context();

                // We need a pointer to the memory location before initializing the struct.
                let mut this = Box::new(MaybeUninit::<Self>::uninit());

                let task_runner_description = FlutterTaskRunnerDescription {
                    struct_size: size_of::<FlutterTaskRunnerDescription>(),
                    user_data: this.deref_mut() as *const _ as *mut _,
                    runs_task_on_current_thread_callback: Some(
                        runs_task_on_current_thread_callback::<BackendData>,
                    ),
                    post_task_callback: Some(post_task_callback::<BackendData>),
                    identifier: 1,
                };

                let task_runners = FlutterCustomTaskRunners {
                    struct_size: size_of::<FlutterCustomTaskRunners>(),
                    platform_task_runner: &task_runner_description,
                    render_task_runner: null(),
                    thread_priority_setter: None,
                };

                let project_args = FlutterProjectArgs {
                    struct_size: size_of::<FlutterProjectArgs>(),
                    assets_path: assets_path.as_ptr(),
                    main_path__unused__: null(),
                    packages_path__unused__: null(),
                    icu_data_path: icu_data_path.as_ptr(),
                    command_line_argc: command_line_argv.len() as c_int,
                    command_line_argv: command_line_argv.as_ptr(),
                    platform_message_callback: Some(platform_message_callback::<BackendData>),
                    vm_snapshot_data: null(),
                    vm_snapshot_data_size: 0,
                    vm_snapshot_instructions: null(),
                    vm_snapshot_instructions_size: 0,
                    isolate_snapshot_data: null(),
                    isolate_snapshot_data_size: 0,
                    isolate_snapshot_instructions: null(),
                    isolate_snapshot_instructions_size: 0,
                    root_isolate_create_callback: None,
                    update_semantics_node_callback: None,
                    update_semantics_custom_action_callback: None,
                    persistent_cache_path: null(),
                    is_persistent_cache_read_only: false,
                    vsync_callback: Some(vsync_callback::<BackendData>),
                    custom_dart_entrypoint: null(),
                    custom_task_runners: &task_runners,
                    shutdown_dart_vm_when_done: true,
                    compositor: null(),
                    dart_old_gen_heap_size: 0,
                    aot_data,
                    compute_platform_resolved_locale_callback: None,
                    dart_entrypoint_argc: 0,
                    dart_entrypoint_argv: null(),
                    log_message_callback: Some(log_callback),
                    log_tag: null(),
                    on_pre_engine_restart_callback: None,
                    update_semantics_callback: None,
                    update_semantics_callback2: None,
                    channel_update_callback: None,
                };

                let renderer_config = FlutterRendererConfig {
                    type_: 0,
                    __bindgen_anon_1: FlutterRendererConfig__bindgen_ty_1 {
                        open_gl: FlutterOpenGLRendererConfig {
                            struct_size: size_of::<FlutterOpenGLRendererConfig>(),
                            make_current: Some(make_current::<BackendData>),
                            clear_current: Some(clear_current::<BackendData>),
                            present: None,
                            fbo_callback: Some(fbo_callback::<BackendData>),
                            make_resource_current: Some(make_resource_current::<BackendData>),
                            // Flutter must request another framebuffer every frame
                            // because we're using a triple-buffered swapchain.
                            fbo_reset_after_present: true,
                            surface_transformation: Some(surface_transformation::<BackendData>),
                            gl_proc_resolver: None,
                            gl_external_texture_frame_callback: Some(
                                gl_external_texture_frame_callback::<BackendData>,
                            ),
                            fbo_with_frame_info_callback: None,
                            present_with_info: Some(present_with_info::<BackendData>),
                            populate_existing_damage: Some(populate_existing_damage::<BackendData>),
                        },
                    },
                };

                let mut flutter_engine: FlutterEngineHandle = null_mut();
                let result = unsafe {
                    FlutterEngineInitialize(
                        FLUTTER_ENGINE_VERSION as usize,
                        &renderer_config,
                        &project_args,
                        this.deref_mut() as *const _ as *mut _,
                        &mut flutter_engine,
                    )
                };
                if result != 0 {
                    return Err(
                        format!("Could not initalize the Flutter engine, error {result}").into(),
                    );
                }

                let binary_messenger =
                    Rc::new(RefCell::new(BinaryMessengerImpl::new(flutter_engine)));

                let method_codec = Rc::new(JsonMethodCodec::new());
                let mut platform_method_channel = MethodChannel::<serde_json::Value>::new(
                    binary_messenger.clone(),
                    "platform".to_string(),
                    method_codec.clone(),
                );

                let (tx_platform_message, rx_platform_message) = channel::channel::<(
                    MethodCall<serde_json::Value>,
                    Box<dyn MethodResult<serde_json::Value>>,
                )>();
                platform_method_channel.set_method_call_mpsc_channel(Some(tx_platform_message));

                server_state
                    .loop_handle
                    .insert_source(rx_platform_message, platform_channel_method_handler)
                    .unwrap();

                let codec = Rc::new(JsonMessageCodec::new());
                let key_event_channel = BasicMessageChannel::<serde_json::Value>::new(
                    binary_messenger.clone(),
                    "flutter/keyevent".to_string(),
                    codec,
                );

                let mut text_input_channel = MethodChannel::<serde_json::Value>::new(
                    binary_messenger.clone(),
                    "flutter/textinput".to_string(),
                    method_codec.clone(),
                );

                let (tx_text_input_message, rx_text_input_message) = channel::channel::<(
                    MethodCall<serde_json::Value>,
                    Box<dyn MethodResult<serde_json::Value>>,
                )>();

                text_input_channel.set_method_call_mpsc_channel(Some(tx_text_input_message));

                let codec = Rc::new(StandardMethodCodec::new());
                server_state
                    .loop_handle
                    .insert_source(
                        rx_text_input_message,
                        text_input_channel_method_call_handler,
                    )
                    .unwrap();

                let mut mouse_cursor_channel = MethodChannel::<EncodableValue>::new(
                    binary_messenger.clone(),
                    "flutter/mousecursor".to_string(),
                    codec,
                );

                let (tx_mouse_cursor_message, rx_mouse_cursor_message) = channel::channel::<(
                    MethodCall<EncodableValue>,
                    Box<dyn MethodResult<EncodableValue>>,
                )>();

                mouse_cursor_channel.set_method_call_mpsc_channel(Some(tx_mouse_cursor_message));

                server_state
                    .loop_handle
                    .insert_source(
                        rx_mouse_cursor_message,
                        mouse_cursor_channel_method_call_handler,
                    )
                    .unwrap();

                server_state
                    .loop_handle
                    .insert_source(rx_flutter_handled_key_event, |event, _, data| {
                        if let Msg((veshell_event, handled)) = event {
                            post_flutter_handle_key_event::<BackendData>(
                                data,
                                veshell_event,
                                handled,
                            );
                        }
                    })
                    .unwrap();

                let task_runner_timer_dispatcher = Dispatcher::new(
                    Timer::immediate(),
                    move |_, _, data: &mut State<BackendData>| {
                        let duration = data.flutter_engine_mut().task_runner.execute_expired_tasks(
                            move |task| {
                                unsafe { FlutterEngineRunTask(flutter_engine, task as *const _) };
                            },
                        );
                        TimeoutAction::ToDuration(duration)
                    },
                );
                let task_runner_timer_registration_token = server_state
                    .loop_handle
                    .register_dispatcher(task_runner_timer_dispatcher.clone())
                    .unwrap();

                server_state
                    .loop_handle
                    .insert_source(
                        rx_reschedule_task_runner_timer,
                        move |event, _, data: &mut State<BackendData>| {
                            if let Msg(duration) = event {
                                task_runner_timer_dispatcher
                                    .as_source_mut()
                                    .set_duration(duration);
                                data.loop_handle
                                    .update(&task_runner_timer_registration_token)
                                    .unwrap();
                            }
                        },
                    )
                    .unwrap();

                let rx_request_external_texture_name_registration_token = server_state
                    .loop_handle
                    .insert_source(rx_request_external_texture_name, move |event, _, data| {
                        if let Msg(texture_id) = event {
                            let texture_swap_chain = data.texture_swapchains.get_mut(&texture_id);
                            let texture_id = match texture_swap_chain {
                                Some(texture) => {
                                    let texture = texture.start_read();
                                    texture.tex_id()
                                }
                                None => 0,
                            };
                            let _ = tx_external_texture_name.send((texture_id, RGBA8));
                        }
                    })
                    .unwrap();

                this.write(Self {
                    loop_handle: server_state.loop_handle.clone(),
                    handle: flutter_engine,
                    data: FlutterEngineData::new(root_egl_context, flutter_engine_channels)?,
                    task_runner: TaskRunner::new(tx_reschedule_task_runner_timer),
                    current_thread_id: std::thread::current().id(),
                    mouse_button_tracker: MouseButtonTracker::new(),
                    binary_messenger: binary_messenger.clone(),
                    platform_method_channel,
                    key_event_channel,
                    text_input: TextInput::new(text_input_channel),
                    rx_request_external_texture_name_registration_token,
                });

                // TODO: Delete this function once Box::assume_init gets stabilized.
                pub unsafe fn assume_init<T>(var: Box<MaybeUninit<T>>) -> Box<T> {
                    let raw = Box::into_raw(var);
                    unsafe { Box::from_raw(raw as *mut T) }
                }
                let this = unsafe { assume_init(this) };

                let result = unsafe { FlutterEngineRunInitialized(flutter_engine) };
                if result != 0 {
                    return Err(format!("Could not run the Flutter engine, error {result}").into());
                }

                Ok((this, embedder_channels))
            })
            .unwrap()
    }

    pub fn current_time_ns() -> u64 {
        unsafe { FlutterEngineGetCurrentTime() }
    }

    pub fn current_time_us() -> u64 {
        unsafe { FlutterEngineGetCurrentTime() / 1000 }
    }

    pub fn send_window_metrics(
        &self,
        size: Size<u32, Physical>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let event = FlutterWindowMetricsEvent {
            struct_size: size_of::<FlutterWindowMetricsEvent>(),
            width: size.w as usize,
            height: size.h as usize,
            pixel_ratio: 1.0,
            left: 0,
            top: 0,
            physical_view_inset_top: 0.0,
            physical_view_inset_right: 0.0,
            physical_view_inset_bottom: 0.0,
            physical_view_inset_left: 0.0,
            display_id: 0,
        };

        let result =
            unsafe { FlutterEngineSendWindowMetricsEvent(self.handle, &event as *const _) };
        if result != 0 {
            return Err(format!("Could not send window metrics event, error {result}").into());
        }
        Ok(())
    }

    /// mhz == millihertz
    pub fn on_vsync(&self, baton: Baton, mhz: u32) -> Result<(), Box<dyn std::error::Error>> {
        let now = unsafe { FlutterEngineGetCurrentTime() };
        let next_frame = now + ((1_000_000.0 / mhz as f64) * 1_000_000.0) as u64;
        let result = unsafe { FlutterEngineOnVsync(self.handle, baton.0, now, next_frame) };
        if result != 0 {
            return Err(format!("Could not send vsync baton, error {result}").into());
        }
        Ok(())
    }

    pub fn send_pointer_event(
        &self,
        event: FlutterPointerEvent,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let result = unsafe { FlutterEngineSendPointerEvent(self.handle, &event as *const _, 1) };
        if result != 0 {
            return Err(format!("Could not send pointer event, error {result}").into());
        }
        Ok(())
    }

    pub fn send_key_event(
        &self,
        event: VeshellKeyEvent,
        repeat: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let timestamp = unsafe { FlutterEngineGetCurrentTime() / 1000 };
        let event_type = if repeat {
            FlutterKeyEventType_kFlutterKeyEventTypeRepeat
        } else {
            match event.state {
                KeyState::Pressed => FlutterKeyEventType_kFlutterKeyEventTypeDown,
                KeyState::Released => FlutterKeyEventType_kFlutterKeyEventTypeUp,
            }
        };

        let physical_key = match XKB_TO_PHYSICAL_KEY_MAP.get(&(event.key_code.raw() as u64)) {
            Some(&key) => key,
            None => {
                return Err(format!("Unsupported keysym: {:?}", event.key_code.raw()).into());
            }
        };

        if event.raw_keysym.is_some() {
            println!(
                "The raw value for raw_keysym is 0x{:x}",
                event.raw_keysym.unwrap().raw()
            );
        }
        println!(
            "The raw value for keysym is 0x{:x}",
            event.keysym.raw() as u64
        );

        let logical_key =
            match GTK_KEYVAL_TO_LOGICAL_KEY_MAP.get(&(event.raw_keysym.unwrap().raw() as u64)) {
                Some(&key) => key,
                None => event.raw_keysym.unwrap().raw() as u64,
            };

        let character = if event.state == KeyState::Released || event.keysym.key_char().is_none() {
            std::ptr::null()
        } else {
            let c_str = CString::new(event.keysym.key_char().unwrap().to_string())
                .expect("CString::new failed");
            c_str.as_ptr()
        };

        /* let key_event_data = Box::new(FlutterKeyEventData {
            key_event: event,
            tx_flutter_handled_key_event: self.data.channels.tx_flutter_handled_key_event.clone(),
        });
        let user_data = Box::into_raw(key_event_data) as *mut c_void; */

        let flutter_key_event = FlutterKeyEvent {
            struct_size: std::mem::size_of::<FlutterKeyEvent>(),
            timestamp: timestamp as f64,
            type_: event_type,
            physical: physical_key,
            logical: logical_key,
            character,
            synthesized: false,
        };

        let result = unsafe {
            FlutterEngineSendKeyEvent(
                self.handle,
                &flutter_key_event as *const _,
                None,                  //Some(key_event_callback::<BackendData>),
                core::ptr::null_mut(), //user_data,
            )
        };

        if result != 0 {
            return Err(format!("Could not send key event, error {result}").into());
        }
        let tx = self.data.channels.tx_flutter_handled_key_event.clone();
        // Send fake key event since we don't care about supporting legacy keyboard implementation
        self.key_event_channel.send(
            &json!({
                "keymap": "linux",
                "toolkit": "gtk",
                "keyCode": physical_key,
                "specifiedLogicalKey": 0,
                "scanCode":event.key_code.raw(),
                "modifiers": 0,
                "unicodeScalarValues": event.keysym.key_char().map(|c| c as u32),
                "type": match event.state {
                    KeyState::Pressed => "keydown",
                    KeyState::Released => "keyup",
                },
            }),
            Some(Box::new(move |response: Option<&[u8]>| {
                // This is the callback that will be called when Flutter replies.
                // Flutter always replies with a single `handled` boolean.
                // If its value is true, some widget listening to keyboard shortcuts probably handled this event.
                let response = match response {
                    Some(response) => response,
                    None => return,
                };

                let message = JsonMessageCodec::new().decode_message(response).unwrap();
                let handled = message["handled"].as_bool().unwrap();
                debug!("handled {handled}");
                tx.send((event, handled)).unwrap();
            })),
        );
        Ok(())
    }

    pub fn register_external_texture(
        &self,
        texture_id: i64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let result = unsafe { FlutterEngineRegisterExternalTexture(self.handle, texture_id) };
        if result != 0 {
            return Err(format!("Could not register external texture, error {result}").into());
        }
        Ok(())
    }

    pub fn mark_external_texture_frame_available(
        &self,
        texture_id: i64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let result =
            unsafe { FlutterEngineMarkExternalTextureFrameAvailable(self.handle, texture_id) };
        if result != 0 {
            return Err(
                format!("Could not mark external texture frame available, error {result}").into(),
            );
        }
        Ok(())
    }

    pub fn monitor_layout_changed(&mut self, outputs: Vec<Output>) {
        self.platform_method_channel.invoke_method(
            "monitor_layout_changed",
            Some(Box::new(json!(MonitorsMessage {
                monitors: outputs.into_iter().map(MyOutput).collect()
            }))),
            None,
        );
    }

    pub fn set_environment_variable(&mut self, key: &str, value: Option<&str>) {
        self.set_environment_variables(HashMap::from([(key, value)]));
    }

    pub fn set_environment_variables(
        &mut self,
        environment_variables: HashMap<&str, Option<&str>>,
    ) {
        self.platform_method_channel.invoke_method(
            "set_environment_variables",
            Some(Box::new(json!(EnvironmentVariables {
                environment_variables,
            }))),
            None,
        );
    }
}

impl<BackendData: Backend + 'static> Drop for FlutterEngine<BackendData> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // Avoid indefinite hang in the Flutter render thread waiting for an external texture.
            let token = self.rx_request_external_texture_name_registration_token;
            self.loop_handle.remove(token);
            let _ = unsafe { FlutterEngineShutdown(self.handle) };
        }
    }
}

struct FlutterEngineData {
    gl: Gles2,
    main_egl_context: EGLContext,
    resource_egl_context: EGLContext,
    output_height: Option<u16>,
    channels: FlutterEngineChannels,
    framebuffer_importer: GlesFramebufferImporter,
}

// Ironically, EGLContext which contains EGLDisplay is Send, but EGLDisplay is not.
// This impl is not needed, but it's here to make it explicit that it's safe to send this struct
// to the Flutter render thread.
unsafe impl Send for FlutterEngineData {}

impl FlutterEngineData {
    fn new(
        root_egl_context: &EGLContext,
        channels: FlutterEngineChannels,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe { root_egl_context.make_current()? };

        let egl_display = root_egl_context.display();
        let gl_attributes = GlAttributes {
            version: (2, 0),
            profile: Some(GlProfile::Core),
            debug: false,
            vsync: false,
        };
        let pixel_format_requirements = PixelFormatRequirements::_8_bit();

        Ok(Self {
            gl: Gles2::load_with(|s| unsafe { egl::get_proc_address(s) } as *const _),
            main_egl_context: EGLContext::new_shared_with_config(
                egl_display,
                root_egl_context,
                gl_attributes,
                pixel_format_requirements,
            )?,
            resource_egl_context: EGLContext::new_shared_with_config(
                egl_display,
                root_egl_context,
                gl_attributes,
                pixel_format_requirements,
            )?,
            output_height: None,
            channels,
            framebuffer_importer: unsafe { GlesFramebufferImporter::new(egl_display.clone())? },
        })
    }
}

pub struct FlutterEngineChannels {
    tx_present: channel::Sender<()>,
    tx_request_fbo: channel::Sender<()>,
    rx_fbo: channel::Channel<Option<Dmabuf>>,
    rx_output_height: channel::Channel<u16>,
    tx_baton: channel::Sender<Baton>,
    tx_request_external_texture_name: channel::Sender<i64>,
    rx_external_texture_name: channel::Channel<(u32, u32)>,
    tx_flutter_handled_key_event: channel::Sender<(VeshellKeyEvent, bool)>,
}

pub struct EmbedderChannels {
    pub rx_present: channel::Channel<()>,
    pub rx_request_fbo: channel::Channel<()>,
    pub tx_fbo: channel::Sender<Option<Dmabuf>>,
    pub tx_output_height: channel::Sender<u16>,
    pub rx_baton: channel::Channel<Baton>,
}

use std::fs::{self, File};
use std::io::Write;

/// We both save a vmService.json file and set and Env variable to be either be able to connect inside running shell or in child shell
fn propagate_vm_service(host: &str, port: i32) -> Result<(), Box<dyn std::error::Error>> {
    std::env::set_var(
        "VESHELL_VM_SERVICE_URL",
        format!(r#"http://{host}:{port}/"#),
    );
    if std::env::var("VESHELL_VSCODE_DEBUG").is_ok() {
        let vm_service = json!({
            "uri": format!(r#"http://{host}:{port}/"#),
        });

        let path = "../.temp/vmService.json";
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        file.write_all(vm_service.to_string().as_bytes())?;
    }

    Ok(())
}

unsafe extern "C" fn log_callback(_: *const i8, message: *const i8, _: *mut c_void) {
    //let tag = std::ffi::CStr::from_ptr(tag).to_string_lossy().into_owned();
    let message = std::ffi::CStr::from_ptr(message)
        .to_string_lossy()
        .into_owned();
    tracing::debug!("{}", message);
}
