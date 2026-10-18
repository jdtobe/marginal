//! Small parsers for the CSS values themes use: colors and lengths.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const TRANSPARENT: Self = Self { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self { a: (self.a as f32 * alpha.clamp(0.0, 1.0)).round() as u8, ..self }
    }
}

pub fn parse_color(value: &str) -> Option<Color> {
    let v = value.trim();
    if let Some(hex) = v.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => Some(Color {
                r: digit(0)? * 17,
                g: digit(1)? * 17,
                b: digit(2)? * 17,
                a: if hex.len() == 4 { digit(3)? * 17 } else { 255 },
            }),
            6 | 8 => {
                Some(Color { r: pair(0)?, g: pair(2)?, b: pair(4)?, a: if hex.len() == 8 { pair(6)? } else { 255 } })
            }
            _ => None,
        };
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.strip_suffix(')')?;
        let parts: Vec<&str> = args.split([',', '/', ' ']).filter(|s| !s.is_empty()).collect();
        let channel = |s: &str| -> Option<u8> {
            Some(match s.strip_suffix('%') {
                Some(p) => (p.parse::<f32>().ok()? * 2.55).round() as u8,
                None => s.parse::<f32>().ok()?.round() as u8,
            })
        };
        let alpha = |s: &str| -> Option<u8> {
            Some(match s.strip_suffix('%') {
                Some(p) => (p.parse::<f32>().ok()? * 2.55).round() as u8,
                None => (s.parse::<f32>().ok()? * 255.0).round() as u8,
            })
        };
        return match parts.as_slice() {
            [r, g, b] => Some(Color::rgb(channel(r)?, channel(g)?, channel(b)?)),
            [r, g, b, a] => Some(Color { r: channel(r)?, g: channel(g)?, b: channel(b)?, a: alpha(a)? }),
            _ => None,
        };
    }
    if let Some(c) = parse_color_function(v) {
        return Some(c);
    }
    if v.eq_ignore_ascii_case("transparent") {
        return Some(Color::TRANSPARENT);
    }
    let lower = v.to_ascii_lowercase();
    NAMED.binary_search_by_key(&lower.as_str(), |(n, _)| n).ok().map(|i| {
        let rgb = NAMED[i].1;
        Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    })
}

/// The arguments of `name(…)` as numbers, with `%` noted, and an optional alpha after `/` or as
/// a fourth value.
/// A color function's numbers (value, was a percentage) and its alpha.
type ColorArgs = (Vec<(f32, bool)>, Option<f32>);

fn args(v: &str, names: &[&str]) -> Option<ColorArgs> {
    let lower = v.to_ascii_lowercase();
    let inner =
        names.iter().find_map(|n| lower.strip_prefix(n).and_then(|r| r.strip_prefix('(')))?.strip_suffix(')')?;
    let (main, alpha) = match inner.split_once('/') {
        Some((m, a)) => (m, Some(a)),
        None => (inner, None),
    };
    let num = |t: &str| -> Option<(f32, bool)> {
        let t = t.trim();
        if t == "none" {
            return Some((0.0, false));
        }
        match t.strip_suffix('%') {
            Some(p) => Some((p.parse().ok()?, true)),
            None => Some((t.trim_end_matches("deg").parse().ok()?, false)),
        }
    };
    let mut values: Vec<(f32, bool)> =
        main.split([',', ' ']).filter(|t| !t.trim().is_empty()).map(num).collect::<Option<_>>()?;
    let alpha = match alpha {
        Some(a) => Some(num(a)?),
        None if values.len() == 4 => values.pop(),
        None => None,
    }
    .map(|(a, pct)| if pct { a / 100.0 } else { a });
    Some((values, alpha))
}

fn rgba(r: f32, g: f32, b: f32, alpha: Option<f32>) -> Color {
    let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color { r: ch(r), g: ch(g), b: ch(b), a: ch(alpha.unwrap_or(1.0)) }
}

/// `hsl()`, `hwb()`, `oklch()` and `oklab()`; `rgb()` is handled separately.
fn parse_color_function(v: &str) -> Option<Color> {
    if let Some((a, alpha)) = args(v, &["hsla", "hsl"]) {
        let [(h, _), (s, _), (l, _)] = a.as_slice() else { return None };
        let (h, s, l) = (h.rem_euclid(360.0), s / 100.0, l / 100.0);
        let f = |n: f32| {
            let k = (n + h / 30.0) % 12.0;
            l - s * l.min(1.0 - l) * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
        };
        return Some(rgba(f(0.0), f(8.0), f(4.0), alpha));
    }
    if let Some((a, alpha)) = args(v, &["hwb"]) {
        let [(h, _), (w, _), (b, _)] = a.as_slice() else { return None };
        let (w, b) = (w / 100.0, b / 100.0);
        let base = parse_color_function(&format!("hsl({h}, 100%, 50%)"))?;
        let mix = |c: u8| (c as f32 / 255.0) * (1.0 - w - b) + w;
        return Some(rgba(mix(base.r), mix(base.g), mix(base.b), alpha));
    }
    let lab = if let Some((a, alpha)) = args(v, &["oklch"]) {
        let [(l, lp), (c, _), (h, _)] = a.as_slice() else { return None };
        let l = if *lp { l / 100.0 } else { *l };
        let h = h.to_radians();
        Some((l, c * h.cos(), c * h.sin(), alpha))
    } else if let Some((a, alpha)) = args(v, &["oklab"]) {
        let [(l, lp), (a2, _), (b2, _)] = a.as_slice() else { return None };
        Some((if *lp { l / 100.0 } else { *l }, *a2, *b2, alpha))
    } else {
        None
    };
    let (l, a, b, alpha) = lab?;
    // OKLab to linear sRGB, then gamma-encode.
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let lin = [
        4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3,
        -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3,
    ];
    let enc = |x: f32| if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    Some(rgba(enc(lin[0]), enc(lin[1]), enc(lin[2]), alpha))
}

/// CSS named colors, sorted by name. (The CSS parser prints the shortest spelling of a
/// color, so `#f00` in a theme arrives here as `red`.)
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f32),
    Em(f32),
    Rem(f32),
    Percent(f32),
    Number(f32),
}

impl Length {
    /// Resolves to pixels. `basis` is what `em` and `%` are relative to.
    pub fn resolve(self, basis: f32, root: f32) -> f32 {
        match self {
            Length::Px(v) => v,
            Length::Em(v) | Length::Number(v) => v * basis,
            Length::Rem(v) => v * root,
            Length::Percent(v) => v / 100.0 * basis,
        }
    }
}

pub fn parse_length(value: &str) -> Option<Length> {
    let v = value.trim();
    let num = |s: &str| s.parse::<f32>().ok();
    if let Some(n) = v.strip_suffix("rem") {
        num(n).map(Length::Rem)
    } else if let Some(n) = v.strip_suffix("px") {
        num(n).map(Length::Px)
    } else if let Some(n) = v.strip_suffix("em") {
        num(n).map(Length::Em)
    } else if let Some(n) = v.strip_suffix('%') {
        num(n).map(Length::Percent)
    } else {
        num(v).map(Length::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some(Color::WHITE));
        assert_eq!(parse_color("#0e1116"), Some(Color::rgb(14, 17, 22)));
        assert_eq!(parse_color("rgba(255, 0, 0, .5)"), Some(Color { r: 255, g: 0, b: 0, a: 128 }));
        assert_eq!(parse_color("rgb(0 0 0 / 50%)"), Some(Color { r: 0, g: 0, b: 0, a: 128 }));
        assert_eq!(parse_color("nonsense"), None);
        assert_eq!(parse_color("red"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color("RebeccaPurple"), Some(Color::rgb(0x66, 0x33, 0x99)));
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0), "sorted for binary search");
        assert_eq!(parse_color("hsl(220, 13%, 18%)"), Some(Color::rgb(40, 44, 52)));
        assert_eq!(parse_color("hsl(0 100% 50% / 50%)"), Some(Color { r: 255, g: 0, b: 0, a: 128 }));
        assert_eq!(parse_color("oklch(62.8% 0.2577 29.23)"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color("oklab(1 0 0)"), Some(Color::WHITE));
        assert_eq!(parse_color("hwb(0 0% 0%)"), Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn lengths() {
        assert_eq!(parse_length("1.5em").unwrap().resolve(16.0, 16.0), 24.0);
        assert_eq!(parse_length("2rem").unwrap().resolve(10.0, 15.0), 30.0);
        assert_eq!(parse_length("12px"), Some(Length::Px(12.0)));
    }
}
