use super::*;
use crate::history::{Edit, History};
use crate::model::{Item, Style};

const WHITE: Color = Color::rgb(255, 255, 255);
const RED: Color = Color::rgb(255, 0, 0);

fn style(c: Color, w: f32) -> Style {
    Style { color: c, width: w }
}

fn doc_with(w: u32, h: u32, annotations: Vec<Annotation>) -> Document {
    let mut doc = Document::new(w, h);
    let mut hist = History::default();
    for annotation in annotations {
        let id = doc.fresh_id();
        let index = doc.items.len();
        hist.commit(
            &mut doc,
            Edit::Add {
                index,
                item: Item { id, annotation },
            },
        );
    }
    doc
}

fn white(w: u32, h: u32) -> RgbaImage {
    RgbaImage::filled(w, h, WHITE)
}

/// One character per `step`×`step` cell, from the cell's centre pixel.
fn ascii(img: &RgbaImage, step: u32) -> String {
    let mut out = String::new();
    for y in (0..img.height).step_by(step as usize) {
        for x in (0..img.width).step_by(step as usize) {
            let p = img.pixel(
                (x + step / 2).min(img.width - 1),
                (y + step / 2).min(img.height - 1),
            );
            let (r, g, b) = (i32::from(p.r), i32::from(p.g), i32::from(p.b));
            out.push(match () {
                () if r > 230 && g > 230 && b > 230 => '.',
                () if r < 40 && g < 40 && b < 40 => '#',
                () if r > g + 60 && r > b + 60 => 'R',
                () if g > r + 60 && g > b + 60 => 'G',
                () if b > r + 60 && b > g + 60 => 'B',
                () if r > 200 && g > 200 && b < 120 => 'Y',
                () => '+',
            });
        }
        out.push('\n');
    }
    out
}

#[test]
fn no_annotations_gives_back_the_base() {
    let base = white(8, 4);
    assert_eq!(render(&base, &Document::new(8, 4)), base);
}

#[test]
fn a_line_has_the_requested_colour_and_thickness() {
    let doc = doc_with(
        40,
        20,
        vec![Annotation::Line {
            from: Point::new(2.0, 10.0),
            to: Point::new(38.0, 10.0),
            style: style(RED, 4.0),
        }],
    );
    let img = render(&white(40, 20), &doc);
    assert_eq!(img.pixel(20, 10), RED);
    assert_eq!(img.pixel(20, 9), RED);
    assert_eq!(img.pixel(20, 5), WHITE);
    assert_eq!(img.pixel(20, 15), WHITE);
}

#[test]
fn outline_and_filled_rectangles() {
    let rect = Rect::new(4.0, 4.0, 30.0, 20.0);
    let outline = render(
        &white(40, 30),
        &doc_with(
            40,
            30,
            vec![Annotation::Rect {
                rect,
                style: style(RED, 2.0),
                filled: false,
            }],
        ),
    );
    let filled = render(
        &white(40, 30),
        &doc_with(
            40,
            30,
            vec![Annotation::Rect {
                rect,
                style: style(RED, 2.0),
                filled: true,
            }],
        ),
    );
    assert_eq!(outline.pixel(19, 14), WHITE);
    assert_eq!(filled.pixel(19, 14), RED);
    assert_eq!(outline.pixel(4, 14), RED);
}

#[test]
fn ellipse_is_round() {
    let doc = doc_with(
        40,
        40,
        vec![Annotation::Ellipse {
            rect: Rect::new(2.0, 2.0, 36.0, 36.0),
            style: style(RED, 2.0),
            filled: true,
        }],
    );
    let img = render(&white(40, 40), &doc);
    assert_eq!(img.pixel(20, 20), RED);
    assert_eq!(img.pixel(3, 3), WHITE); // the corner of the bounding box stays empty
}

#[test]
fn highlighter_multiplies() {
    let yellow = Color::rgb(255, 255, 0).with_alpha(255);
    let mut base = white(40, 10);
    // a black "text" pixel row in the middle
    for x in 10..30 {
        let o = (5 * 40 + x) * 4;
        base.data[o..o + 3].fill(0);
    }
    let doc = doc_with(
        40,
        10,
        vec![Annotation::Highlight {
            points: vec![Point::new(2.0, 5.0), Point::new(38.0, 5.0)],
            color: yellow,
            width: 8.0,
        }],
    );
    let img = render(&base, &doc);
    assert_eq!(img.pixel(5, 5), Color::rgb(255, 255, 0)); // white × yellow
    assert_eq!(img.pixel(20, 5), Color::rgb(0, 0, 0)); // black stays black: the text remains readable
}

#[test]
fn blur_and_pixelate_destroy_detail() {
    let mut base = white(32, 32);
    for y in 0..32usize {
        for x in 0..32usize {
            if (x + y) % 2 == 0 {
                let o = (y * 32 + x) * 4;
                base.data[o..o + 3].fill(0);
            }
        }
    }
    let area = Rect::new(8.0, 8.0, 16.0, 16.0);
    let blurred = render(
        &base,
        &doc_with(
            32,
            32,
            vec![Annotation::Blur {
                rect: area,
                radius: 3.0,
            }],
        ),
    );
    let mosaic = render(
        &base,
        &doc_with(
            32,
            32,
            vec![Annotation::Pixelate {
                rect: area,
                block: 8,
            }],
        ),
    );
    for img in [&blurred, &mosaic] {
        let v = i32::from(img.pixel(15, 15).r);
        assert!((v - 128).abs() < 20, "{v}");
        assert_eq!(img.pixel(0, 0), base.pixel(0, 0)); // outside the area: untouched
    }
}

#[test]
fn a_blur_covers_what_was_drawn_before_it() {
    // A secret drawn, then blurred over: the red must not survive as a sharp edge.
    let doc = doc_with(
        40,
        20,
        vec![
            Annotation::Rect {
                rect: Rect::new(10.0, 5.0, 20.0, 10.0),
                style: style(RED, 1.0),
                filled: true,
            },
            Annotation::Pixelate {
                rect: Rect::new(0.0, 0.0, 40.0, 20.0),
                block: 40,
            },
        ],
    );
    let img = render(&white(40, 20), &doc);
    assert_eq!(img.pixel(0, 0), img.pixel(20, 10));
}

#[test]
fn text_is_drawn_inside_its_bounds() {
    let a = Annotation::Text {
        pos: Point::new(4.0, 4.0),
        text: "Hi".into(),
        size: 20.0,
        color: Color::rgb(0, 0, 0),
        background: None,
    };
    let b = a.bounds();
    let img = render(&white(60, 40), &doc_with(60, 40, vec![a]));
    let dark = (0..40)
        .flat_map(|y| (0..60).map(move |x| (x, y)))
        .filter(|&(x, y)| img.pixel(x, y).r < 100)
        .collect::<Vec<_>>();
    assert!(dark.len() > 20, "{}", dark.len());
    assert!(
        dark.iter()
            .all(|&(x, y)| b.inflate(2.0).contains(Point::new(x as f32, y as f32)))
    );
}

#[test]
fn text_background_fills_behind_the_glyphs() {
    let a = Annotation::Text {
        pos: Point::new(10.0, 10.0),
        text: "I".into(),
        size: 20.0,
        color: Color::rgb(0, 0, 0),
        background: Some(RED),
    };
    let img = render(&white(60, 50), &doc_with(60, 50, vec![a]));
    assert_eq!(img.pixel(10, 12), RED);
}

#[test]
fn marker_shows_its_number_in_a_contrasting_colour() {
    let a = Annotation::Marker {
        center: Point::new(20.0, 20.0),
        number: 7,
        color: RED,
        radius: 14.0,
    };
    let img = render(&white(40, 40), &doc_with(40, 40, vec![a]));
    assert_eq!(img.pixel(9, 20), RED);
    assert_eq!(img.pixel(1, 1), WHITE);
    let whites = (12..28)
        .flat_map(|y| (12..28).map(move |x| (x, y)))
        .filter(|&(x, y)| img.pixel(x, y).g > 200)
        .count();
    assert!(whites > 8, "{whites}");
}

#[test]
fn crop_changes_the_output_size_and_content() {
    let mut base = white(20, 10);
    base.data[(3 * 20 + 5) * 4..(3 * 20 + 5) * 4 + 3].fill(0);
    let mut doc = Document::new(20, 10);
    doc.crop = Some(Rect::new(5.0, 3.0, 4.0, 2.0));
    let img = render(&base, &doc);
    assert_eq!((img.width, img.height), (4, 2));
    assert_eq!(img.pixel(0, 0), Color::rgb(0, 0, 0));
    assert_eq!(img.pixel(1, 0), WHITE);
}

#[test]
fn annotations_outside_the_image_do_not_panic() {
    let doc = doc_with(
        10,
        10,
        vec![
            Annotation::Line {
                from: Point::new(-50.0, -50.0),
                to: Point::new(500.0, 500.0),
                style: style(RED, 3.0),
            },
            Annotation::Text {
                pos: Point::new(-30.0, 8.0),
                text: "clipped".into(),
                size: 30.0,
                color: RED,
                background: None,
            },
            Annotation::Blur {
                rect: Rect::new(-10.0, -10.0, 100.0, 100.0),
                radius: 5.0,
            },
            Annotation::Pixelate {
                rect: Rect::new(200.0, 200.0, 5.0, 5.0),
                block: 3,
            },
            Annotation::Marker {
                center: Point::new(-5.0, 50.0),
                number: 1,
                color: RED,
                radius: 8.0,
            },
            Annotation::Arrow {
                from: Point::new(1.0, 1.0),
                to: Point::new(1.0, 1.0),
                style: style(RED, 2.0),
            },
            Annotation::Pen {
                points: vec![],
                style: style(RED, 2.0),
            },
        ],
    );
    let img = render(&white(10, 10), &doc);
    assert_eq!((img.width, img.height), (10, 10));
}

#[test]
fn from_bgra_swaps_channels_and_skips_padding() {
    // 1x2 image, stride 8: (B,G,R,A) = (1,2,3,0) then (4,5,6,0), each row padded by 4 bytes
    let bgra = [1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 0, 9, 9, 9, 9];
    let img = RgbaImage::from_bgra(1, 2, 8, &bgra).unwrap();
    assert_eq!(img.data, vec![3, 2, 1, 255, 6, 5, 4, 255]);
    assert!(RgbaImage::from_bgra(2, 2, 4, &bgra).is_none());
}

#[test]
fn reference_scene() {
    let doc = doc_with(
        96,
        48,
        vec![
            Annotation::Rect {
                rect: Rect::new(4.0, 4.0, 40.0, 24.0),
                style: style(Color::rgb(0x1E, 0x88, 0xE5), 4.0),
                filled: false,
            },
            Annotation::Ellipse {
                rect: Rect::new(50.0, 4.0, 40.0, 24.0),
                style: style(Color::rgb(0x43, 0xA0, 0x47), 4.0),
                filled: true,
            },
            Annotation::Arrow {
                from: Point::new(6.0, 44.0),
                to: Point::new(60.0, 36.0),
                style: style(RED, 3.0),
            },
            Annotation::Line {
                from: Point::new(66.0, 44.0),
                to: Point::new(92.0, 44.0),
                style: style(Color::rgb(0, 0, 0), 3.0),
            },
            Annotation::Marker {
                center: Point::new(20.0, 16.0),
                number: 1,
                color: RED,
                radius: 8.0,
            },
        ],
    );
    insta::assert_snapshot!(ascii(&render(&white(96, 48), &doc), 2));
}

#[test]
fn a_bgra_screen_lands_at_its_place_in_the_desktop() {
    // A 2x2 screen (rows 12 bytes apart) written at (1, 1) of a 4x3 desktop.
    let src = [
        1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, //
        7, 8, 9, 0, 10, 11, 12, 0, 99, 99, 99, 99,
    ];
    let mut dst = vec![0u8; 4 * 3 * 4];
    let row = 4 * 4;
    super::bgra_to_rgba(&src, 12, &mut dst[row + 4..], row, (2, 2));
    assert_eq!(&dst[row + 4..row + 12], &[3, 2, 1, 255, 6, 5, 4, 255]);
    assert_eq!(
        &dst[2 * row + 4..2 * row + 12],
        &[9, 8, 7, 255, 12, 11, 10, 255]
    );
    assert!(dst[..row + 4].iter().all(|&b| b == 0));
    assert!(dst[2 * row + 12..].iter().all(|&b| b == 0));
}
