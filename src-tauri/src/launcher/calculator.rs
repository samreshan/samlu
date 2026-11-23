//! Local arithmetic, percentages and unit conversions for launcher queries.

pub fn evaluate(query: &str) -> Option<String> {
    let query = query.trim();
    if query.len() > 256 {
        return None;
    }
    let explicit = query.starts_with('=');
    let expression = query.strip_prefix('=').unwrap_or(query).trim();
    if !expression.starts_with(|c: char| c.is_ascii_digit() || "(+-−.".contains(c)) {
        return None;
    }
    let normalized = expression
        .to_ascii_lowercase()
        .replace('×', "*")
        .replace('÷', "/")
        .replace('−', "-");
    let words: Vec<_> = normalized.split_whitespace().collect();
    match words.as_slice() {
        [percent, "of", amount] => {
            let ratio = percent.strip_suffix('%')?.parse::<f64>().ok()? / 100.0;
            let result = ratio * amount.parse::<f64>().ok()?;
            return result.is_finite().then(|| format_number(result));
        }
        [amount, from, "to" | "in", to] => {
            let (dimension, source) = unit(from)?;
            let (target_dimension, target) = unit(to)?;
            let value = amount.parse::<f64>().ok()? * source / target;
            return (dimension == target_dimension && value.is_finite()).then(|| {
                let symbol = match *to {
                    "b" => "B",
                    "kb" => "kB",
                    "mb" => "MB",
                    "gb" => "GB",
                    "kib" => "KiB",
                    "mib" => "MiB",
                    "gib" => "GiB",
                    other => other,
                };
                format!("{} {symbol}", format_number(value))
            });
        }
        _ => {}
    }
    let expression = normalized.as_str();
    // Leave bare numbers and ISO dates available for ordinary searches.
    if !explicit
        && (expression.parse::<f64>().is_ok()
            || (expression.len() == 10
                && chrono::NaiveDate::parse_from_str(expression, "%Y-%m-%d").is_ok()))
    {
        return None;
    }
    let mut parser = Parser {
        bytes: expression.as_bytes(),
        index: 0,
    };
    let value = parser.expression().ok()?;
    parser.skip_space();
    if parser.index != parser.bytes.len() || !value.is_finite() {
        return None;
    }
    Some(format_number(value))
}

fn unit(name: &str) -> Option<(&'static str, f64)> {
    Some(match name {
        "mm" => ("length", 0.001),
        "cm" => ("length", 0.01),
        "m" => ("length", 1.0),
        "km" => ("length", 1000.0),
        "in" => ("length", 0.0254),
        "ft" => ("length", 0.3048),
        "mi" => ("length", 1609.344),
        "g" => ("mass", 1.0),
        "kg" => ("mass", 1000.0),
        "lb" => ("mass", 453.59237),
        "ms" => ("time", 0.001),
        "s" => ("time", 1.0),
        "min" => ("time", 60.0),
        "h" => ("time", 3600.0),
        "b" => ("bytes", 1.0),
        "kb" => ("bytes", 1000.0),
        "mb" => ("bytes", 1e6),
        "gb" => ("bytes", 1e9),
        "kib" => ("bytes", 1024.0),
        "mib" => ("bytes", 1048576.0),
        "gib" => ("bytes", 1073741824.0),
        _ => return None,
    })
}

fn format_number(value: f64) -> String {
    if value != 0.0 && value.abs() < 1e-9 {
        return value.to_string();
    }
    if value.fract() == 0.0 && value.abs() < i64::MAX as f64 {
        return format!("{}", value as i64);
    }
    let text = format!("{value:.10}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl Parser<'_> {
    fn expression(&mut self) -> Result<f64, ()> {
        let mut value = self.term()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'+') => {
                    self.index += 1;
                    value += self.term()?;
                }
                Some(b'-') => {
                    self.index += 1;
                    value -= self.term()?;
                }
                _ => return Ok(value),
            }
        }
    }

    fn term(&mut self) -> Result<f64, ()> {
        let mut value = self.power()?;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'*') => {
                    self.index += 1;
                    value *= self.power()?;
                }
                Some(b'/') => {
                    self.index += 1;
                    let divisor = self.power()?;
                    if divisor == 0.0 {
                        return Err(());
                    }
                    value /= divisor;
                }
                Some(b'%') => {
                    self.index += 1;
                    let divisor = self.power()?;
                    if divisor == 0.0 {
                        return Err(());
                    }
                    value %= divisor;
                }
                _ => return Ok(value),
            }
        }
    }

    fn power(&mut self) -> Result<f64, ()> {
        let value = self.unary()?;
        self.skip_space();
        if self.peek() == Some(b'^') {
            self.index += 1;
            Ok(value.powf(self.power()?))
        } else {
            Ok(value)
        }
    }

    fn unary(&mut self) -> Result<f64, ()> {
        self.skip_space();
        match self.peek() {
            Some(b'+') => {
                self.index += 1;
                self.unary()
            }
            Some(b'-') => {
                self.index += 1;
                Ok(-self.unary()?)
            }
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> Result<f64, ()> {
        self.skip_space();
        if self.peek() == Some(b'(') {
            self.index += 1;
            let value = self.expression()?;
            self.skip_space();
            if self.peek() != Some(b')') {
                return Err(());
            }
            self.index += 1;
            return Ok(value);
        }
        let start = self.index;
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.')) {
            self.index += 1;
        }
        if start == self.index {
            return Err(());
        }
        std::str::from_utf8(&self.bytes[start..self.index])
            .map_err(|_| ())?
            .parse()
            .map_err(|_| ())
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::evaluate;

    #[test]
    fn understands_launcher_queries() {
        assert_eq!(evaluate("4 × (8 + 2)").as_deref(), Some("40"));
        assert_eq!(evaluate("18% of 250").as_deref(), Some("45"));
        assert_eq!(evaluate("2 GiB in MiB").as_deref(), Some("2048 MiB"));
        for query in ["2026-11-23", "2026", "notes-2", "5 cm to min", "1 / 0"] {
            assert_eq!(evaluate(query), None);
        }
    }

    #[test]
    fn evaluates_precedence_and_parentheses() {
        assert_eq!(evaluate("= 2 + 3 * 4").as_deref(), Some("14"));
        assert_eq!(evaluate("=(2 + 3) * 4").as_deref(), Some("20"));
        assert_eq!(evaluate("=2^3^2").as_deref(), Some("512"));
        assert!(evaluate("= 1 / 0").is_none());
    }
}
