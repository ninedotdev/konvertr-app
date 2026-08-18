//! Color parsing + conversion between hex / rgb / hsl / oklch.
//! Ported from konvertr's color-converter.ts, math included.

/// Normalized color: channels 0..=255, alpha 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

/// Auto-detect and parse hex (#rgb/#rrggbb/#rrggbbaa), rgb()/rgba(),
/// hsl()/hsla(), or oklch(). Returns None on anything else.
pub fn parse(input: &str) -> Option<Rgba> {
    let s = input.trim().to_ascii_lowercase();
    parse_hex(&s)
        .or_else(|| parse_rgb(&s))
        .or_else(|| parse_hsl(&s))
        .or_else(|| parse_oklch(&s))
}

fn clamp(v: f64, min: f64, max: f64) -> f64 {
    v.max(min).min(max)
}

fn clamp_int(v: f64, min: f64, max: f64) -> f64 {
    clamp(v, min, max).round()
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let hex = s.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(f64::from);
    let nibble = |i: usize| {
        u8::from_str_radix(&hex[i..i + 1], 16)
            .ok()
            .map(|v| f64::from(v * 17))
    };
    match hex.len() {
        3 => Some(Rgba {
            r: nibble(0)?,
            g: nibble(1)?,
            b: nibble(2)?,
            a: 1.0,
        }),
        6 => Some(Rgba {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: 1.0,
        }),
        8 => Some(Rgba {
            r: byte(0)?,
            g: byte(2)?,
            b: byte(4)?,
            a: byte(6)? / 255.0,
        }),
        _ => None,
    }
}

/// Split "a, b, c" or "a b c / d"-style args after stripping `prefix(` and `)`.
fn func_args<'a>(s: &'a str, prefixes: &[&str]) -> Option<Vec<&'a str>> {
    let inner = prefixes.iter().find_map(|p| {
        s.strip_prefix(p)
            .and_then(|r| r.trim_start().strip_prefix('('))
    })?;
    let inner = inner.trim_end().strip_suffix(')')?;
    Some(inner.split(',').map(str::trim).collect())
}

fn parse_rgb(s: &str) -> Option<Rgba> {
    let args = func_args(s, &["rgba", "rgb"])?;
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let chan = |t: &str| -> Option<f64> {
        let v: u32 = t.parse().ok()?;
        if t.len() > 3 {
            return None;
        }
        Some(clamp_int(v as f64, 0.0, 255.0))
    };
    let a = match args.get(3) {
        Some(t) => clamp(t.parse().ok()?, 0.0, 1.0),
        None => 1.0,
    };
    Some(Rgba {
        r: chan(args[0])?,
        g: chan(args[1])?,
        b: chan(args[2])?,
        a,
    })
}

fn parse_hsl(s: &str) -> Option<Rgba> {
    let args = func_args(s, &["hsla", "hsl"])?;
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let h: f64 = args[0].parse::<f64>().ok()? % 360.0;
    let sat = clamp(
        args[1].strip_suffix('%')?.parse::<f64>().ok()? / 100.0,
        0.0,
        1.0,
    );
    let l = clamp(
        args[2].strip_suffix('%')?.parse::<f64>().ok()? / 100.0,
        0.0,
        1.0,
    );
    let a = match args.get(3) {
        Some(t) => clamp(t.parse().ok()?, 0.0, 1.0),
        None => 1.0,
    };
    let (r, g, b) = hsl_to_rgb(h, sat, l);
    Some(Rgba { r, g, b, a })
}

fn parse_oklch(s: &str) -> Option<Rgba> {
    let inner = s
        .strip_prefix("oklch")?
        .trim_start()
        .strip_prefix('(')?
        .trim_end()
        .strip_suffix(')')?
        .trim();
    // "L C H" or "L C H / a", space-separated.
    let (lch, alpha) = match inner.split_once('/') {
        Some((lch, a)) => (lch.trim(), Some(a.trim())),
        None => (inner, None),
    };
    let parts: Vec<&str> = lch.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let l_raw = parts[0];
    let l = if let Some(pct) = l_raw.strip_suffix('%') {
        pct.parse::<f64>().ok()? / 100.0
    } else {
        l_raw.parse().ok()?
    };
    let l = clamp(l, 0.0, 1.0);
    let c = clamp(parts[1].parse().ok()?, 0.0, 0.4);
    let h: f64 = parts[2].parse::<f64>().ok()? % 360.0;
    let a = match alpha {
        Some(t) => clamp(t.parse().ok()?, 0.0, 1.0),
        None => 1.0,
    };
    let (r, g, b) = oklch_to_rgb(l, c, h);
    Some(Rgba { r, g, b, a })
}

// --- HSL <-> RGB ---

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (
        clamp_int((r1 + m) * 255.0, 0.0, 255.0),
        clamp_int((g1 + m) * 255.0, 0.0, 255.0),
        clamp_int((b1 + m) * 255.0, 0.0, 255.0),
    )
}

fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let (r1, g1, b1) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r1.max(g1).max(b1);
    let min = r1.min(g1).min(b1);
    let d = max - min;
    let l = (max + min) / 2.0;
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };
    let mut h = 0.0;
    if d != 0.0 {
        h = if max == r1 {
            ((g1 - b1) / d) % 6.0
        } else if max == g1 {
            (b1 - r1) / d + 2.0
        } else {
            (r1 - g1) / d + 4.0
        };
        h = (h * 60.0).round();
        if h < 0.0 {
            h += 360.0;
        }
    }
    (h, s, l)
}

// --- Linear RGB <-> sRGB ---

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// --- OKLab / OKLCH ---

fn linear_rgb_to_oklab(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    (
        0.2104542553 * l_ + 0.793617785 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.428592205 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.808675766 * s_,
    )
}

fn oklab_to_linear_rgb(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.291485548 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    (
        4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3,
        -1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3,
        -0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3,
    )
}

fn rgb_to_oklch(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let lr = srgb_to_linear(r / 255.0);
    let lg = srgb_to_linear(g / 255.0);
    let lb = srgb_to_linear(b / 255.0);
    let (l, a, bb) = linear_rgb_to_oklab(lr, lg, lb);
    let c = (a * a + bb * bb).sqrt();
    let mut h = bb.atan2(a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    (l, c, h)
}

fn oklch_to_rgb(l: f64, c: f64, h: f64) -> (f64, f64, f64) {
    let h_rad = h.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();
    let (lr, lg, lb) = oklab_to_linear_rgb(l, a, b);
    (
        clamp_int(linear_to_srgb(lr) * 255.0, 0.0, 255.0),
        clamp_int(linear_to_srgb(lg) * 255.0, 0.0, 255.0),
        clamp_int(linear_to_srgb(lb) * 255.0, 0.0, 255.0),
    )
}

// --- Formatting ---

fn round(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// Format a rounded float the way JS does: no trailing zeros, no ".0".
fn fmt_num(value: f64, decimals: i32) -> String {
    let v = round(value, decimals);
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        let s = format!("{:.*}", decimals as usize, v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

impl Rgba {
    pub fn to_hex(&self) -> String {
        let hex = |v: f64| format!("{:02x}", v as u8);
        let base = format!("#{}{}{}", hex(self.r), hex(self.g), hex(self.b));
        if self.a < 1.0 {
            format!("{base}{}", hex((self.a * 255.0).round()))
        } else {
            base
        }
    }

    pub fn to_rgb(&self) -> String {
        let (r, g, b) = (self.r as i64, self.g as i64, self.b as i64);
        if self.a < 1.0 {
            format!("rgba({r}, {g}, {b}, {})", fmt_num(self.a, 2))
        } else {
            format!("rgb({r}, {g}, {b})")
        }
    }

    pub fn to_hsl(&self) -> String {
        let (h, s, l) = rgb_to_hsl(self.r, self.g, self.b);
        let h = h as i64;
        let sp = fmt_num(s * 100.0, 1);
        let lp = fmt_num(l * 100.0, 1);
        if self.a < 1.0 {
            format!("hsla({h}, {sp}%, {lp}%, {})", fmt_num(self.a, 2))
        } else {
            format!("hsl({h}, {sp}%, {lp}%)")
        }
    }

    pub fn to_oklch(&self) -> String {
        let (l, c, h) = rgb_to_oklch(self.r, self.g, self.b);
        let (l, c, h) = (fmt_num(l, 4), fmt_num(c, 4), fmt_num(h, 2));
        if self.a < 1.0 {
            format!("oklch({l} {c} {h} / {})", fmt_num(self.a, 2))
        } else {
            format!("oklch({l} {c} {h})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        let c = parse("#f00").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (255.0, 0.0, 0.0, 1.0));
        let c = parse("#FF8000").unwrap();
        assert_eq!((c.r, c.g, c.b), (255.0, 128.0, 0.0));
        let c = parse("#ff000080").unwrap();
        assert!((c.a - 128.0 / 255.0).abs() < 1e-9);
        assert!(parse("#ff00").is_none());
        assert!(parse("#gggggg").is_none());
    }

    #[test]
    fn parses_rgb_and_hsl() {
        let c = parse("rgb(255, 0, 0)").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (255.0, 0.0, 0.0, 1.0));
        let c = parse("rgba(0, 128, 255, 0.5)").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (0.0, 128.0, 255.0, 0.5));
        let c = parse("hsl(0, 100%, 50%)").unwrap();
        assert_eq!((c.r, c.g, c.b), (255.0, 0.0, 0.0));
        let c = parse("hsla(120, 100%, 50%, 0.25)").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (0.0, 255.0, 0.0, 0.25));
        assert!(parse("rgb(255, 0)").is_none());
        assert!(parse("not a color").is_none());
    }

    #[test]
    fn red_to_oklch_matches_known_value() {
        let c = parse("#ff0000").unwrap();
        let s = c.to_oklch();
        // Reference: oklch(0.6280 0.2577 29.23) for sRGB red.
        assert!(s.starts_with("oklch(0.628"), "{s}");
        assert!(s.contains("29.23"), "{s}");
    }

    #[test]
    fn white_and_black_oklch() {
        let w = parse("#ffffff").unwrap().to_oklch();
        assert!(w.starts_with("oklch(1 0 "), "{w}");
        let b = parse("#000000").unwrap();
        let (l, c, _) = rgb_to_oklch(b.r, b.g, b.b);
        assert!(l.abs() < 1e-6 && c.abs() < 1e-6);
    }

    #[test]
    fn oklch_round_trips_to_red() {
        let c = parse("oklch(0.628 0.2577 29.23)").unwrap();
        assert_eq!(c.to_hex(), "#ff0000");
        let c = parse("oklch(62.8% 0.2577 29.23 / 0.5)").unwrap();
        assert_eq!(c.a, 0.5);
    }

    #[test]
    fn formats_match_web_shapes() {
        let c = parse("#ff0000").unwrap();
        assert_eq!(c.to_hex(), "#ff0000");
        assert_eq!(c.to_rgb(), "rgb(255, 0, 0)");
        assert_eq!(c.to_hsl(), "hsl(0, 100%, 50%)");
        let c = parse("rgba(255, 0, 0, 0.5)").unwrap();
        assert_eq!(c.to_hex(), "#ff000080");
        assert_eq!(c.to_rgb(), "rgba(255, 0, 0, 0.5)");
        assert_eq!(c.to_hsl(), "hsla(0, 100%, 50%, 0.5)");
    }

    #[test]
    fn hsl_round_trips() {
        let c = parse("hsl(210, 40%, 60%)").unwrap();
        let back = parse(&c.to_hsl()).unwrap();
        assert!((back.r - c.r).abs() <= 1.0);
        assert!((back.g - c.g).abs() <= 1.0);
        assert!((back.b - c.b).abs() <= 1.0);
    }
}
