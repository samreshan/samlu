//! Small expression evaluator for launcher queries beginning with `=`.

pub fn evaluate(query: &str) -> Option<String> {
    let expression = query.trim().strip_prefix('=')?.trim();
    if expression.is_empty() {
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

fn format_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON && value.abs() <= i64::MAX as f64 {
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
    fn evaluates_precedence_and_parentheses() {
        assert_eq!(evaluate("= 2 + 3 * 4").as_deref(), Some("14"));
        assert_eq!(evaluate("=(2 + 3) * 4").as_deref(), Some("20"));
        assert_eq!(evaluate("=2^3^2").as_deref(), Some("512"));
        assert!(evaluate("= 1 / 0").is_none());
    }
}
