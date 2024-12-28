use std::ffi::c_void;
use std::ptr::null_mut;

use smithay::backend::renderer::gles::ffi;
use smithay::reexports::calloop::channel;
use tracing::error;

use crate::backend::Backend;
use crate::flutter_engine::embedder::{
    FlutterDamage, FlutterOpenGLTexture, FlutterPlatformMessage, FlutterPresentInfo, FlutterRect,
    FlutterTask, FlutterTransformation,
};
use crate::flutter_engine::platform_channels::binary_messenger::BinaryMessenger;
use crate::flutter_engine::{Baton, FlutterEngine};
use crate::keyboard::VeshellKeyEvent;

pub unsafe extern "C" fn make_current<BackendData>(user_data: *mut c_void) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    match flutter_engine.data.main_egl_context.make_current() {
        Ok(()) => true,
        Err(err) => {
            error!("{}", err);
            false
        }
    }
}

pub unsafe extern "C" fn make_resource_current<BackendData>(user_data: *mut c_void) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    match flutter_engine.data.resource_egl_context.make_current() {
        Ok(()) => true,
        Err(err) => {
            error!("{}", err);
            false
        }
    }
}

pub unsafe extern "C" fn clear_current<BackendData>(user_data: *mut c_void) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    match flutter_engine.data.main_egl_context.unbind() {
        Ok(()) => true,
        Err(err) => {
            error!("{}", err);
            false
        }
    }
}

pub unsafe extern "C" fn fbo_callback<BackendData>(user_data: *mut c_void) -> u32
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    if flutter_engine
        .data
        .channels
        .tx_request_fbo
        .send(())
        .is_err()
    {
        return 0;
    }
    if let Ok(Some(dmabuf)) = flutter_engine.data.channels.rx_fbo.recv() {
        flutter_engine
            .data
            .framebuffer_importer
            .import_framebuffer(&flutter_engine.data.main_egl_context, dmabuf)
            .unwrap_or(0)
    } else {
        0
    }
}

pub unsafe extern "C" fn present_with_info<BackendData>(
    user_data: *mut c_void,
    _frame_present_info: *const FlutterPresentInfo,
) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    flutter_engine.data.gl.Finish();
    flutter_engine.data.channels.tx_present.send(()).is_ok()
}

pub unsafe extern "C" fn populate_existing_damage<BackendData>(
    _user_data: *mut c_void,
    _fbo_id: isize,
    existing_damage: *mut FlutterDamage,
) where
    BackendData: Backend + 'static,
{
    let existing_damage = &mut *existing_damage;
    existing_damage.struct_size = std::mem::size_of::<FlutterDamage>();
    existing_damage.num_rects = 1;

    // TODO: Implement proper damage tracking.
    // The Flutter engine docs says that if this callback is not implemented,
    // it will repaint the entire screen every frame, but it's false.
    // It's probably a bug in the Flutter engine, but until I implement proper damage tracking
    // we manually set the damage to the entire screen.
    static FLUTTER_RECT: FlutterRect = FlutterRect {
        left: 0.0,
        top: 0.0,
        right: 10000.0,
        bottom: 10000.0,
    };

    existing_damage.damage = &FLUTTER_RECT as *const _ as *mut _;
}

pub unsafe extern "C" fn surface_transformation<BackendData>(
    user_data: *mut c_void,
) -> FlutterTransformation
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);

    while let Ok(output_height) = flutter_engine.data.channels.rx_output_height.try_recv() {
        flutter_engine.data.output_height = Some(output_height);
    }

    match flutter_engine.data.output_height {
        Some(output_height) => FlutterTransformation {
            scaleX: 1.0,
            skewX: 0.0,
            transX: 0.0,
            skewY: 0.0,
            scaleY: -1.0,
            transY: output_height as f64,
            pers0: 0.0,
            pers1: 0.0,
            pers2: 1.0,
        },
        None => FlutterTransformation {
            scaleX: 1.0,
            skewX: 0.0,
            transX: 0.0,
            skewY: 0.0,
            scaleY: 1.0,
            transY: 0.0,
            pers0: 0.0,
            pers1: 0.0,
            pers2: 1.0,
        },
    }
}

pub unsafe extern "C" fn vsync_callback<BackendData>(
    user_data: *mut std::os::raw::c_void,
    baton: isize,
) where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    let _ = flutter_engine.data.channels.tx_baton.send(Baton(baton));
}

pub unsafe extern "C" fn runs_task_on_current_thread_callback<BackendData>(
    user_data: *mut c_void,
) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    flutter_engine.current_thread_id == std::thread::current().id()
}

pub unsafe extern "C" fn post_task_callback<BackendData>(
    task: FlutterTask,
    target_time: u64,
    user_data: *mut c_void,
) where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    let timeout = flutter_engine.task_runner.enqueue_task(task, target_time);
    flutter_engine
        .task_runner
        .reschedule_timer
        .send(timeout)
        .unwrap();
}

pub unsafe extern "C" fn platform_message_callback<BackendData>(
    message: *const FlutterPlatformMessage,
    user_data: *mut c_void,
) where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    let message = &*message;
    flutter_engine
        .binary_messenger
        .borrow_mut()
        .handle_message(message);
}

pub unsafe extern "C" fn gl_external_texture_frame_callback<BackendData>(
    user_data: *mut c_void,
    texture_id: i64,
    _width: usize,
    _height: usize,
    texture_out: *mut FlutterOpenGLTexture,
) -> bool
where
    BackendData: Backend + 'static,
{
    let flutter_engine = &mut *(user_data as *mut FlutterEngine<BackendData>);
    let channels = &mut flutter_engine.data.channels;

    let (texture_name, _) = channels
        .tx_request_external_texture_name
        .send(texture_id)
        .ok()
        .and_then(|()| channels.rx_external_texture_name.recv().ok())
        .unwrap_or((0, ffi::RGBA8));

    let texture_out = &mut *texture_out;

    // TODO: Don't hardcode the target.
    // If the texture is imported from a DMABUF, I think it the target should be GL_TEXTURE_EXTERNAL_OES.
    texture_out.target = ffi::TEXTURE_2D;
    texture_out.name = texture_name;
    // TODO: Should probably not hardcode the format in case the texture is not RGBA8.
    // I can't just assign the format obtained from the channel because it's BGRA_EXT and for some
    // reason it doesn't work while RGBA8 does.
    texture_out.format = ffi::RGBA8;
    texture_out.user_data = null_mut();
    texture_out.destruction_callback = None;
    texture_out.width = 0;
    texture_out.height = 0;

    texture_name != 0
}

pub struct FlutterKeyEventData {
    pub key_event: VeshellKeyEvent,
    pub tx_flutter_handled_key_event: channel::Sender<(VeshellKeyEvent, bool)>,
}
/// It's currently not used because handled is always false it's seems that we will need it when the deprecation of the legacy keyboard handled will be removed we currently listen to the [key_event_channel.send] callback
pub unsafe extern "C" fn key_event_callback<BackendData>(handled: bool, user_data: *mut c_void)
where
    BackendData: Backend + 'static,
{
    let data = &mut *(user_data as *mut FlutterKeyEventData);

    data.tx_flutter_handled_key_event
        .send((data.key_event, handled))
        .unwrap();
}
