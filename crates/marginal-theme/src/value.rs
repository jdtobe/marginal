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
    match v {
        "black" => Some(Color::BLACK),
        "white" => Some(Color::WHITE),
        "transparent" => Some(Color::TRANSPARENT),
        _ => None,
    }
}

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
    }

    #[test]
    fn lengths() {
        assert_eq!(parse_length("1.5em").unwrap().resolve(16.0, 16.0), 24.0);
        assert_eq!(parse_length("2rem").unwrap().resolve(10.0, 15.0), 30.0);
        assert_eq!(parse_length("12px"), Some(Length::Px(12.0)));
    }
}
