//! Evaluating what's typed: `+ − × ÷ ^`, brackets, `%`, `sqrt()`, `pi` and `e`. The usual order of working applies,
//! `50%` is 0.5, and a percentage added or taken away is of what it's added to, so `200 + 10%` is 220.

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a sum yet, or not one that makes sense.
    Syntax,
    DivideByZero,
    /// A result that isn't a number: the root of a negative, or too big to hold.
    Domain,
}

impl Error {
    pub fn message(&self) -> &'static str {
        match self {
            Error::Syntax => "",
            Error::DivideByZero => "Can’t divide by zero",
            Error::Domain => "Not a number",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Times,
    Divide,
    Power,
    Percent,
    Open,
    Close,
    Name(String),
}

fn tokens(text: &str) -> Result<Vec<Token>, Error> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | ',' | '\u{a0}' | '\u{202f}' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == ',') {
                    i += 1;
                }
                let number: String = chars[start..i].iter().filter(|c| **c != ',').collect();
                out.push(Token::Number(number.parse().map_err(|_| Error::Syntax)?));
            }
            '+' => {
                out.push(Token::Plus);
                i += 1;
            }
            '-' | '−' | '–' => {
                out.push(Token::Minus);
                i += 1;
            }
            '*' | '×' | 'x' | 'X' | '·' => {
                out.push(Token::Times);
                i += 1;
            }
            '/' | '÷' => {
                out.push(Token::Divide);
                i += 1;
            }
            '^' => {
                out.push(Token::Power);
                i += 1;
            }
            '%' => {
                out.push(Token::Percent);
                i += 1;
            }
            '(' => {
                out.push(Token::Open);
                i += 1;
            }
            ')' => {
                out.push(Token::Close);
                i += 1;
            }
            'π' => {
                out.push(Token::Name("pi".to_owned()));
                i += 1;
            }
            '√' => {
                out.push(Token::Name("sqrt".to_owned()));
                i += 1;
            }
            c if c.is_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].is_alphabetic() {
                    i += 1;
                }
                out.push(Token::Name(chars[start..i].iter().collect::<String>().to_lowercase()));
            }
            _ => return Err(Error::Syntax),
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

/// A value, and whether it was written as a percentage, which `+` and `−` read as a share of the other side.
type Value = (f64, bool);

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn sum(&mut self) -> Result<f64, Error> {
        let mut value = self.product()?.0;
        loop {
            let plus = match self.peek() {
                Some(Token::Plus) => true,
                Some(Token::Minus) => false,
                _ => return Ok(value),
            };
            self.at += 1;
            let (right, percent) = self.product()?;
            let right = if percent { value * right } else { right };
            value = if plus { value + right } else { value - right };
        }
    }

    fn product(&mut self) -> Result<Value, Error> {
        let mut value = self.power()?;
        loop {
            match self.peek() {
                Some(Token::Times) => {
                    self.at += 1;
                    value = (value.0 * self.power()?.0, false);
                }
                Some(Token::Divide) => {
                    self.at += 1;
                    let divisor = self.power()?.0;
                    if divisor == 0.0 {
                        return Err(Error::DivideByZero);
                    }
                    value = (value.0 / divisor, false);
                }
                // 2(3 + 4) and 3pi are products.
                Some(Token::Open | Token::Name(_)) => value = (value.0 * self.power()?.0, false),
                _ => return Ok(value),
            }
        }
    }

    fn power(&mut self) -> Result<Value, Error> {
        let base = self.unary()?;
        if self.peek() == Some(&Token::Power) {
            self.at += 1;
            // Right to left: 2^3^2 is 2^(3^2).
            let exponent = self.power()?.0;
            let value = base.0.powf(exponent);
            return if value.is_finite() { Ok((value, false)) } else { Err(Error::Domain) };
        }
        Ok(base)
    }

    fn unary(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some(Token::Minus) => {
                self.at += 1;
                let (value, percent) = self.unary()?;
                Ok((-value, percent))
            }
            Some(Token::Plus) => {
                self.at += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Value, Error> {
        let (mut value, mut percent) = (self.primary()?, false);
        while self.peek() == Some(&Token::Percent) {
            self.at += 1;
            value /= 100.0;
            percent = true;
        }
        Ok((value, percent))
    }

    fn primary(&mut self) -> Result<f64, Error> {
        match self.next() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::Open) => {
                let value = self.sum()?;
                // A bracket left open at the end of what's typed still counts.
                match self.peek() {
                    Some(Token::Close) => self.at += 1,
                    None => {}
                    Some(_) => return Err(Error::Syntax),
                }
                Ok(value)
            }
            Some(Token::Name(name)) => match name.as_str() {
                "pi" => Ok(std::f64::consts::PI),
                "e" => Ok(std::f64::consts::E),
                "sqrt" => {
                    let value = match self.next() {
                        Some(Token::Open) => {
                            let inner = self.sum()?;
                            if self.peek() == Some(&Token::Close) {
                                self.at += 1;
                            }
                            inner
                        }
                        Some(Token::Number(n)) => n,
                        _ => return Err(Error::Syntax),
                    };
                    if value < 0.0 { Err(Error::Domain) } else { Ok(value.sqrt()) }
                }
                _ => Err(Error::Syntax),
            },
            _ => Err(Error::Syntax),
        }
    }
}

pub fn eval(text: &str) -> Result<f64, Error> {
    let tokens = tokens(text)?;
    if tokens.is_empty() {
        return Err(Error::Syntax);
    }
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.sum()?;
    if parser.at < parser.tokens.len() {
        return Err(Error::Syntax);
    }
    if value.is_finite() { Ok(value) } else { Err(Error::Domain) }
}

/// A result as it's shown: twelve significant digits, so 0.1 + 0.2 is 0.3, with thousands set apart.
pub fn show(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let magnitude = value.abs().log10().floor() as i32;
    if !(-7..15).contains(&magnitude) {
        let text = format!("{value:.11e}");
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        let mantissa = if mantissa.contains('.') { mantissa.trim_end_matches('0').trim_end_matches('.') } else { mantissa };
        return format!("{mantissa}e{exponent}");
    }
    let decimals = (11 - magnitude).max(0) as usize;
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.') } else { &text };
    let (sign, digits) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    let (whole, fraction) = digits.split_once('.').map_or((digits, None), |(whole, fraction)| (whole, Some(fraction)));
    let mut grouped = String::new();
    for (n, digit) in whole.chars().enumerate() {
        if n > 0 && (whole.len() - n) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    match fraction {
        Some(fraction) => format!("{sign}{grouped}.{fraction}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(text: &str, expected: f64) {
        let value = eval(text).unwrap_or_else(|error| panic!("{text}: {error:?}"));
        assert!((value - expected).abs() < 1e-9, "{text} = {value}, not {expected}");
    }

    #[test]
    fn the_usual_order_of_working() {
        is("2 + 3 × 4", 14.0);
        is("(2 + 3) × 4", 20.0);
        is("10 − 4 − 3", 3.0);
        is("100 ÷ 5 ÷ 2", 10.0);
        is("2^3^2", 512.0);
        is("-2^2", 4.0);
        is("2 * -3", -6.0);
    }

    #[test]
    fn typed_the_ways_people_type() {
        is("12*3/4", 9.0);
        is("1,000 + 2,500", 3500.0);
        is("2(3 + 4)", 14.0);
        is("3π", 3.0 * std::f64::consts::PI);
        is("sqrt(16) + √9", 7.0);
        is("(2 + 3", 5.0);
    }

    #[test]
    fn percentages() {
        is("50%", 0.5);
        is("200 + 10%", 220.0);
        is("200 − 10%", 180.0);
        is("200 × 10%", 20.0);
        is("50% × 50%", 0.25);
    }

    #[test]
    fn what_cannot_be_worked_out() {
        assert_eq!(eval(""), Err(Error::Syntax));
        assert_eq!(eval("2 +"), Err(Error::Syntax));
        assert_eq!(eval("2 3"), Err(Error::Syntax));
        assert_eq!(eval("1 ÷ 0"), Err(Error::DivideByZero));
        assert_eq!(eval("sqrt(-1)"), Err(Error::Domain));
        assert_eq!(eval("2 $ 3"), Err(Error::Syntax));
        assert_eq!(eval("10^1000"), Err(Error::Domain));
    }

    #[test]
    fn results_are_shown_tidily() {
        assert_eq!(show(0.1 + 0.2), "0.3");
        assert_eq!(show(1.0 / 3.0), "0.333333333333");
        assert_eq!(show(1234567.0), "1,234,567");
        assert_eq!(show(-1234.5), "-1,234.5");
        assert_eq!(show(0.0), "0");
        assert_eq!(show(1e20), "1e20");
        assert_eq!(show(2.5e-9), "2.5e-9");
        assert_eq!(show(100.0), "100");
    }
}
