//! Raw output buffers read back through `wlr-screencopy`.

use crate::Rgb10;

/// The `wl_shm` formats a compositor may hand back for a screencopy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Xrgb8888,
    Argb8888,
    Xbgr8888,
    Abgr8888,
    Xrgb2101010,
    Argb2101010,
    Xbgr2101010,
    Abgr2101010,
}

impl FrameFormat {
    pub(crate) fn from_shm(code: u32) -> Option<Self> {
        use wayland_client::protocol::wl_shm::Format as F;
        Some(match F::try_from(code).ok()? {
            F::Xrgb8888 => Self::Xrgb8888,
            F::Argb8888 => Self::Argb8888,
            F::Xbgr8888 => Self::Xbgr8888,
            F::Abgr8888 => Self::Abgr8888,
            F::Xrgb2101010 => Self::Xrgb2101010,
            F::Argb2101010 => Self::Argb2101010,
            F::Xbgr2101010 => Self::Xbgr2101010,
            F::Abgr2101010 => Self::Abgr2101010,
            _ => return None,
        })
    }

    pub fn bits_per_channel(self) -> u32 {
        match self {
            Self::Xrgb8888 | Self::Argb8888 | Self::Xbgr8888 | Self::Abgr8888 => 8,
            _ => 10,
        }
    }

    fn is_bgr(self) -> bool {
        matches!(
            self,
            Self::Xbgr8888 | Self::Abgr8888 | Self::Xbgr2101010 | Self::Abgr2101010
        )
    }
}

/// One output's buffer exactly as the compositor holds it: physical size,
/// native format, *before* the output transform is undone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub(crate) format: FrameFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: u32,
    pub(crate) y_invert: bool,
    pub(crate) data: Vec<u8>,
}

impl RawFrame {
    pub fn format(&self) -> FrameFormat {
        self.format
    }

    pub fn bits_per_channel(&self) -> u32 {
        self.format.bits_per_channel()
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Whether the compositor reported the rows bottom-up.
    pub fn y_invert(&self) -> bool {
        self.y_invert
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The pixel at buffer coordinates `(x, y)` (rows as stored), widened to
    /// 10 bits per channel for 8-bit formats by bit replication.
    pub fn pixel(&self, x: u32, y: u32) -> Rgb10 {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside {}x{}",
            self.width,
            self.height
        );
        let i = (y * self.stride + x * 4) as usize;
        let v = u32::from_le_bytes(self.data[i..i + 4].try_into().expect("4 bytes"));
        let (hi, mid, lo) = if self.bits_per_channel() == 8 {
            let w = |c: u32| ((c << 2) | (c >> 6)) as u16;
            (w((v >> 16) & 0xff), w((v >> 8) & 0xff), w(v & 0xff))
        } else {
            (
                ((v >> 20) & 0x3ff) as u16,
                ((v >> 10) & 0x3ff) as u16,
                (v & 0x3ff) as u16,
            )
        };
        if self.format.is_bgr() {
            Rgb10(lo, mid, hi)
        } else {
            Rgb10(hi, mid, lo)
        }
    }
}
