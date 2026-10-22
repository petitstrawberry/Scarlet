//! Bounded expression evaluation and unit conversion for the desktop calculator.
//! Arithmetic uses finite IEEE-754 double precision, with ordinary precedence.

use std::fmt;

pub const MAX_EXPRESSION_BYTES: usize = 512;
const MAX_DEPTH: usize = 48;

#[derive(Clone, Debug, PartialEq)]
pub struct CalcError {
    pub column: usize,
    pub message: &'static str,
}

impl fmt::Display for CalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (column {})", self.message, self.column)
    }
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: &'static str) -> CalcError {
        CalcError {
            column: self.source[..self.position].chars().count() + 1,
            message,
        }
    }

    fn peek(&mut self) -> Option<char> {
        while let Some(c) = self.source[self.position..].chars().next() {
            if !c.is_whitespace() {
                return Some(c);
            }
            self.position += c.len_utf8();
        }
        None
    }

    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += c.len_utf8();
        Some(c)
    }

    fn finite(&self, value: f64) -> Result<f64, CalcError> {
        if value.is_finite() {
            Ok(value)
        } else {
            Err(self.error("Result is outside the supported range"))
        }
    }

    fn expression(&mut self, minimum: u8) -> Result<f64, CalcError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("Expression is nested too deeply"));
        }
        let mut left = match self.peek() {
            Some('+') => {
                self.take();
                self.expression(5)?
            }
            Some('-' | '−') => {
                self.take();
                -self.expression(5)?
            }
            Some('(') => {
                self.take();
                let value = self.expression(0)?;
                if self.take() != Some(')') {
                    return Err(self.error("Expected a closing parenthesis"));
                }
                value
            }
            Some('π') => {
                self.take();
                std::f64::consts::PI
            }
            Some('p') if self.source[self.position..].starts_with("pi") => {
                self.position += 2;
                std::f64::consts::PI
            }
            Some('e') => {
                self.take();
                std::f64::consts::E
            }
            Some('s') if self.source[self.position..].starts_with("sqrt") => {
                self.position += 4;
                if self.take() != Some('(') {
                    return Err(self.error("Use sqrt(number)"));
                }
                let value = self.expression(0)?;
                if self.take() != Some(')') {
                    return Err(self.error("Expected a closing parenthesis"));
                }
                if value < 0.0 {
                    return Err(self.error("Square root requires a non-negative number"));
                }
                value.sqrt()
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number()?,
            _ => return Err(self.error("Expected a number or parenthesis")),
        };
        loop {
            let Some(operator) = self.peek() else {
                break;
            };
            if operator == '%' {
                if 9 < minimum {
                    break;
                }
                self.take();
                left /= 100.0;
                continue;
            }
            let (left_binding, right_binding) = match operator {
                '+' | '-' | '−' => (1, 2),
                '*' | '×' | '/' | '÷' => (3, 4),
                '^' => (7, 6),
                _ => break,
            };
            if left_binding < minimum {
                break;
            }
            self.take();
            let right = self.expression(right_binding)?;
            left = match operator {
                '+' => left + right,
                '-' | '−' => left - right,
                '*' | '×' => left * right,
                '/' | '÷' => {
                    if right == 0.0 {
                        return Err(self.error("Cannot divide by zero"));
                    }
                    left / right
                }
                '^' => {
                    if left == 0.0 && right < 0.0 {
                        return Err(self.error("Cannot divide by zero"));
                    }
                    if left < 0.0 && right.fract() != 0.0 {
                        return Err(self.error("Result is not a real number"));
                    }
                    left.powf(right)
                }
                _ => unreachable!(),
            };
            left = self.finite(left)?;
        }
        self.depth -= 1;
        self.finite(left)
    }

    fn number(&mut self) -> Result<f64, CalcError> {
        let start = self.position;
        let bytes = self.source.as_bytes();
        let mut digits = 0;
        while bytes.get(self.position).is_some_and(u8::is_ascii_digit) {
            self.position += 1;
            digits += 1;
        }
        if bytes.get(self.position) == Some(&b'.') {
            self.position += 1;
            while bytes.get(self.position).is_some_and(u8::is_ascii_digit) {
                self.position += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(self.error("Expected digits after the decimal point"));
        }
        if matches!(bytes.get(self.position), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(bytes.get(self.position), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let exponent = self.position;
            while bytes.get(self.position).is_some_and(u8::is_ascii_digit) {
                self.position += 1;
            }
            if exponent == self.position {
                return Err(self.error("Expected exponent digits"));
            }
        }
        let value = self.source[start..self.position]
            .parse::<f64>()
            .map_err(|_| self.error("Invalid number"))?;
        self.finite(value)
    }
}

pub fn evaluate(expression: &str) -> Result<f64, CalcError> {
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(CalcError {
            column: 1,
            message: "Expression is too long (maximum 512 bytes)",
        });
    }
    let mut parser = Parser {
        source: expression,
        position: 0,
        depth: 0,
    };
    let result = parser.expression(0)?;
    if parser.peek().is_some() {
        return Err(parser.error("Expected an operator"));
    }
    Ok(if result == 0.0 { 0.0 } else { result })
}

/// Display twelve significant digits without hiding very small nonzero results.
pub fn format_value(value: f64) -> String {
    if value == 0.0 {
        return "0".into();
    }
    let absolute = value.abs();
    if !(1e-6..1e12).contains(&absolute) {
        let rendered = format!("{value:.11e}");
        let (mantissa, exponent) = rendered.split_once('e').unwrap();
        format!(
            "{}e{}",
            mantissa.trim_end_matches('0').trim_end_matches('.'),
            exponent
        )
    } else {
        let decimals = (11 - absolute.log10().floor() as i32).clamp(0, 17) as usize;
        let rendered = format!("{value:.decimals$}");
        if rendered.contains('.') {
            rendered.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            rendered
        }
    }
}

#[derive(Clone, Copy)]
pub struct Unit {
    pub name: &'static str,
    scale: f64,
    offset: f64,
}

const fn unit(name: &'static str, scale: f64) -> Unit {
    Unit {
        name,
        scale,
        offset: 0.0,
    }
}
pub const CATEGORIES: [&str; 5] = ["Length", "Mass", "Temperature", "Time", "Data size"];
const LENGTH: &[Unit] = &[
    unit("Metres (m)", 1.0),
    unit("Kilometres (km)", 1000.0),
    unit("Centimetres (cm)", 0.01),
    unit("Millimetres (mm)", 0.001),
    unit("Inches (in)", 0.0254),
    unit("Feet (ft)", 0.3048),
    unit("Yards (yd)", 0.9144),
    unit("Miles (mi)", 1609.344),
];
const MASS: &[Unit] = &[
    unit("Kilograms (kg)", 1.0),
    unit("Grams (g)", 0.001),
    unit("Milligrams (mg)", 0.000001),
    unit("Pounds (lb)", 0.45359237),
    unit("Ounces (oz)", 0.028349523125),
];
const TEMPERATURE: &[Unit] = &[
    Unit {
        name: "Celsius (°C)",
        scale: 1.0,
        offset: 273.15,
    },
    Unit {
        name: "Fahrenheit (°F)",
        scale: 5.0 / 9.0,
        offset: 459.67 * 5.0 / 9.0,
    },
    unit("Kelvin (K)", 1.0),
];
const TIME: &[Unit] = &[
    unit("Seconds (s)", 1.0),
    unit("Minutes (min)", 60.0),
    unit("Hours (h)", 3600.0),
    unit("Days (d)", 86400.0),
];
const DATA_SIZE: &[Unit] = &[
    unit("Bytes (B)", 1.0),
    unit("Kilobytes (kB)", 1000.0),
    unit("Megabytes (MB)", 1_000_000.0),
    unit("Gigabytes (GB)", 1_000_000_000.0),
    unit("Kibibytes (KiB)", 1024.0),
    unit("Mebibytes (MiB)", 1_048_576.0),
    unit("Gibibytes (GiB)", 1_073_741_824.0),
];
pub fn units(category: usize) -> &'static [Unit] {
    match category {
        0 => LENGTH,
        1 => MASS,
        2 => TEMPERATURE,
        3 => TIME,
        4 => DATA_SIZE,
        _ => &[],
    }
}

pub fn convert(value: f64, category: usize, from: usize, to: usize) -> Result<f64, &'static str> {
    let choices = units(category);
    let source = choices.get(from).ok_or("Choose a source unit")?;
    let target = choices.get(to).ok_or("Choose a destination unit")?;
    if !value.is_finite() {
        return Err("Enter a finite number");
    }
    let base = value * source.scale + source.offset;
    if category == 2 && base < -1e-10 {
        return Err("Temperature cannot be below absolute zero");
    }
    let result = if from == to {
        value
    } else if category != 2 {
        value * (source.scale / target.scale)
    } else {
        (base - target.offset) / target.scale
    };
    if !result.is_finite() {
        return Err("Converted value is outside the supported range");
    }
    Ok(if result.abs() < 1e-12 && category == 2 {
        0.0
    } else {
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() <= 1e-10 * b.abs().max(1.0), "{a} != {b}");
    }
    #[test]
    fn representable_large_conversion_avoids_intermediate_overflow() {
        assert_eq!(convert(1e308, 0, 1, 1), Ok(1e308));
        assert!(convert(1e308, 0, 1, 7).unwrap().is_finite());
        assert!(convert(-1.0, 2, 2, 2).is_err());
    }
    #[test]
    fn arithmetic_precedence() {
        for (input, expected) in [
            ("2+3*4", 14.0),
            ("(2+3)*4", 20.0),
            ("-2^2", -4.0),
            ("(-2)^2", 4.0),
            ("2^3^2", 512.0),
            ("2^-3", 0.125),
            ("1--2", 3.0),
            ("200*10%", 20.0),
            ("50%+25%", 0.75),
            (".5 + 1.", 1.5),
            ("sqrt(9)+sqrt(16)", 7.0),
            ("1e3 / 2E+2", 5.0),
            (" １２", 0.0),
        ] {
            if input == " １２" {
                assert!(evaluate(input).is_err());
            } else {
                close(evaluate(input).unwrap(), expected);
            }
        }
    }
    #[test]
    fn unicode_and_constants() {
        close(evaluate("−2 × (3 + 4) ÷ 2").unwrap(), -7.0);
        close(evaluate("pi-π").unwrap(), 0.0);
        close(evaluate("e").unwrap(), std::f64::consts::E);
    }
    #[test]
    fn rejects_invalid_input() {
        for input in [
            "", ".", "1+", "(1+2", "2(3)", "1..2", "1e", "sqrt -1", "1/0", "0^-1", "sqrt(-1)",
            "(-1)^.5", "1e999", "1e308*2", "NaN", "∞", "2foo", "1,000", "2)",
        ] {
            assert!(evaluate(input).is_err(), "{input}");
        }
    }
    #[test]
    fn bounds_nesting_and_length() {
        assert!(evaluate(&"(".repeat(60)).is_err());
        assert!(evaluate(&"1".repeat(513)).is_err());
        assert!(evaluate(&format!("{}1{}", "(".repeat(30), ")".repeat(30))).is_ok());
    }
    #[test]
    fn stable_display() {
        assert_eq!(format_value(evaluate(".1+.2").unwrap()), "0.3");
        assert_eq!(format_value(-0.0), "0");
        assert_eq!(format_value(1e-12), "1e-12");
        assert_eq!(format_value(1e12), "1e12");
        assert_eq!(format_value(12345.0), "12345");
    }
    #[test]
    fn conversions_round_trip() {
        for category in 0..CATEGORIES.len() {
            for from in 0..units(category).len() {
                for to in 0..units(category).len() {
                    let value = 1234.5;
                    close(
                        convert(
                            convert(value, category, from, to).unwrap(),
                            category,
                            to,
                            from,
                        )
                        .unwrap(),
                        value,
                    );
                }
            }
        }
    }
    #[test]
    fn known_conversion_values() {
        close(convert(1.0, 0, 7, 0).unwrap(), 1609.344);
        close(convert(32.0, 2, 1, 0).unwrap(), 0.0);
        close(convert(100.0, 2, 0, 1).unwrap(), 212.0);
        close(convert(1.0, 4, 4, 0).unwrap(), 1024.0);
        assert!(convert(-274.0, 2, 0, 2).is_err());
        assert!(convert(f64::INFINITY, 0, 0, 1).is_err());
        assert!(convert(1.0, 99, 0, 1).is_err());
    }
}
