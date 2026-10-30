//! Lottie animations (Discord's standard stickers), drawn on the CPU: velato reads and evaluates the
//! animation, and vello_cpu, which egui already uses for text, rasterises each frame.

use std::time::Duration;

use egui::ColorImage;
use velato::model::{ImageAsset, fixed};
use vello_cpu::kurbo::{Affine, BezPath, Shape};
use vello_cpu::peniko::{BlendMode, Brush, Fill};
use vello_cpu::{PaintType, Pixmap, RenderContext, Resources};

pub struct Animation {
    composition: velato::Composition,
    renderer: velato::Renderer,
}

/// Draws velato's shapes into a vello_cpu context.
struct Sink<'a> {
    ctx: &'a mut RenderContext,
}

fn path(shape: &impl Shape) -> BezPath {
    shape.to_path(0.1)
}

impl velato::RenderSink for Sink<'_> {
    fn push_layer(&mut self, blend: impl Into<BlendMode>, alpha: f32, transform: Affine, shape: &impl Shape) {
        self.ctx.set_transform(transform);
        self.ctx.push_layer(Some(&path(shape)), Some(blend.into()), Some(alpha), None, None);
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl Shape) {
        self.ctx.set_transform(transform);
        self.ctx.push_clip_layer(&path(shape));
    }

    fn pop_layer(&mut self) {
        self.ctx.pop_layer();
    }

    fn draw(&mut self, stroke: Option<&fixed::Stroke>, transform: Affine, brush: &fixed::Brush, shape: &impl Shape) {
        let paint: PaintType = match brush {
            Brush::Solid(c) => PaintType::Solid(*c),
            Brush::Gradient(g) => PaintType::Gradient(g.clone()),
            // Stickers don't embed images.
            Brush::Image(_) => return,
        };
        self.ctx.set_transform(transform);
        self.ctx.set_paint(paint);
        let p = path(shape);
        match stroke {
            Some(s) => {
                self.ctx.set_stroke(s.clone());
                self.ctx.stroke_path(&p);
            }
            None => {
                self.ctx.set_fill_rule(Fill::NonZero);
                self.ctx.fill_path(&p);
            }
        }
    }

    fn draw_image(&mut self, _: &ImageAsset, _: Affine, _: f64) {}
}

impl Animation {
    pub fn parse(json: &[u8]) -> Option<Animation> {
        let composition = velato::Composition::from_slice(json).ok()?;
        (composition.width > 0 && composition.height > 0).then(|| Animation { composition, renderer: velato::Renderer::new() })
    }

    /// The frame shown `elapsed` into the animation, looping.
    pub fn frame_at(&self, elapsed: Duration) -> f64 {
        let frames = &self.composition.frames;
        let length = (frames.end - frames.start).max(1.0);
        frames.start + (elapsed.as_secs_f64() * self.composition.frame_rate.max(1.0)) % length
    }

    /// How long one frame is shown.
    pub fn frame_time(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.composition.frame_rate.clamp(1.0, 120.0))
    }

    /// Renders `frame` into a `size`×`size` image, fitted and centred.
    pub fn render(&mut self, frame: f64, size: u16) -> ColorImage {
        let (w, h) = (self.composition.width as f64, self.composition.height as f64);
        let scale = size as f64 / w.max(h);
        let offset = ((size as f64 - w * scale) / 2.0, (size as f64 - h * scale) / 2.0);
        let transform = Affine::translate(offset) * Affine::scale(scale);
        let mut ctx = RenderContext::new(size, size);
        self.renderer.append(&self.composition, frame, transform, 1.0, &mut Sink { ctx: &mut ctx });
        ctx.flush();
        let mut pixmap = Pixmap::new(size, size);
        ctx.render(&mut pixmap, &mut Resources::new());
        ColorImage::from_rgba_premultiplied([size as usize, size as usize], pixmap.data_as_u8_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 100×100 animation of a red square, 50 wide, sliding from x=0 to x=50 over 10 frames.
    const SQUARE: &str = r#"{"v":"5.7.0","fr":10,"ip":0,"op":10,"w":100,"h":100,"layers":[{"ty":4,"ind":1,"ip":0,"op":10,"st":0,
      "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},"p":{"a":1,"k":[{"t":0,"s":[25,50],"i":{"x":1,"y":1},"o":{"x":0,"y":0}},{"t":10,"s":[75,50]}]},
            "a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]}},
      "shapes":[{"ty":"rc","p":{"a":0,"k":[0,0]},"s":{"a":0,"k":[50,50]},"r":{"a":0,"k":0}},
                {"ty":"fl","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100}}]}]}"#;

    fn pixel(img: &ColorImage, x: usize, y: usize) -> [u8; 4] {
        img.pixels[y * img.size[0] + x].to_array()
    }

    #[test]
    fn renders_and_animates() {
        let mut a = Animation::parse(SQUARE.as_bytes()).expect("parses");
        let first = a.render(0.0, 100);
        assert_eq!(first.size, [100, 100]);
        // Frame 0: the square covers x 0–50.
        assert_eq!(pixel(&first, 10, 50), [255, 0, 0, 255]);
        assert_eq!(pixel(&first, 90, 50)[3], 0);
        // Frame 10 (just before the end): moved right.
        let last = a.render(9.99, 100);
        assert_eq!(pixel(&last, 90, 50), [255, 0, 0, 255]);
        assert_eq!(pixel(&last, 10, 50)[3], 0);
        // Looping time to frames.
        assert_eq!(a.frame_at(Duration::from_millis(1500)), 5.0);
    }

    /// Renders a Lottie file (`SWIFTCORD_TEST_LOTTIE`) to a PNG next to it, for looking at.
    #[test]
    fn renders_a_local_file() {
        let Ok(path) = std::env::var("SWIFTCORD_TEST_LOTTIE") else { return };
        let mut a = Animation::parse(&std::fs::read(&path).unwrap()).expect("parses");
        let t = std::time::Instant::now();
        let img = a.render(a.composition.frames.start + 10.0, 160);
        println!("rendered in {:?}", t.elapsed());
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        let png = crate::platform::wic::encode_png(&crate::platform::wic::Rgba { width: 160, height: 160, pixels: rgba }).unwrap();
        std::fs::write(format!("{path}.png"), png).unwrap();
    }
}
