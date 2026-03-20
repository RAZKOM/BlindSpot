use serde::{Deserialize, Serialize};

use crate::tracker::RectPx;

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RedactBox {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub w: f32,
    #[serde(default)]
    pub h: f32,

    pub anchor: Anchor,

    #[serde(default)]
    pub anchor_offset_x_px: f32,
    #[serde(default)]
    pub anchor_offset_y_px: f32,

    #[serde(default)] 
    
    pub w_px: f32,
    #[serde(default)]
    pub h_px: f32,

    #[serde(default)]
    pub manual_anchor: bool,

    #[serde(default)]
    pub use_anchor_offsets: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BoxRectPx {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RedactBox {
    pub fn from_drag(start: (f32, f32), end: (f32, f32), window: RectPx) -> Self {
        let left = start.0.min(end.0);
        let right = start.0.max(end.0);
        let top = start.1.min(end.1);
        let bottom = start.1.max(end.1);

        let mut b = Self {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            anchor: Anchor::TopLeft,
            anchor_offset_x_px: 0.0,
            anchor_offset_y_px: 0.0,
            w_px: right - left,
            h_px: bottom - top,
            manual_anchor: false,
            use_anchor_offsets: true,
        };

        b.set_position_and_pick_anchor(left, top, window);
        b
    }

    fn set_position_and_pick_anchor(&mut self, box_left: f32, box_top: f32, window: RectPx) {
        let wl = window.left as f32;
        let wt = window.top as f32;
        let wr = window.right as f32;
        let wb = window.bottom as f32;

        let corners = [
            (Anchor::TopLeft,     dist2(box_left,              box_top,             wl, wt)),
            (Anchor::TopRight,    dist2(box_left + self.w_px,  box_top,             wr, wt)),
            (Anchor::BottomLeft,  dist2(box_left,              box_top + self.h_px, wl, wb)),
            (Anchor::BottomRight, dist2(box_left + self.w_px,  box_top + self.h_px, wr, wb)),
        ];

        let mut best = &corners[0];
        for c in &corners {
            if c.1 < best.1 {
                best = c;
            }
        }
        self.anchor = best.0.clone();
        self.compute_offsets_from_position(box_left, box_top, window);
    }

    pub fn compute_offsets_from_position(&mut self, box_left: f32, box_top: f32, window: RectPx) {
        let (ax, ay) = anchor_corner(window, self.anchor.clone());

        match self.anchor {
            Anchor::TopLeft => {
                self.anchor_offset_x_px = box_left - ax;
                self.anchor_offset_y_px = box_top - ay;
            }
            Anchor::TopRight => {
                self.anchor_offset_x_px = ax - (box_left + self.w_px);
                self.anchor_offset_y_px = box_top - ay;
            }
            Anchor::BottomLeft => {
                self.anchor_offset_x_px = box_left - ax;
                self.anchor_offset_y_px = ay - (box_top + self.h_px);
            }
            Anchor::BottomRight => {
                self.anchor_offset_x_px = ax - (box_left + self.w_px);
                self.anchor_offset_y_px = ay - (box_top + self.h_px);
            }
        }

        self.use_anchor_offsets = true;
    }

    pub fn to_pixels(&self, window: RectPx) -> BoxRectPx {
        if self.use_anchor_offsets {
            let (ax, ay) = anchor_corner(window, self.anchor.clone());

            let (bx, by) = match self.anchor {
                Anchor::TopLeft => (
                    ax + self.anchor_offset_x_px,
                    ay + self.anchor_offset_y_px,
                ),
                Anchor::TopRight => (
                    ax - self.anchor_offset_x_px - self.w_px,
                    ay + self.anchor_offset_y_px,
                ),
                Anchor::BottomLeft => (
                    ax + self.anchor_offset_x_px,
                    ay - self.anchor_offset_y_px - self.h_px,
                ),
                Anchor::BottomRight => (
                    ax - self.anchor_offset_x_px - self.w_px,
                    ay - self.anchor_offset_y_px - self.h_px,
                ),
            };

            return BoxRectPx {
                x: bx,
                y: by,
                w: self.w_px,
                h: self.h_px,
            };
        }

        let ww = window.width().max(1) as f32;
        let wh = window.height().max(1) as f32;
        BoxRectPx {
            x: window.left as f32 + self.x * ww,
            y: window.top as f32 + self.y * wh,
            w: self.w * ww,
            h: self.h * wh,
        }
    }

    pub fn hit_test(&self, window: RectPx, point: (f32, f32)) -> bool {
        let p = self.to_pixels(window);
        point.0 >= p.x && point.0 <= p.x + p.w && point.1 >= p.y && point.1 <= p.y + p.h
    }

    #[allow(dead_code)]
    pub fn recompute_anchor(&mut self, window: RectPx) {
        let p = self.to_pixels(window);
        self.set_position_and_pick_anchor(p.x, p.y, window);
    }

    #[allow(dead_code)]
    pub fn recompute_anchor_offsets(&mut self, window: RectPx) {
        let p = self.to_pixels(window);
        self.compute_offsets_from_position(p.x, p.y, window);
    }

    pub fn set_anchor(&mut self, new_anchor: Anchor, window: RectPx) {
        let p = self.to_pixels(window);
        self.anchor = new_anchor;
        self.compute_offsets_from_position(p.x, p.y, window);
    }

    #[allow(dead_code)]
    pub fn move_by_pixels(&mut self, window: RectPx, dx: f32, dy: f32) {
        let p = self.to_pixels(window);
        let new_left = p.x + dx;
        let new_top = p.y + dy;

        if !self.manual_anchor {
            self.set_position_and_pick_anchor(new_left, new_top, window);
        } else {
            self.compute_offsets_from_position(new_left, new_top, window);
        }
    }

    pub fn resize_to_pixels(
        &mut self,
        window: RectPx,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
    ) {
        let l = left.min(right);
        let t = top.min(bottom);
        let r = left.max(right);
        let b = top.max(bottom);

        self.w_px = r - l;
        self.h_px = b - t;

        if !self.manual_anchor {
            self.set_position_and_pick_anchor(l, t, window);
        } else {
            self.compute_offsets_from_position(l, t, window);
        }
    }

    #[allow(dead_code)]
    pub fn normalize(&mut self) {
        if !self.use_anchor_offsets {
            self.x = self.x.clamp(0.0, 1.0);
            self.y = self.y.clamp(0.0, 1.0);
            self.w = self.w.clamp(0.0, 1.0 - self.x);
            self.h = self.h.clamp(0.0, 1.0 - self.y);
        }
    }
}

fn dist2(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x1 - x2;
    let dy = y1 - y2;
    dx * dx + dy * dy
}

pub fn anchor_corner(window: RectPx, anchor: Anchor) -> (f32, f32) {
    match anchor {
        Anchor::TopLeft => (window.left as f32, window.top as f32),
        Anchor::TopRight => (window.right as f32, window.top as f32),
        Anchor::BottomLeft => (window.left as f32, window.bottom as f32),
        Anchor::BottomRight => (window.right as f32, window.bottom as f32),
    }
}
