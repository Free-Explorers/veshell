use std::env;

use backend::Backend;
use log::debug;
use smithay::{
    reexports::wayland_server::{
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_surface::{self},
    },
    wayland::compositor::{
        with_surface_tree_downward, CompositorClientState, SurfaceAttributes, TraversalAction,
    },
};
use tracing_subscriber::{fmt, EnvFilter};

use crate::flutter_engine::FlutterEngine;
use crate::mouse_button_tracker::MouseButtonTracker;
use crate::state::State;

mod backend;
mod cursor;
mod flutter_engine;
mod focus;
mod gles_framebuffer_importer;
mod input_handling;
mod keyboard;
mod mouse_button_tracker;
mod state;
mod texture_swap_chain;
mod wayland;
mod xwayland;

fn initialize_env() {
    if std::env::var("VESHELL_DEFAULT_CONFIG_DIR").is_err() {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let config_dir = exe_dir.join("settings/default");
                std::env::set_var("VESHELL_DEFAULT_CONFIG_DIR", config_dir);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Set up subscriber with both stdout and file layers
    tracing_subscriber::fmt()
        .with_timer(fmt::time::ChronoLocal::new(String::from(
            "%Y-%m-%d %H:%M:%S%.3f",
        )))
        .compact()
        .with_env_filter(EnvFilter::from_default_env()) // Log level from RUST_LOG
        .init();

    debug!("Starting Veshell");
    // Fix XWayland crash when too many file descriptors are open.
    let _ = rlimit::increase_nofile_limit(u64::MAX);
    initialize_env();
    if env::var("DISPLAY").is_ok() || env::var("WAYLAND_DISPLAY").is_ok() {
        backend::x11_client::run_x11_client();
    } else {
        backend::drm_backend::run_drm_backend();
    }

    Ok(())
}

pub struct FlutterState<BackendData: Backend + 'static> {
    pub flutter_engine: FlutterEngine<BackendData>,
    pub mouse_button_tracker: MouseButtonTracker,
}

pub fn send_frames_surface_tree(surface: &wl_surface::WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surf, states, &()| {
            // the surface may not have any user_data if it is a subsurface and has not
            // yet been commited
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {
        debug!("Client initialized {:?}", _client_id);
    }

    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        debug!("Client disconnected {:?} {:?}", _client_id, _reason);
    }
}
