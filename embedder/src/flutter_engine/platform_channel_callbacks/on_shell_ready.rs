use std::borrow::Borrow;

use crate::flutter_engine::platform_channels::method_result::MethodResult;

use crate::flutter_engine::platform_channels::method_call::MethodCall;

use crate::backend::Backend;
use crate::flutter_engine::wayland_messages::NewX11Surface;
use crate::wayland::wayland::get_surface_id;
use serde_json::json;
use smithay::reexports::wayland_server::Resource;

use crate::state::State;

// For development purpose the shell can reload to apply current changes
// In order to be the most transparent for the dev
// We resend all existing surfaces to it so it can benefit from the Persistence
pub fn on_shell_ready<BackendData: Backend + 'static>(
    _method_call: MethodCall<serde_json::Value>,
    mut result: Box<dyn MethodResult<serde_json::Value>>,
    data: &mut State<BackendData>,
) {
    let surfaces = data.surfaces.clone();

    tracing::info!("on_shell_ready");
    // Send new_surface for all existing surface
    for surface_id in surfaces.keys() {
        let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_surface",
            Some(Box::new(json!({
                "surfaceId": surface_id,
            }))),
            None,
        );
    }

    let toplevels = data.xdg_toplevels.clone();
    for (surface_id, surface) in toplevels.iter() {
        let pid = {
            let client = surface.wl_surface().client().unwrap();

            let credentials = client.get_credentials(&data.display_handle).unwrap();

            credentials.pid
        };
        let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_toplevel",
            Some(Box::new(json!({
                "surfaceId": surface_id,
                "pid": pid,
            }))),
            None,
        );
    }

    let popups = data.xdg_popups.clone();
    for surface_id in popups.keys() {
        if let Some(wl_surface) = surfaces.get(surface_id) {
            let (parent, position) = {
                let popup_message = data.construct_popup_role_message(wl_surface.clone().borrow());
                (
                    popup_message.as_ref().unwrap().parent,
                    popup_message.unwrap().position,
                )
            };
            let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;
            platform_method_channel.invoke_method(
                "new_popup",
                Some(Box::new(json!({
                    "surfaceId": surface_id,
                    "parent": parent,
                    "position": position,
                }))),
                None,
            );
        }
    }
    let subsurfaces = data.subsurfaces.clone();

    for surface_id in subsurfaces.keys() {
        if let Some(wl_surface) = surfaces.get(surface_id) {
            let subsurface_message = State::<BackendData>::construct_subsurface_role_message(
                wl_surface.clone().borrow(),
            );
            let platform_method_channel: &mut crate::flutter_engine::platform_channels::method_channel::MethodChannel<serde_json::Value> = &mut data.flutter_engine_mut().platform_method_channel;

            platform_method_channel.invoke_method(
                "new_subsurface",
                Some(Box::new(json!({
                    "surfaceId": surface_id,
                    "parent": subsurface_message.parent,
                }))),
                None,
            );
        }
    }

    let x11_surfaces = data.x11_surface_per_x11_window.clone();

    for x11_surface in x11_surfaces.values() {
        let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_x11_surface",
            Some(Box::new(json!(NewX11Surface {
                x11_surface_id: State::<BackendData>::get_x11_surface_id(x11_surface),
                override_redirect: x11_surface.is_override_redirect(),
            }))),
            None,
        );
    }

    for x11_surface in x11_surfaces.values() {
        let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;

        platform_method_channel.invoke_method(
            "x11_properties_changed",
            Some(Box::new(json!({
                "x11SurfaceId": State::<BackendData>::get_x11_surface_id(x11_surface),
                "title": if !x11_surface.title().is_empty() { Some(x11_surface.title()) } else { None },
                "windowClass": x11_surface.class(),
                "instance": if !x11_surface.instance().is_empty() {
                    Some(x11_surface.instance())
                } else {
                    None
                },
                "startupId": x11_surface.startup_id(),
                "pid": x11_surface.pid(),
            }))),
            None,
        );
        if x11_surface.is_mapped() {
            platform_method_channel.invoke_method(
                "surface_associated",
                Some(Box::new(json!({
                    "surfaceId":  get_surface_id(x11_surface.wl_surface().unwrap().borrow()),
                    "x11SurfaceId": State::<BackendData>::get_x11_surface_id(x11_surface),
                }))),
                None,
            );
            data.map_x11_surface(x11_surface.clone());
        }
    }

    // send commited_state for all existing surface
    for surface_id in surfaces.keys() {
        if let Some(wl_surface) = surfaces.get(surface_id) {
            let surface_message = data.construct_surface_message(wl_surface.clone().borrow());
            let platform_method_channel = &mut data.flutter_engine_mut().platform_method_channel;
            tracing::info!("Restore commit_surface");
            platform_method_channel.invoke_method(
                "commit_surface",
                Some(Box::new(json!(surface_message))),
                None,
            );
        }
    }
    result.success(None);
}
