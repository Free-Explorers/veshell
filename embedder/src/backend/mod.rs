use smithay::backend::{renderer::gles::GlesRenderer, session::libseat::LibSeatSession};

pub mod drm_backend;
pub mod render;
pub mod x11_client;

pub trait Backend {
    const HAS_RELATIVE_MOTION: bool = false;

    fn seat_name(&self) -> String;

    fn get_session(&self) -> LibSeatSession;

    fn with_primary_renderer<T>(&mut self, f: impl FnOnce(&GlesRenderer) -> T) -> Option<T>;

    fn with_primary_renderer_mut<T>(&mut self, f: impl FnOnce(&mut GlesRenderer) -> T)
        -> Option<T>;
}
