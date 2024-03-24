use crate::flutter_engine::wayland_messages::{MapX11Surface, NewX11Surface};
use crate::focus::KeyboardFocusTarget;
use crate::server::{get_surface_id, ServerState};
use crate::{Backend, CalloopData};
use serde_json::json;
use smithay::desktop::space::SpaceElement;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection};
use smithay::wayland::selection::primary_selection::{clear_primary_selection, current_primary_selection_userdata, PrimarySelectionHandler, PrimarySelectionState, request_primary_client_selection, set_primary_selection};
use smithay::wayland::selection::SelectionTarget;
use smithay::xwayland::xwm::{Reorder, XwmId};
use smithay::xwayland::{xwm, X11Surface, X11Wm, XwmHandler};
use std::cell::RefCell;
use std::os::fd::OwnedFd;
use smithay::delegate_primary_selection;
use tracing::{error, trace};

struct MyX11SurfaceState {
    x11_surface_id: u64,
}

fn get_x11_surface_id(x11_surface: &X11Surface) -> u64 {
    x11_surface
        .user_data()
        .get::<RefCell<MyX11SurfaceState>>()
        .unwrap()
        .borrow()
        .x11_surface_id
}

impl<BackendData: Backend> ServerState<BackendData> {
    fn new_x11_surface(&mut self, surface: X11Surface) {
        self.x11_surface_per_x11_window
            .insert(surface.window_id(), surface.clone());

        surface.user_data().insert_if_missing(|| {
            RefCell::new(MyX11SurfaceState {
                x11_surface_id: self.get_new_x11_surface_id(),
            })
        });

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "new_x11_surface",
            Some(Box::new(json!(NewX11Surface {
                x11_surface_id: get_x11_surface_id(&surface),
            }))),
            None,
        );
    }

    fn map_x11_surface(&mut self, surface: X11Surface) {
        let Some(wl_surface) = surface.wl_surface() else {
            return;
        };
        self.x11_surface_per_wl_surface
            .insert(wl_surface.clone(), surface.clone());

        let parent = if surface.is_override_redirect() {
            surface
                .is_transient_for()
                .and_then(|x11_surface| self.x11_surface_per_x11_window.get(&x11_surface))
                // Fall back on the focused surface if the transient parent is not known.
                .or_else(|| {
                    self.keyboard.current_focus().and_then(|focus| {
                        focus
                            .wl_surface()
                            .and_then(|focus| self.x11_surface_per_wl_surface.get(&focus))
                    })
                })
                .map(|x11_surface| get_x11_surface_id(x11_surface))
        } else {
            surface
                .is_transient_for()
                .and_then(|x11_surface| self.x11_surface_per_x11_window.get(&x11_surface))
                .map(|x11_surface| get_x11_surface_id(&x11_surface))
        };

        let platform_method_channel = &mut self.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "map_x11_surface",
            Some(Box::new(json!(MapX11Surface {
                x11_surface_id: get_x11_surface_id(&surface),
                surface_id: get_surface_id(&wl_surface),
                override_redirect: surface.is_override_redirect(),
                geometry: surface.geometry().into(),
                parent,
                title: surface.title(),
                window_class: surface.class(),
                instance: surface.instance(),
                startup_id: surface.startup_id(),
            }))),
            None,
        );
    }
}

impl<BackendData: Backend> XwmHandler for CalloopData<BackendData> {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.state.x11_wm.as_mut().unwrap()
    }

    fn new_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        let mut geometry = surface.geometry();
        geometry.loc = Point::from((0, 0));
        surface.configure(geometry).unwrap();

        self.state.new_x11_surface(surface);
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.state.new_x11_surface(surface);
    }

    fn map_window_request(&mut self, _xwm: XwmId, surface: X11Surface) {
        surface.set_mapped(true).unwrap();
        surface.set_activated(true).unwrap();
    }

    fn map_window_notify(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.state.map_x11_surface(surface);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, surface: X11Surface) {
        self.state.map_x11_surface(surface.clone());
        surface.set_activated(true).unwrap();
    }

    fn unmapped_window(&mut self, xwm: XwmId, surface: X11Surface) {
        let Some(wl_surface) = surface.wl_surface() else {
            return;
        };

        self.state.x11_surface_per_wl_surface.remove(&wl_surface);

        let x11_surface_id = get_x11_surface_id(&surface);

        let platform_method_channel = &mut self.state.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "unmap_x11_surface",
            Some(Box::new(json!({
                "x11SurfaceId": x11_surface_id,
            }))),
            None,
        );

        if !surface.is_override_redirect() {
            surface.set_mapped(false).unwrap();
        }
    }

    fn destroyed_window(&mut self, xwm: XwmId, surface: X11Surface) {
        let x11_surface_id = get_x11_surface_id(&surface);

        let platform_method_channel = &mut self.state.flutter_engine_mut().platform_method_channel;
        platform_method_channel.invoke_method(
            "destroy_x11_surface",
            Some(Box::new(json!({
                "x11SurfaceId": x11_surface_id,
            }))),
            None,
        );

        self.state
            .x11_surface_per_x11_window
            .remove(&surface.window_id());
    }

    fn configure_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        reorder: Option<Reorder>,
    ) {
        // We just set the new size, but don't let windows move themselves around freely.
        let mut geo = window.geometry();
        geo.loc = Point::from((0, 0));
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        above: Option<u32>,
    ) {
    }

    fn resize_request(
        &mut self,
        xwm: XwmId,
        window: X11Surface,
        button: u32,
        resize_edge: xwm::ResizeEdge,
    ) {
    }

    fn move_request(&mut self, xwm: XwmId, window: X11Surface, button: u32) {
    }

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        if let Some(keyboard) = self.state.seat.get_keyboard() {
            // check that an X11 window is focused
            if let Some(KeyboardFocusTarget::X11Surface(surface)) = keyboard.current_focus() {
                if surface.xwm_id().unwrap() == xwm {
                    return true;
                }
            }
        }
        false
    }

    fn send_selection(
        &mut self,
        _xwm: XwmId,
        selection: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) =
                    request_data_device_client_selection(&self.state.seat, mime_type, fd)
                {
                    error!(
                        ?err,
                        "Failed to request current wayland clipboard for Xwayland",
                    );
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.state.seat, mime_type, fd)
                {
                    error!(
                        ?err,
                        "Failed to request current wayland primary selection for Xwayland",
                    );
                }
            }
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        trace!(?selection, ?mime_types, "Got Selection from X11",);
        // TODO check, that focused windows is X11 window before doing this
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(
                &self.state.display_handle,
                &self.state.seat,
                mime_types,
                (),
            ),
            SelectionTarget::Primary => {
                set_primary_selection(&self.state.display_handle, &self.state.seat, mime_types, ())
            }
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.state.seat).is_some() {
                    clear_data_device_selection(&self.state.display_handle, &self.state.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.state.seat).is_some() {
                    clear_primary_selection(&self.state.display_handle, &self.state.seat)
                }
            }
        }
    }
}
