//! Window snapping state (PLAN.md §4.2): which window is under the pointer,
//! and which of the windows stacked there W has cycled to.

use hoarder_wayland::Window;

/// The windows the Overlay can snap to, and the one it currently would.
#[derive(Debug)]
pub struct Hover {
    /// Topmost first.
    windows: Vec<Window>,
    /// Indices of the windows under the pointer, topmost first.
    stack: Vec<usize>,
    /// How many times W was pressed since the stack last changed.
    cycle: usize,
}

impl Hover {
    pub fn new(windows: Vec<Window>) -> Hover {
        Hover {
            windows,
            stack: Vec::new(),
            cycle: 0,
        }
    }

    /// The pointer moved to the global logical point `(x, y)`. Moving onto
    /// a different stack of windows starts over at the topmost one.
    pub fn point(&mut self, (x, y): (f64, f64)) {
        let stack: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.contains(x, y))
            .map(|(i, _)| i)
            .collect();
        if stack != self.stack {
            self.stack = stack;
            self.cycle = 0;
        }
    }

    /// W: go one window further down the stack, and back to the top.
    pub fn cycle(&mut self) {
        self.cycle += 1;
    }

    /// The index of the window a click would capture.
    pub fn current(&self) -> Option<usize> {
        (!self.stack.is_empty()).then(|| self.stack[self.cycle % self.stack.len()])
    }

    pub fn window(&self, index: usize) -> &Window {
        &self.windows[index]
    }
}
