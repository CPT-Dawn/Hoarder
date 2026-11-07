//! A sprite: a small picture floating over the Overlay (the Loupe, the hint
//! bar, the cursor layer) on its own subsurface, so moving or redrawing it
//! never touches the full-output chrome buffer.
//!
//! The subsurface is desynchronised and takes no input, so pointer events
//! keep landing on the Overlay surface beneath. Its position is in the
//! parent's logical coordinates and takes effect on the parent's next commit.

use crate::overlay::App;
use smithay_client_toolkit::{
    compositor::CompositorState,
    shm::slot::{Buffer, SlotPool},
    subcompositor::SubcompositorState,
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_region::WlRegion, wl_shm, wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

#[derive(Debug)]
pub(crate) struct Sprite {
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    buffers: Vec<Buffer>,
    /// Logical position last asked for.
    at: (i32, i32),
    shown: bool,
}

impl Sprite {
    /// A hidden sprite over `parent`, or just above `sibling` if given.
    pub(crate) fn new(
        parent: &WlSurface,
        sibling: Option<&WlSurface>,
        compositor: &CompositorState,
        subcompositor: &SubcompositorState,
        viewporter: &WpViewporter,
        qh: &QueueHandle<App>,
    ) -> Sprite {
        let (subsurface, surface) = subcompositor.create_subsurface(parent.clone(), qh);
        subsurface.set_desync();
        if let Some(sibling) = sibling {
            subsurface.place_above(sibling);
        }
        let empty: WlRegion = compositor.wl_compositor().create_region(qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let viewport = viewporter.get_viewport(&surface, qh, ());
        Sprite {
            surface,
            subsurface,
            viewport,
            buffers: Vec::new(),
            at: (i32::MIN, i32::MIN),
            shown: false,
        }
    }

    pub(crate) fn is_shown(&self) -> bool {
        self.shown
    }

    /// Show a `size` (physical pixels) picture, `logical` big, at logical
    /// `at`. `paint` fills the cleared canvas: premultiplied `Argb8888`,
    /// `4 * size.0` bytes per row. Returns whether the parent must commit
    /// for a new position to apply.
    pub(crate) fn show(
        &mut self,
        pool: &mut SlotPool,
        size: (u32, u32),
        logical: (i32, i32),
        at: (i32, i32),
        paint: impl FnOnce(&mut [u8]),
    ) -> std::io::Result<bool> {
        let (w, h) = (size.0.max(1) as i32, size.1.max(1) as i32);
        self.buffers
            .retain(|b| b.height() == h && b.stride() == w * 4 || b.canvas(pool).is_none());
        let free = self
            .buffers
            .iter()
            .position(|b| b.height() == h && b.stride() == w * 4 && b.canvas(pool).is_some());
        let index = match free {
            Some(i) => i,
            None => {
                let (buffer, _) = pool
                    .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
                    .map_err(std::io::Error::other)?;
                self.buffers.push(buffer);
                self.buffers.len() - 1
            }
        };
        let buffer = &self.buffers[index];
        let canvas = buffer.canvas(pool).expect("free or new");
        canvas.fill(0);
        paint(canvas);
        self.viewport
            .set_destination(logical.0.max(1), logical.1.max(1));
        buffer
            .attach_to(&self.surface)
            .map_err(std::io::Error::other)?;
        self.surface.damage_buffer(0, 0, w, h);
        self.surface.commit();
        self.shown = true;
        Ok(self.move_to(at))
    }

    /// Move to logical `at`. Returns whether the parent must commit.
    pub(crate) fn move_to(&mut self, at: (i32, i32)) -> bool {
        if self.at == at {
            return false;
        }
        self.at = at;
        self.subsurface.set_position(at.0, at.1);
        true
    }

    pub(crate) fn hide(&mut self) {
        if self.shown {
            self.surface.attach(None, 0, 0);
            self.surface.commit();
            self.shown = false;
        }
    }
}

impl Drop for Sprite {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
    }
}
