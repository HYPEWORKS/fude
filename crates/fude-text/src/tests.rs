use super::*;
use fude_render::{Canvas, ImageStore, Rect, rasterize};

const ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

fn layout(ts: &mut TextSystem, text: &str, style: &TextStyle) -> LineLayout {
    let mut out = LineLayout::default();
    ts.layout_line(text, style, &mut out);
    out
}

#[test]
fn fast_path_matches_shaper() {
    let mut ts = TextSystem::new();
    let style = TextStyle::new(FontId::MONO, 15.0);
    let fast = layout(&mut ts, ASCII, &style);

    // Force the same text through harfrust with the same features.
    let face = &ts.faces[0];
    let mut buffer = harfrust::Buffer::new();
    let mut shaped = LineLayout::default();
    let scale = style.px / face.upem;
    let width = layout::shape_segment_for_test(
        face,
        &ts.no_ligatures,
        &mut buffer,
        ASCII,
        scale,
        &mut shaped,
    );

    assert_eq!(fast.glyphs.len(), shaped.glyphs.len());
    for (a, b) in fast.glyphs.iter().zip(&shaped.glyphs) {
        assert_eq!(
            a.glyph, b.glyph,
            "glyph id differs at cluster {}",
            a.cluster
        );
        assert_eq!(a.cluster, b.cluster);
        assert!(
            (a.x - b.x).abs() < 1e-3,
            "x differs at cluster {}",
            a.cluster
        );
        assert_eq!(b.y, 0.0);
    }
    assert!((fast.width - width).abs() < 1e-3);
}

#[test]
fn monospace_advance_and_metrics() {
    let ts = TextSystem::new();
    let m = ts.metrics(FontId::MONO, 10.0);
    // JetBrains Mono: 1000 units per em, 600-unit advances.
    assert!(
        (m.space_advance - 6.0).abs() < 1e-4,
        "space advance {}",
        m.space_advance
    );
    assert!(m.ascent > 0.0 && m.descent > 0.0);
}

#[test]
fn tabs_advance_to_next_stop() {
    let mut ts = TextSystem::new();
    let style = TextStyle::new(FontId::MONO, 10.0); // 6 px columns, stops every 24 px
    let l = layout(&mut ts, "a\tb\t\tc", &style);
    let xs: Vec<f32> = l.glyphs.iter().map(|g| g.x).collect();
    assert_eq!(xs, vec![0.0, 24.0, 72.0]);
    let clusters: Vec<u32> = l.glyphs.iter().map(|g| g.cluster).collect();
    assert_eq!(clusters, vec![0, 2, 5]);
}

#[test]
fn combining_marks_go_through_the_shaper() {
    let mut ts = TextSystem::new();
    let style = TextStyle::new(FontId::MONO, 14.0);
    let text = "cafe\u{301} ─│┌ 筆";
    let l = layout(&mut ts, text, &style);
    assert!(!l.glyphs.is_empty());
    assert!(l.glyphs.windows(2).all(|w| w[0].cluster <= w[1].cluster));
    assert!(l.glyphs.iter().all(|g| (g.cluster as usize) < text.len()));
}

#[test]
fn ligatures_change_glyphs_only_when_enabled() {
    let mut ts = TextSystem::new();
    let mut style = TextStyle::new(FontId::MONO, 14.0);
    let plain = layout(&mut ts, "a -> b != c", &style);
    style.ligatures = true;
    let lig = layout(&mut ts, "a -> b != c", &style);
    let ids = |l: &LineLayout| l.glyphs.iter().map(|g| g.glyph).collect::<Vec<_>>();
    assert_ne!(
        ids(&plain),
        ids(&lig),
        "JetBrains Mono should form ligatures for -> and !="
    );
    // Monospace either way: total width is unchanged.
    assert!((plain.width - lig.width).abs() < 1e-3);
}

/// Sum of coverage when drawing `text` white on black.
fn ink(ts: &mut TextSystem, text: &str, style: &TextStyle) -> u64 {
    let (w, h) = (400, 40);
    let mut px = vec![0u32; w * h];
    let mut canvas = Canvas::new(&mut px, w, h);
    let mut list = DrawList::new();
    let mut images = ImageStore::new();
    let l = layout(ts, text, style);
    ts.draw_line(
        &l,
        style,
        2.0,
        30,
        Color::rgb(0xFFFFFF),
        (0.0, w as f32),
        &mut list,
        &mut images,
    );
    rasterize(
        &list,
        &images,
        &mut canvas,
        Rect::new(0, 0, w as i32, h as i32),
    );
    px.iter().map(|p| (p & 0xFF) as u64).sum()
}

#[test]
fn bold_draws_more_ink_than_regular() {
    let mut ts = TextSystem::new();
    let regular = ink(&mut ts, "Hello, fude!", &TextStyle::new(FontId::MONO, 20.0));
    let bold = ink(
        &mut ts,
        "Hello, fude!",
        &TextStyle::new(FontId::MONO_BOLD, 20.0),
    );
    assert!(regular > 0);
    assert!(
        bold as f64 > regular as f64 * 1.2,
        "regular={regular} bold={bold}"
    );
}

#[test]
fn glyphs_are_cached() {
    let mut ts = TextSystem::new();
    let style = TextStyle::new(FontId::MONO, 16.0);
    ink(&mut ts, "aaaa", &style);
    let after_first = ts.cached_glyphs();
    ink(&mut ts, "aaaa", &style);
    assert_eq!(ts.cached_glyphs(), after_first);
    assert!(after_first <= 4, "one entry per subpixel offset at most");
}

/// Regression: swash reuses its scratch buffer without clearing it, so a glyph rasterized
/// after another one must not contain the previous glyph's pixels.
#[test]
fn glyph_bitmaps_do_not_bleed_into_each_other() {
    let style = TextStyle::new(FontId::MONO, 18.0);
    let mut fresh = TextSystem::new();
    let alone = ink(&mut fresh, "l", &style);
    let mut warmed = TextSystem::new();
    ink(&mut warmed, "WM@#&%", &style);
    let after_others = ink(&mut warmed, "l", &style);
    assert_eq!(alone, after_others);
}
