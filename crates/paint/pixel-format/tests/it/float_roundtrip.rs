//! `.pcraft` is lossless for f64: every coordinate reloads bit-for-bit.

use studio_pixel_color::{ColorMode, SampleType};
use studio_pixel_doc::{Document, Layer, LayerContent, Size};

#[test]
fn f64_coordinates_reload_bit_exact() {
    let x = 11.183221215612905_f64;
    let y = 0.1 + 0.2;
    let mut doc = Document::new("Floats", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    doc.layers.push(Layer::new(
        "Shape",
        LayerContent::Shape(studio_pixel_doc::ShapeLayer {
            path: studio_pixel_doc::Path::new(vec![studio_pixel_doc::Subpath::polygon(&[(x, y), (40.0, 10.0), (25.0, 40.0)])]),
            ..Default::default()
        }),
    ));
    doc.guides.horizontal.push(x);
    doc.guides.vertical.push(y);
    doc.ruler_origin = [x, y];
    let bytes = studio_pixel_format::save_to_bytes(&doc, &Default::default()).unwrap();
    let back = studio_pixel_format::load_from_bytes(&bytes).unwrap();
    assert_eq!(back.guides.horizontal[0].to_bits(), x.to_bits());
    assert_eq!(back.guides.vertical[0].to_bits(), y.to_bits());
    assert_eq!(back.ruler_origin.map(f64::to_bits), [x, y].map(f64::to_bits));
    assert_eq!(back, doc);
}
