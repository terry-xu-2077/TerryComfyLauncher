//! Physical-pixel geometry shared by the Win10 window mask and alpha frame.
// Logical CSS pixels; keep styles.css --window-radius in sync.
pub const CORNER_RADIUS: i32 = 16;
const MASK_INSET: f64 = 0.75;
const SAMPLES: i32 = 4;

pub struct FrameGeometry {
    w: i32,
    h: i32,
    pub radius: f64,
    inner_band: f64,
    inner_ramp: f64,
    outer_fade: f64,
}

impl FrameGeometry {
    pub fn new(w: i32, h: i32, scale: f64, width: f64) -> Self {
        let inner_ramp = (0.5 * scale).max(1.0);
        Self {
            w,
            h,
            radius: (CORNER_RADIUS as f64 * scale)
                .round()
                .min(w.min(h) as f64 / 2.0),
            // Keep the binary mask inside the fully opaque part of the frame,
            // including the footprint of the subpixel samples. Thin CSS strokes
            // must not shrink this safety band or expose desktop through the ramp.
            inner_band: (width * scale).max(inner_ramp + 1.5),
            inner_ramp,
            outer_fade: 1.2 * scale,
        }
    }

    fn distance(&self, x: f64, y: f64) -> f64 {
        let qx = (x - self.w as f64 / 2.0).abs() - self.w as f64 / 2.0 + self.radius;
        let qy = (y - self.h as f64 / 2.0).abs() - self.h as f64 / 2.0 + self.radius;
        qx.max(qy).min(0.0) + qx.max(0.0).hypot(qy.max(0.0)) - self.radius
    }

    /// Exclusive horizontal span of pixel centers inside the binary mask.
    /// Unlike CreateRoundRectRgn, this is symmetric and uses the alpha path's
    /// exact pixel-center convention, including at the right and bottom edges.
    pub fn row_span(&self, y: i32) -> Option<(i32, i32)> {
        let cy = y as f64 + 0.5;
        if cy < MASK_INSET || cy > self.h as f64 - MASK_INSET {
            return None;
        }
        let dy = (self.radius - cy)
            .max(cy - (self.h as f64 - self.radius))
            .max(0.0);
        let r = (self.radius - MASK_INSET).max(0.0);
        let edge = if dy > 0.0 {
            self.radius - (r * r - dy * dy).max(0.0).sqrt()
        } else {
            MASK_INSET
        };
        let left = (edge - 0.5).ceil() as i32;
        (left < self.w - left).then_some((left, self.w - left))
    }

    fn profile(&self, t: f64) -> f64 {
        let inner = ((t + self.inner_band) / self.inner_ramp).clamp(0.0, 1.0);
        let outer = (1.0 - t / self.outer_fade).clamp(0.0, 1.0);
        inner.min(outer)
    }

    pub fn pixel_alpha(&self, x: i32, y: i32) -> f64 {
        let t = self.distance(x as f64 + 0.5, y as f64 + 0.5);
        // Distance is 1-Lipschitz; all samples are within sqrt(2)/2 pixels.
        // Only the two transition bands require supersampling.
        if t < -self.inner_band - 0.71 || t > self.outer_fade + 0.71 {
            return 0.0;
        }
        if t >= -self.inner_band + self.inner_ramp + 0.71 && t <= -0.71 {
            return 1.0;
        }
        let mut sum = 0.0;
        for sy in 0..SAMPLES {
            for sx in 0..SAMPLES {
                sum += self.profile(self.distance(
                    x as f64 + (sx as f64 + 0.5) / SAMPLES as f64,
                    y as f64 + (sy as f64 + 0.5) / SAMPLES as f64,
                ));
            }
        }
        sum / (SAMPLES * SAMPLES) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_and_feather_do_not_expose_hard_edges_at_supported_dpis() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            for width in [0.5, 1.0, 1.5, 6.0] {
                let g = FrameGeometry::new(
                    (721.0 * scale) as i32,
                    (441.0 * scale) as i32,
                    scale,
                    width,
                );
                for y in 0..g.radius as i32 + 3 {
                    for x in 0..g.radius as i32 + 3 {
                        let inside = g.row_span(y).is_some_and(|(l, r)| x >= l && x < r);
                        assert_eq!(g.row_span(y), g.row_span(g.h - 1 - y));
                        assert_eq!(
                            inside,
                            g.row_span(y)
                                .is_some_and(|(l, r)| g.w - 1 - x >= l && g.w - 1 - x < r)
                        );
                        assert_eq!(
                            inside,
                            g.distance(x as f64 + 0.5, y as f64 + 0.5) <= -MASK_INSET
                        );
                        for sy in 0..SAMPLES {
                            for sx in 0..SAMPLES {
                                let t = g.distance(
                                    x as f64 + (sx as f64 + 0.5) / 4.0,
                                    y as f64 + (sy as f64 + 0.5) / 4.0,
                                );
                                // Outer feather must blend only with desktop; inner
                                // feather must blend only with opaque window content.
                                assert!(
                                    !inside || t <= 0.0,
                                    "outer leak: {scale}, {width}, {x}, {y}"
                                );
                                assert!(
                                    inside || t >= -g.inner_band + g.inner_ramp,
                                    "inner leak: {scale}, {width}, {x}, {y}"
                                );
                            }
                        }
                        let a = g.pixel_alpha(x, y);
                        assert!((a - g.pixel_alpha(g.w - 1 - x, y)).abs() < 1e-10);
                        assert!((a - g.pixel_alpha(x, g.h - 1 - y)).abs() < 1e-10);
                    }
                }
            }
        }
    }

    #[test]
    fn corner_coverage_matches_high_resolution_area_reference() {
        for scale in [1.0, 1.25, 1.75, 2.0] {
            let g = FrameGeometry::new(720, 440, scale, 1.0);
            for y in 0..g.radius as i32 + 2 {
                for x in 0..g.radius as i32 + 2 {
                    let mut reference = 0.0;
                    for sy in 0..32 {
                        for sx in 0..32 {
                            reference += g.profile(g.distance(
                                x as f64 + (sx as f64 + 0.5) / 32.0,
                                y as f64 + (sy as f64 + 0.5) / 32.0,
                            ));
                        }
                    }
                    assert!((g.pixel_alpha(x, y) - reference / 1024.0).abs() < 0.015);
                }
            }
        }
    }
}
