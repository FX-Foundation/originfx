// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn px(img: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * 4;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

fn gen(g: &Generator, w: usize, h: usize) -> Vec<u8> {
    render(g, w, h, &mut Fonts::new()).unwrap()
}

fn rect(wp: f64, hp: f64, corner: f64, fill: Colour) -> Generator {
    Generator::Shape(Shape {
        kind: ShapeKind::Rect { corner_pct: corner },
        width_pct: wp,
        height_pct: hp,
        fill: Fill::Solid(fill),
        stroke: None,
    })
}

fn alpha_mass(img: &[u8]) -> f64 {
    img.chunks(4).map(|p| f64::from(p[3]) / 255.0).sum()
}

#[test]
fn colours_parse_and_print() {
    assert_eq!(Colour::parse("#ff8000"), Some(Colour::rgb(255, 128, 0)));
    assert_eq!(Colour::parse("f80"), Some(Colour::rgb(255, 136, 0)));
    assert_eq!(Colour::parse("#11223344").unwrap().a, 0x44);
    assert_eq!(Colour::parse("white"), Some(Colour::WHITE));
    assert_eq!(Colour::parse("transparent").unwrap().a, 0);
    assert!(Colour::parse("#12").is_none());
    assert!(Colour::parse("zzzzzz").is_none());
    assert!(Colour::parse("é€é€").is_none());
    for c in [
        Colour::rgb(1, 2, 3),
        Colour {
            r: 9,
            g: 8,
            b: 7,
            a: 6,
        },
    ] {
        assert_eq!(Colour::parse(&c.hex()), Some(c));
    }
}

#[test]
fn a_fill_covers_the_frame_and_a_gradient_runs_the_right_way() {
    let img = gen(&Generator::Fill(Fill::Solid(Colour::rgb(10, 20, 30))), 8, 6);
    assert!(img.chunks(4).all(|p| p == [10, 20, 30, 255]));
    let g = Generator::Fill(Fill::Linear {
        from: Colour::BLACK,
        to: Colour::WHITE,
        angle_deg: 0.0,
    });
    let img = gen(&g, 100, 10);
    assert!(px(&img, 100, 0, 5)[0] < 5);
    assert!(px(&img, 100, 99, 5)[0] > 250);
    assert!((i32::from(px(&img, 100, 50, 5)[0]) - 128).abs() <= 3);
    // Same colour along a column at angle 0.
    assert_eq!(px(&img, 100, 40, 0), px(&img, 100, 40, 9));
    // 90 degrees runs top to bottom.
    let g = Generator::Fill(Fill::Linear {
        from: Colour::BLACK,
        to: Colour::WHITE,
        angle_deg: 90.0,
    });
    let img = gen(&g, 10, 100);
    assert!(px(&img, 10, 5, 0)[0] < 5 && px(&img, 10, 5, 99)[0] > 250);
}

#[test]
fn a_rectangle_is_where_it_should_be_with_clean_edges() {
    // 50 % by 50 % of 100 x 100, in the middle: pixels 25..75.
    let img = gen(&rect(50.0, 50.0, 0.0, Colour::rgb(255, 0, 0)), 100, 100);
    assert_eq!(px(&img, 100, 50, 50), [255, 0, 0, 255]);
    assert_eq!(px(&img, 100, 30, 30), [255, 0, 0, 255]);
    assert_eq!(px(&img, 100, 10, 10), [0, 0, 0, 0]);
    assert_eq!(px(&img, 100, 90, 50), [0, 0, 0, 0]);
    assert!((alpha_mass(&img) - 2500.0).abs() < 1.0);
    // Transparent areas are fully clear: no stray colour.
    assert!(img.chunks(4).all(|p| p[3] != 0 || p == [0, 0, 0, 0]));
}

#[test]
fn rounded_corners_and_pills() {
    let sq = alpha_mass(&gen(&rect(50.0, 50.0, 0.0, Colour::WHITE), 200, 200));
    let rounded = alpha_mass(&gen(&rect(50.0, 50.0, 50.0, Colour::WHITE), 200, 200));
    let pill = alpha_mass(&gen(&rect(50.0, 50.0, 100.0, Colour::WHITE), 200, 200));
    assert!(rounded < sq && pill < rounded);
    // A full pill of a square is a circle: area pi r^2.
    let circle = std::f64::consts::PI * 50.0 * 50.0;
    assert!((pill - circle).abs() / circle < 0.01, "{pill} vs {circle}");
    // The corner is empty, the middle of an edge is not.
    let img = gen(&rect(50.0, 50.0, 100.0, Colour::WHITE), 200, 200);
    assert_eq!(px(&img, 200, 51, 51)[3], 0);
    assert_eq!(px(&img, 200, 100, 51)[3], 255);
}

#[test]
fn ellipse_polygon_and_star_have_the_right_area() {
    let ell = |w, h| {
        Generator::Shape(Shape {
            kind: ShapeKind::Ellipse,
            width_pct: w,
            height_pct: h,
            fill: Fill::Solid(Colour::WHITE),
            stroke: None,
        })
    };
    let m = alpha_mass(&gen(&ell(60.0, 40.0), 300, 300));
    let want = std::f64::consts::PI * 90.0 * 60.0;
    assert!((m - want).abs() / want < 0.02, "{m} vs {want}");
    let poly = |n| {
        Generator::Shape(Shape {
            kind: ShapeKind::Polygon { sides: n },
            width_pct: 100.0,
            height_pct: 100.0,
            fill: Fill::Solid(Colour::WHITE),
            stroke: None,
        })
    };
    // A regular n-gon with circumradius r has area n/2 r^2 sin(2 pi / n); r = 100 px here.
    for n in [3u32, 4, 5, 6, 8] {
        let m = alpha_mass(&gen(&poly(n), 200, 200));
        let want =
            f64::from(n) / 2.0 * 100.0 * 100.0 * (2.0 * std::f64::consts::PI / f64::from(n)).sin();
        assert!((m - want).abs() / want < 0.02, "n={n}: {m} vs {want}");
    }
    // A triangle points up: the top middle is filled, the top corners are not.
    let tri = gen(&poly(3), 200, 200);
    assert_eq!(px(&tri, 200, 100, 8)[3], 255);
    assert_eq!(px(&tri, 200, 20, 8)[3], 0);
    // A star has less area than its polygon of the same radius, and a tip at the top.
    let star = |inner| {
        Generator::Shape(Shape {
            kind: ShapeKind::Star {
                points: 5,
                inner_pct: inner,
            },
            width_pct: 100.0,
            height_pct: 100.0,
            fill: Fill::Solid(Colour::WHITE),
            stroke: None,
        })
    };
    let s = gen(&star(40.0), 200, 200);
    assert!(alpha_mass(&s) < alpha_mass(&gen(&poly(5), 200, 200)));
    assert_eq!(px(&s, 200, 100, 8)[3], 255);
    // Inner radius 100 % makes a regular decagon-like shape: a fatter star has more area.
    assert!(alpha_mass(&gen(&star(70.0), 200, 200)) > alpha_mass(&s));
    // The middle is always filled.
    assert_eq!(px(&s, 200, 100, 100)[3], 255);
}

#[test]
fn strokes_sit_on_the_edge() {
    let g = Generator::Shape(Shape {
        kind: ShapeKind::Rect { corner_pct: 0.0 },
        width_pct: 50.0,
        height_pct: 50.0,
        fill: Fill::Solid(Colour::rgb(255, 0, 0)),
        stroke: Some(Stroke {
            colour: Colour::rgb(0, 0, 255),
            width_pct: 10.0,
        }),
    });
    let img = gen(&g, 200, 200);
    // Edge at x = 50; stroke 20 px wide centred on it: 40..60.
    assert_eq!(px(&img, 200, 50, 100), [0, 0, 255, 255]);
    assert_eq!(px(&img, 200, 42, 100), [0, 0, 255, 255]);
    assert_eq!(px(&img, 200, 100, 100), [255, 0, 0, 255]);
    assert_eq!(px(&img, 200, 30, 100), [0, 0, 0, 0]);
}

#[test]
fn shape_fill_can_be_a_gradient_and_translucent() {
    let g = Generator::Shape(Shape {
        kind: ShapeKind::Rect { corner_pct: 0.0 },
        width_pct: 100.0,
        height_pct: 100.0,
        fill: Fill::Linear {
            from: Colour::rgb(255, 0, 0),
            to: Colour {
                r: 0,
                g: 0,
                b: 255,
                a: 0,
            },
            angle_deg: 0.0,
        },
        stroke: None,
    });
    let img = gen(&g, 100, 10);
    assert!(px(&img, 100, 1, 5)[3] > 245 && px(&img, 100, 1, 5)[0] > 245);
    assert!(px(&img, 100, 98, 5)[3] < 10);
}

#[test]
fn degenerate_sizes_draw_nothing_and_never_panic() {
    for g in [
        rect(0.0, 50.0, 0.0, Colour::WHITE),
        rect(50.0, -5.0, 0.0, Colour::WHITE),
    ] {
        assert!(gen(&g, 20, 20).iter().all(|&b| b == 0));
    }
    let _ = gen(&rect(900.0, 900.0, 40.0, Colour::WHITE), 1, 1);
    assert!(render(
        &rect(10.0, 10.0, 0.0, Colour::WHITE),
        0,
        0,
        &mut Fonts::new()
    )
    .unwrap()
    .is_empty());
}

fn text(s: &str) -> Text {
    Text {
        text: s.into(),
        font: None,
        size_pct: 20.0,
        colour: Colour::WHITE,
        align: Align::Centre,
        line_gap_pct: 0.0,
        tracking_pct: 0.0,
        outline: None,
        shadow: None,
    }
}

#[test]
fn text_draws_inside_the_frame_and_is_repeatable() {
    if Fonts::default_path().is_none() {
        eprintln!("no system font; skipping");
        return;
    }
    let g = Generator::Text(text("HELLO"));
    let a = gen(&g, 320, 180);
    assert_eq!(a, gen(&g, 320, 180));
    let mass = alpha_mass(&a);
    assert!(mass > 200.0, "{mass}");
    // Centred: ink sits around the middle both ways.
    let (mut minx, mut maxx, mut miny, mut maxy) = (usize::MAX, 0, usize::MAX, 0);
    for y in 0..180 {
        for x in 0..320 {
            if px(&a, 320, x, y)[3] > 0 {
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
            }
        }
    }
    let (cx, cy) = ((minx + maxx) as f64 / 2.0, (miny + maxy) as f64 / 2.0);
    assert!((cx - 160.0).abs() < 8.0, "cx {cx}");
    assert!((cy - 90.0).abs() < 14.0, "cy {cy}");
    // Bigger text, more ink; empty text, none.
    let mut big = text("HELLO");
    big.size_pct = 30.0;
    assert!(alpha_mass(&gen(&Generator::Text(big), 320, 180)) > mass * 1.5);
    assert_eq!(
        alpha_mass(&gen(&Generator::Text(text("  ")), 320, 180)),
        0.0
    );
}

#[test]
fn text_lines_alignment_outline_and_shadow() {
    if Fonts::default_path().is_none() {
        return;
    }
    let one = alpha_mass(&gen(&Generator::Text(text("Hi")), 320, 180));
    let two = alpha_mass(&gen(&Generator::Text(text("Hi\nHi")), 320, 180));
    assert!((two / one - 2.0).abs() < 0.05, "{two} {one}");
    // A short second line aligned left or right moves the ink.
    let ink_left = |a: &[u8], rows: std::ops::Range<usize>| -> usize {
        (0..320)
            .find(|&x| rows.clone().any(|y| px(a, 320, x, y)[3] > 0))
            .unwrap()
    };
    let mut l = text("WIDE LINE HERE\ni");
    l.align = Align::Left;
    let mut r = l.clone();
    r.align = Align::Right;
    let (al, ar) = (
        gen(&Generator::Text(l), 320, 180),
        gen(&Generator::Text(r), 320, 180),
    );
    assert!(ink_left(&ar, 100..140) > ink_left(&al, 100..140) + 40);
    // Outline adds ink around; a shadow adds ink shifted.
    let plain = alpha_mass(&gen(&Generator::Text(text("O")), 320, 180));
    let mut o = text("O");
    o.outline = Some(Stroke {
        colour: Colour::BLACK,
        width_pct: 3.0,
    });
    assert!(alpha_mass(&gen(&Generator::Text(o), 320, 180)) > plain * 1.15);
    let mut s = text("O");
    s.shadow = Some(Shadow {
        colour: Colour::BLACK,
        dx_pct: 3.0,
        dy_pct: 3.0,
    });
    assert!(alpha_mass(&gen(&Generator::Text(s), 320, 180)) > plain * 1.15);
}

#[test]
fn font_errors_are_clean() {
    let mut t = text("x");
    t.font = Some("/definitely/not/here.ttf".into());
    let e = render(&Generator::Text(t), 10, 10, &mut Fonts::new()).unwrap_err();
    assert!(matches!(e, Error::FontNotFound(_)));
    let dir = std::env::temp_dir().join("mg_bad_font.ttf");
    std::fs::write(&dir, b"not a font").unwrap();
    let mut t = text("x");
    t.font = Some(dir.to_string_lossy().into());
    let e = render(&Generator::Text(t), 10, 10, &mut Fonts::new()).unwrap_err();
    assert!(matches!(e, Error::FontInvalid(_)));
    let _ = std::fs::remove_file(dir);
    assert_eq!(Align::parse("center"), Some(Align::Centre));
    assert_eq!(Align::parse(Align::Right.name()), Some(Align::Right));
}
