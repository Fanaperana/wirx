//! Component values: strings are kept as written; numbers (base units) are formatted the way
//! engineers write them (`4k7`, `100nF`, `10uH`).

use rhai::Dynamic;

const PREFIXES: [(f64, &str); 8] = [(1e9, "G"), (1e6, "M"), (1e3, "k"), (1.0, ""), (1e-3, "m"), (1e-6, "u"), (1e-9, "n"), (1e-12, "p")];

fn trim(v: f64) -> String {
    let s = format!("{:.3}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Engineering notation; resistors use RKM style (`4k7`), others a unit suffix (`2.2uF`).
pub fn format(v: f64, symbol: &str) -> String {
    let unit = match symbol {
        "capacitor" | "cap_polarized" => "F",
        "inductor" => "H",
        _ => "",
    };
    if v == 0.0 {
        return format!("0{unit}");
    }
    let (scale, prefix) = PREFIXES.iter().copied().find(|(s, _)| v.abs() >= *s * 0.9995).unwrap_or((1e-12, "p"));
    let m = v / scale;
    if symbol == "resistor" {
        let t = trim(m);
        return match (t.split_once('.'), prefix) {
            (Some((a, b)), "") => format!("{a}R{b}"),
            (Some((a, b)), p) => format!("{a}{p}{b}"),
            (None, p) => format!("{t}{p}"),
        };
    }
    format!("{}{prefix}{unit}", trim(m))
}

pub fn value_text(v: &Dynamic, symbol: &str) -> Option<String> {
    if v.is_string() {
        return v.clone().into_string().ok();
    }
    if let Ok(f) = v.as_float() {
        return Some(format(f, symbol));
    }
    if let Ok(i) = v.as_int() {
        return Some(format(i as f64, symbol));
    }
    None
}

const E24: [f64; 24] = [1.0, 1.1, 1.2, 1.3, 1.5, 1.6, 1.8, 2.0, 2.2, 2.4, 2.7, 3.0, 3.3, 3.6, 3.9, 4.3, 4.7, 5.1, 5.6, 6.2, 6.8, 7.5, 8.2, 9.1];

/// Nearest value of the E24 or E96 series (by ratio).
pub fn snap_series(v: f64, series: u32) -> f64 {
    if v <= 0.0 {
        return v;
    }
    let decade = 10f64.powf(v.log10().floor());
    let mantissas: Vec<f64> = if series == 24 { E24.to_vec() } else { (0..96).map(|i| (10f64.powf(i as f64 / 96.0) * 100.0).round() / 100.0).collect() };
    let mut best = (f64::MAX, v);
    for d in [decade / 10.0, decade, decade * 10.0] {
        for m in &mantissas {
            let c = m * d;
            let err = (c / v).ln().abs();
            if err < best.0 {
                best = (err, c);
            }
        }
    }
    (best.1 * 1e6).round() / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_engineers() {
        assert_eq!(format(4700.0, "resistor"), "4k7");
        assert_eq!(format(300.0, "resistor"), "300");
        assert_eq!(format(4.7, "resistor"), "4R7");
        assert_eq!(format(1e-7, "capacitor"), "100nF");
        assert_eq!(format(2.2e-6, "capacitor"), "2.2uF");
        assert_eq!(format(1e-5, "inductor"), "10uH");
        assert_eq!(snap_series(300.0, 24), 300.0);
        assert_eq!(snap_series(4600.0, 24), 4700.0);
    }
}
