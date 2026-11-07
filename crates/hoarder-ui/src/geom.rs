//! Integer rectangles in a surface's physical pixels, for damage tracking.

/// A rectangle of physical pixels. Empty when `w` or `h` is not positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = Rect::new(
            x,
            y,
            self.right().min(other.right()) - x,
            self.bottom().min(other.bottom()) - y,
        );
        if r.is_empty() { Rect::default() } else { r }
    }

    /// The smallest rectangle covering both (empty ones are ignored).
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// Grow by `n` pixels on every side.
    pub fn inflate(&self, n: i32) -> Rect {
        Rect::new(self.x - n, self.y - n, self.w + 2 * n, self.h + 2 * n)
    }

    /// `self` minus `other`, as up to four disjoint rectangles.
    pub fn subtract(&self, other: &Rect) -> Vec<Rect> {
        let cut = self.intersect(other);
        if cut.is_empty() {
            return if self.is_empty() { vec![] } else { vec![*self] };
        }
        [
            Rect::new(self.x, self.y, self.w, cut.y - self.y),
            Rect::new(self.x, cut.bottom(), self.w, self.bottom() - cut.bottom()),
            Rect::new(self.x, cut.y, cut.x - self.x, cut.h),
            Rect::new(cut.right(), cut.y, self.right() - cut.right(), cut.h),
        ]
        .into_iter()
        .filter(|r| !r.is_empty())
        .collect()
    }

    /// The one-pixel ring just outside `self`, as four rectangles.
    pub fn ring(&self) -> [Rect; 4] {
        let o = self.inflate(1);
        [
            Rect::new(o.x, o.y, o.w, 1),
            Rect::new(o.x, o.bottom() - 1, o.w, 1),
            Rect::new(o.x, self.y, 1, self.h),
            Rect::new(o.right() - 1, self.y, 1, self.h),
        ]
    }
}
