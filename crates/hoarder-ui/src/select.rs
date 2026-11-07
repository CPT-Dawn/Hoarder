//! The Overlay's selection state machine (PLAN.md §4.2), free of Wayland:
//! pointer and key input in global logical coordinates go in, the selection
//! to draw and the Target to capture come out.
//!
//! ```text
//! Idle ──press──► Dragging ──release──► Capture(Region)
//!  │                 │  Space held: move instead of resize
//!  │                 └─ Shift+release / confirm ──► Refine ──Enter──► Capture(Region)
//!  │  O / click ──► Capture(Output under the pointer)
//!  │  A ──► Capture(All Outputs)
//!  │  Enter ──► Capture(last Region)
//!  └─ Esc ──► Cancel
//! ```

use crate::overlay::Region;

/// How far from a Refine handle a press still grabs it, in logical pixels.
const GRAB_REACH: f64 = 10.0;

/// A key the selection reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Space,
    Output,
    AllOutputs,
    /// Toggle the cursor layer; the selection ignores it.
    Cursor,
    Left,
    Right,
    Up,
    Down,
}

/// What the input asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Region(Region),
    /// The Output under this point (a click, or `O` at the pointer).
    OutputAt((f64, f64)),
    AllOutputs,
    Cancel,
}

/// Which edges of the selection a grab or a nudge moves. All four: the
/// whole selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Edges {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

impl Edges {
    const ALL: Edges = Edges {
        left: true,
        top: true,
        right: true,
        bottom: true,
    };
}

/// A selection as its four edges, `x0 <= x1` and `y0 <= y1`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn spanning(a: (f64, f64), b: (f64, f64)) -> Rect {
        Rect {
            x0: a.0.min(b.0),
            y0: a.1.min(b.1),
            x1: a.0.max(b.0),
            y1: a.1.max(b.1),
        }
    }

    fn region(&self) -> Region {
        Region {
            x: self.x0,
            y: self.y0,
            width: self.x1 - self.x0,
            height: self.y1 - self.y0,
        }
    }

    /// Move `edges` by `(dx, dy)`; edges that cross swap roles, and so do
    /// the flags in `edges`, so a grab keeps following the pointer.
    fn shift(&mut self, edges: &mut Edges, (dx, dy): (f64, f64)) {
        if edges.left {
            self.x0 += dx;
        }
        if edges.right {
            self.x1 += dx;
        }
        if edges.top {
            self.y0 += dy;
        }
        if edges.bottom {
            self.y1 += dy;
        }
        if self.x0 > self.x1 {
            std::mem::swap(&mut self.x0, &mut self.x1);
            std::mem::swap(&mut edges.left, &mut edges.right);
        }
        if self.y0 > self.y1 {
            std::mem::swap(&mut self.y0, &mut self.y1);
            std::mem::swap(&mut edges.top, &mut edges.bottom);
        }
    }

    /// What a press at `at` grabs: a corner or edge handle, the inside
    /// (move), or nothing.
    fn grab_at(&self, (x, y): (f64, f64)) -> Option<Edges> {
        let near = |v: f64, edge: f64| (v - edge).abs() <= GRAB_REACH;
        let within_x = x >= self.x0 - GRAB_REACH && x <= self.x1 + GRAB_REACH;
        let within_y = y >= self.y0 - GRAB_REACH && y <= self.y1 + GRAB_REACH;
        if !(within_x && within_y) {
            return None;
        }
        let edges = Edges {
            left: near(x, self.x0) && !near(x, self.x1),
            right: near(x, self.x1),
            top: near(y, self.y0) && !near(y, self.y1),
            bottom: near(y, self.y1),
        };
        if edges != Edges::default() {
            return Some(edges);
        }
        let inside = x > self.x0 && x < self.x1 && y > self.y0 && y < self.y1;
        inside.then_some(Edges::ALL)
    }
}

#[derive(Debug, Clone, Copy)]
enum State {
    Idle,
    /// The button is down and the pointer is drawing a selection.
    Dragging {
        anchor: (f64, f64),
        /// Where the pointer is.
        pointer: (f64, f64),
        /// Arrow-key nudges, added to the pointer's position.
        nudge: (f64, f64),
        /// The Refine selection to go back to if this drag turns out to be a click.
        previous: Option<Rect>,
    },
    Refine {
        rect: Rect,
        /// What the arrows move: the handle last grabbed.
        active: Edges,
        /// The handle being dragged, and the pointer's last position.
        grab: Option<(Edges, (f64, f64))>,
    },
}

/// The Overlay's selection and what input does to it.
#[derive(Debug, Clone)]
pub struct Selector {
    state: State,
    /// Every release goes to Refine (`screenshot.confirm`).
    confirm: bool,
    last_region: Option<Region>,
    shift: bool,
    space: bool,
    /// With clamping on, the outputs' rectangles; a selection stays on the
    /// one its drag started on.
    outputs: Option<Vec<Rect>>,
    /// The output the current selection is held to.
    home: Option<Rect>,
}

impl Selector {
    pub fn new(confirm: bool, last_region: Option<Region>) -> Selector {
        Selector {
            state: State::Idle,
            confirm,
            last_region,
            shift: false,
            space: false,
            outputs: None,
            home: None,
        }
    }

    /// Keep every selection on the output, out of `outputs` (`(x, y,
    /// width, height)`), where its drag started.
    pub fn clamp_to(&mut self, outputs: Vec<(f64, f64, f64, f64)>) {
        self.outputs = Some(
            outputs
                .into_iter()
                .map(|(x, y, w, h)| Rect {
                    x0: x,
                    y0: y,
                    x1: x + w,
                    y1: y + h,
                })
                .collect(),
        );
    }

    /// `rect` cut down to the home output, if selections are held to one.
    fn clamped(&self, rect: Rect) -> Rect {
        match self.home {
            Some(home) => {
                let x0 = rect.x0.clamp(home.x0, home.x1);
                let y0 = rect.y0.clamp(home.y0, home.y1);
                Rect {
                    x0,
                    y0,
                    x1: rect.x1.clamp(x0, home.x1),
                    y1: rect.y1.clamp(y0, home.y1),
                }
            }
            None => rect,
        }
    }

    /// The selection to show, if any.
    pub fn region(&self) -> Option<Region> {
        match self.state {
            State::Idle => None,
            State::Dragging {
                anchor,
                pointer,
                nudge,
                ..
            } => Some(
                self.clamped(Rect::spanning(anchor, plus(pointer, nudge)))
                    .region(),
            ),
            State::Refine { rect, .. } => Some(self.clamped(rect).region()),
        }
    }

    /// Whether the selection is in Refine (and so shows its handles).
    pub fn refining(&self) -> bool {
        matches!(self.state, State::Refine { .. })
    }

    pub fn set_shift(&mut self, held: bool) {
        self.shift = held;
    }

    /// Whether Shift is held.
    pub fn shift_held(&self) -> bool {
        self.shift
    }

    /// Left button pressed at `at`.
    pub fn press(&mut self, at: (f64, f64)) {
        match &mut self.state {
            State::Refine { rect, active, grab } => match rect.grab_at(at) {
                Some(edges) => {
                    *active = edges;
                    *grab = Some((edges, at));
                }
                None => {
                    let previous = Some(*rect);
                    self.start_drag(at, previous);
                }
            },
            _ => self.start_drag(at, None),
        }
    }

    fn start_drag(&mut self, at: (f64, f64), previous: Option<Rect>) {
        self.home = self.outputs.as_ref().and_then(|outputs| {
            outputs
                .iter()
                .find(|o| at.0 >= o.x0 && at.0 < o.x1 && at.1 >= o.y0 && at.1 < o.y1)
                .copied()
        });
        self.state = State::Dragging {
            anchor: at,
            pointer: at,
            nudge: (0.0, 0.0),
            previous,
        };
    }

    /// The pointer moved to `at`.
    pub fn motion(&mut self, at: (f64, f64)) {
        match &mut self.state {
            State::Dragging {
                anchor, pointer, ..
            } => {
                if self.space {
                    *anchor = plus(*anchor, (at.0 - pointer.0, at.1 - pointer.1));
                }
                *pointer = at;
            }
            State::Refine {
                rect,
                active,
                grab: Some((edges, last)),
            } => {
                rect.shift(edges, (at.0 - last.0, at.1 - last.1));
                *last = at;
                *active = *edges;
            }
            _ => {}
        }
    }

    /// Left button released at `at`. `px` is the size of one physical pixel
    /// there, in logical pixels: a drag smaller than that is a click.
    pub fn release(&mut self, at: (f64, f64), px: f64) -> Option<Action> {
        self.motion(at);
        match self.state {
            State::Dragging {
                anchor,
                pointer,
                nudge,
                previous,
            } => {
                let cursor = plus(pointer, nudge);
                let rect = self.clamped(Rect::spanning(anchor, cursor));
                let tiny = (rect.x1 - rect.x0) / px < 0.5 || (rect.y1 - rect.y0) / px < 0.5;
                if tiny {
                    return match previous {
                        Some(rect) => {
                            self.refine(rect, Edges::ALL);
                            None
                        }
                        None => {
                            self.state = State::Idle;
                            Some(Action::OutputAt(at))
                        }
                    };
                }
                if self.shift || self.confirm || previous.is_some() {
                    let active = Edges {
                        left: cursor.0 < anchor.0,
                        right: cursor.0 >= anchor.0,
                        top: cursor.1 < anchor.1,
                        bottom: cursor.1 >= anchor.1,
                    };
                    self.refine(rect, active);
                    None
                } else {
                    self.state = State::Idle;
                    Some(Action::Region(rect.region()))
                }
            }
            State::Refine { ref mut grab, .. } => {
                *grab = None;
                None
            }
            State::Idle => None,
        }
    }

    fn refine(&mut self, rect: Rect, active: Edges) {
        self.state = State::Refine {
            rect,
            active,
            grab: None,
        };
    }

    /// A key went down (or up, for `released`). `px` is the size of one
    /// physical pixel under the selection's moving corner, in logical pixels.
    pub fn key(&mut self, key: Key, released: bool, px: f64) -> Option<Action> {
        if key == Key::Space {
            self.space = !released;
            return None;
        }
        if released {
            return None;
        }
        let step = if self.shift { 10.0 } else { 1.0 } * px;
        let (dx, dy) = match key {
            Key::Left => (-step, 0.0),
            Key::Right => (step, 0.0),
            Key::Up => (0.0, -step),
            Key::Down => (0.0, step),
            Key::Escape => return Some(Action::Cancel),
            Key::AllOutputs => return Some(Action::AllOutputs),
            Key::Output => return None, // resolved by the caller, which knows the pointer
            Key::Cursor => return None,
            Key::Enter => {
                return match self.state {
                    State::Idle => self.last_region.map(Action::Region),
                    State::Refine { rect, .. } => Some(Action::Region(self.clamped(rect).region())),
                    State::Dragging { .. } => None,
                };
            }
            Key::Space => unreachable!(),
        };
        match &mut self.state {
            State::Dragging { nudge, .. } => *nudge = plus(*nudge, (dx, dy)),
            State::Refine { rect, active, .. } => {
                // An arrow only moves the edges the active handle has along
                // its own axis.
                let mut edges = Edges {
                    left: active.left && dx != 0.0,
                    right: active.right && dx != 0.0,
                    top: active.top && dy != 0.0,
                    bottom: active.bottom && dy != 0.0,
                };
                rect.shift(&mut edges, (dx, dy));
                if dx != 0.0 {
                    (active.left, active.right) = (edges.left, edges.right);
                } else {
                    (active.top, active.bottom) = (edges.top, edges.bottom);
                }
            }
            State::Idle => {}
        }
        None
    }

    /// The point arrow keys move, if any: the drag's moving corner, or the
    /// active handle in Refine.
    pub fn moving_point(&self) -> Option<(f64, f64)> {
        match self.state {
            State::Idle => None,
            State::Dragging { pointer, nudge, .. } => Some(plus(pointer, nudge)),
            State::Refine { rect, active, .. } => Some((
                if active.left { rect.x0 } else { rect.x1 },
                if active.top { rect.y0 } else { rect.y1 },
            )),
        }
    }
}

fn plus(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: f64, y: f64, width: f64, height: f64) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_drag_that_crosses_over_its_anchor_still_spans_both_points() {
        let mut s = Selector::new(false, None);
        s.press((100.0, 100.0));
        s.motion((50.0, 150.0));
        assert_eq!(
            s.release((40.0, 160.0), 1.0),
            Some(Action::Region(region(40.0, 100.0, 60.0, 60.0)))
        );
    }

    #[test]
    fn a_refine_handle_dragged_past_the_opposite_edge_flips_the_selection() {
        let mut s = Selector::new(true, None);
        s.press((100.0, 100.0));
        s.release((200.0, 200.0), 1.0);
        s.press((200.0, 150.0)); // the right edge
        s.release((50.0, 150.0), 1.0);
        assert_eq!(s.region(), Some(region(50.0, 100.0, 50.0, 100.0)));
        // The grabbed edge is now the left one, and arrows move it.
        s.key(Key::Left, false, 1.0);
        assert_eq!(s.region(), Some(region(49.0, 100.0, 51.0, 100.0)));
    }

    #[test]
    fn a_clamped_selection_stays_on_its_output_through_drags_and_nudges() {
        let mut s = Selector::new(false, None);
        s.clamp_to(vec![(0.0, 0.0, 320.0, 240.0), (320.0, 0.0, 320.0, 240.0)]);
        s.press((300.0, 100.0));
        s.motion((400.0, 300.0));
        assert_eq!(s.region(), Some(region(300.0, 100.0, 20.0, 140.0)));
        s.shift = true;
        s.key(Key::Right, false, 1.0);
        s.shift = false;
        assert_eq!(
            s.release((330.0, 120.0), 1.0),
            Some(Action::Region(region(300.0, 100.0, 20.0, 20.0)))
        );
        // The next drag, on the other output, is held there instead.
        s.press((330.0, 100.0));
        assert_eq!(
            s.release((10.0, 120.0), 1.0),
            Some(Action::Region(region(320.0, 100.0, 10.0, 20.0)))
        );
    }
}
