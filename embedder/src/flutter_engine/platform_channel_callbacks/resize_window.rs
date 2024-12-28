use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg;
use smithay::wayland::xwayland_shell::XWAYLAND_SHELL_ROLE;

use crate::backend::Backend;
use crate::flutter_engine::platform_channels::method_call::MethodCall;
use crate::flutter_engine::platform_channels::method_result::MethodResult;

use crate::state::State;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeWindowPayload {
    surface_id: u64,
    width: i32,
    height: i32,
}

pub fn resize_window<BackendData: Backend + 'static>(
    method_call: MethodCall<serde_json::Value>,
    mut result: Box<dyn MethodResult<serde_json::Value>>,
    data: &mut State<BackendData>,
) {
    let args = method_call.arguments().unwrap().clone();
    let payload: ResizeWindowPayload = serde_json::from_value(args).unwrap();

    let Some(wl_surface) = data.surfaces.get(&payload.surface_id).cloned() else {
        result.error(
            "surface_doesnt_exist".to_string(),
            format!("Surface {} doesn't exist", payload.surface_id),
            None,
        );
        return;
    };

    let role = with_states(&wl_surface, |states| states.role);
    match role {
        Some(xdg::XDG_TOPLEVEL_ROLE) => {
            let toplevel = data.xdg_toplevels.get(&payload.surface_id).cloned();

            let Some(toplevel) = toplevel else {
                result.error(
                    "toplevel_doesnt_exist".to_string(),
                    format!("Toplevel {} doesn't exist", payload.surface_id),
                    None,
                );
                return;
            };

            toplevel.with_pending_state(|state| {
                state.size = Some((payload.width, payload.height).into());
            });
            toplevel.send_pending_configure();

            result.success(None);
        }
        Some(XWAYLAND_SHELL_ROLE) => {
            let Some(x11_surface) = data.x11_surface_per_wl_surface.get(&wl_surface) else {
                result.error(
                    "x11_surface_doesnt_exist".to_string(),
                    format!("X11 Surface {} doesn't exist", payload.surface_id),
                    None,
                );
                return;
            };

            if x11_surface.is_override_redirect() {
                result.error(
                    "resize_unsupported_for_override_redirect_x11_surfaces".to_string(),
                    format!(
                        "Resize unsupported for override redirect X11 surface {}",
                        payload.surface_id,
                    ),
                    None,
                );
                return;
            }

            let mut geometry = x11_surface.geometry();
            geometry.size = (payload.width, payload.height).into();
            x11_surface.configure(geometry).unwrap();

            result.success(None);
        }
        _ => {
            result.error(
                "invalid_surface_role".to_string(),
                format!("Surface {} has an invalid role", payload.surface_id),
                None,
            );
        }
    }
}
