//! Read the text of an expression into a tree of operations.
//!
//! The grammar is this crate's own; its operators and their precedence follow Python's so that an
//! expression reads as the recipe line it replaces. From loosest to tightest: `or`, `and`, `not`,
//! comparisons and `in`, `+ -`, `* / %`, unary `-`, `**`, then `sample.name`, `[mask]`, and calls.

use crate::array::{Constant, Operator};
use crate::{problem, Result};

/// One node of an expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    Constant(Constant),
    /// A branch, or a name the recipe defines.
    Name(String),
    /// `sample.name`: a field of the sample's metadata.
    Sample(String),
    Call(String, Vec<Expression>),
    Negate(Box<Expression>),
    Not(Box<Expression>),
    Binary(Operator, Box<Expression>, Box<Expression>),
    /// `values[mask]`: the members of each event's list that the mask keeps.
    Pick(Box<Expression>, Box<Expression>),
    /// `value in [a, b]`.
    In(Box<Expression>, Vec<Expression>),
}

impl Expression {
    /// The expressions it holds, in the order written.
    pub(crate) fn children(&self) -> impl Iterator<Item = &Expression> {
        let (one, two, rest): (Option<&Expression>, Option<&Expression>, &[Expression]) = match self {
            Expression::Constant(_) | Expression::Name(_) | Expression::Sample(_) => (None, None, &[]),
            Expression::Call(_, arguments) => (None, None, arguments),
            Expression::Negate(inner) | Expression::Not(inner) => (Some(inner), None, &[]),
            Expression::Binary(_, left, right) | Expression::Pick(left, right) => (Some(left), Some(right), &[]),
            Expression::In(left, list) => (Some(left), None, list),
        };
        one.into_iter().chain(two).chain(rest)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(Constant),
    Text(String),
    Name(String),
    Symbol(&'static str),
}

const SYMBOLS: [&str; 18] = [
    "**", "<=", ">=", "==", "!=", "+", "-", "*", "/", "%", "<", ">", "(", ")", "[", "]", ",", ".",
];

/// Split the text into tokens, each with the column it starts at.
fn tokens(text: &str) -> Result<Vec<(Token, usize)>> {
    let characters: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        let start = at;
        let character = characters[at];
        if character.is_whitespace() {
            at += 1;
        } else if character.is_ascii_digit()
            || (character == '.' && characters.get(at + 1).is_some_and(char::is_ascii_digit))
        {
            let mut float = false;
            while at < characters.len() {
                let next = characters[at];
                let exponent = matches!(next, 'e' | 'E')
                    && characters
                        .get(at + 1)
                        .is_some_and(|after| after.is_ascii_digit() || matches!(after, '+' | '-'));
                if exponent {
                    float = true;
                    at += 2;
                } else if next == '.' {
                    float = true;
                    at += 1;
                } else if next.is_ascii_digit() || next == '_' {
                    at += 1;
                } else {
                    break;
                }
            }
            let written: String = characters[start..at]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            let number = match float {
                true => written.parse().map(Constant::Float).map_err(|_| problem(format!("{written} is not a number (column {})", start + 1))),
                false => written.parse().map(Constant::Whole).map_err(|_| problem(format!("{written} is more than the largest whole number, {} (column {}): write it as a float, as {written}.0", i64::MAX, start + 1))),
            }?;
            out.push((Token::Number(number), start));
        } else if character.is_alphabetic() || character == '_' {
            while at < characters.len()
                && (characters[at].is_alphanumeric() || characters[at] == '_')
            {
                at += 1;
            }
            out.push((Token::Name(characters[start..at].iter().collect()), start));
        } else if character == '"' || character == '\'' {
            at += 1;
            while at < characters.len() && characters[at] != character {
                at += 1;
            }
            if at == characters.len() {
                return Err(problem(format!(
                    "The text starting at column {} is never closed",
                    start + 1
                )));
            }
            out.push((
                Token::Text(characters[start + 1..at].iter().collect()),
                start,
            ));
            at += 1;
        } else {
            let rest: String = characters[at..].iter().take(2).collect();
            let Some(symbol) = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol)) else {
                let doubled = rest.len() == 2 && rest.chars().all(|c| c == character) && "&|".contains(character);
                let found = if doubled { rest.as_str() } else { &rest[..character.len_utf8()] };
                let write = match character {
                    '=' => ": to compare, write ==, as x == 1",
                    '!' => ": write not, as not x",
                    '&' => ": write and, as a and b",
                    '|' => ": write or, as a or b",
                    _ => "",
                };
                return Err(problem(format!("Unexpected {found} at column {}{write}", start + 1)));
            };
            out.push((Token::Symbol(symbol), start));
            at += symbol.len();
        }
    }
    Ok(out)
}

struct Parser<'a> {
    tokens: Vec<(Token, usize)>,
    at: usize,
    length: usize,
    /// How many levels the expression being read is inside of: brackets, calls, masks, lists,
    /// `not`, unary `-` and `**`.
    nested: usize,
    /// How many levels deep a name is: a defined name its expression's, any other one.
    named: &'a dyn Fn(&str) -> usize,
}

/// How many levels deep an expression may be, by the README's "How deep".
pub const DEEPEST: usize = 100;

/// An expression and how many levels deep it is.
type Parsed = (Expression, usize);

impl Parser<'_> {
    /// Look at the next token without taking it.
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at).map(|(token, _)| token)
    }

    /// Say where the next token is, for a message.
    fn place(&self) -> String {
        match self.tokens.get(self.at) {
            Some((_, column)) => format!("column {}", column + 1),
            None => format!("the end (column {})", self.length + 1),
        }
    }

    /// Take the next token if it is this symbol.
    fn symbol(&mut self, symbol: &str) -> bool {
        let found = matches!(self.peek(), Some(Token::Symbol(next)) if *next == symbol);
        self.at += found as usize;
        found
    }

    /// Take the next token if it is this word.
    fn word(&mut self, word: &str) -> bool {
        let found = matches!(self.peek(), Some(Token::Name(next)) if next == word);
        self.at += found as usize;
        found
    }

    /// Require a symbol.
    fn expect(&mut self, symbol: &str) -> Result<()> {
        match self.symbol(symbol) {
            true => Ok(()),
            false => Err(problem(format!("Expected {symbol} at {}", self.place()))),
        }
    }

    /// One level around something `inner` levels deep, refused past the deepest.
    fn level(&self, inner: usize) -> Result<usize> {
        match inner < DEEPEST {
            true => Ok(inner + 1),
            false => Err(self.too_deep()),
        }
    }

    /// What an expression deeper than the deepest is refused with.
    fn too_deep(&self) -> crate::Error {
        problem(format!("The expression is more than {DEEPEST} levels deep at {} (a defined name as deep as its own expression): write it with fewer", self.place()))
    }

    /// Read what `read` reads one level further in, refused before it is read past the deepest.
    fn inside(&mut self, read: fn(&mut Self) -> Result<Parsed>) -> Result<Parsed> {
        if self.nested + 1 >= DEEPEST {
            return Err(self.too_deep());
        }
        self.nested += 1;
        let parsed = read(self);
        self.nested -= 1;
        parsed
    }

    /// The dotted name that begins at the next token, `obj.m_state.fPt` of `….obj.m_state.fPt`.
    fn dotted(&self) -> String {
        let mut parts = Vec::new();
        let mut at = self.at;
        while let Some((Token::Name(part), _)) = self.tokens.get(at) {
            parts.push(part.as_str());
            match self.tokens.get(at + 1) {
                Some((Token::Symbol("."), _)) => at += 2,
                _ => break,
            }
        }
        parts.join(".")
    }

    /// A chain of one binary operator level, built left to right.
    fn chain(&mut self, next: fn(&mut Self) -> Result<Parsed>, operator: fn(&mut Self) -> Option<Operator>) -> Result<Parsed> {
        let (mut left, mut depth) = next(self)?;
        while let Some(found) = operator(self) {
            let (right, deep) = next(self)?;
            depth = self.level(depth.max(deep))?;
            left = Expression::Binary(found, Box::new(left), Box::new(right));
        }
        Ok((left, depth))
    }

    /// A whole expression: `and-expression (or and-expression)*`.
    fn expression(&mut self) -> Result<Parsed> {
        self.chain(Self::both, |parser| parser.word("or").then_some(Operator::Or))
    }

    /// `not-expression (and not-expression)*`.
    fn both(&mut self) -> Result<Parsed> {
        self.chain(Self::negation, |parser| parser.word("and").then_some(Operator::And))
    }

    /// `not not-expression`, or a comparison.
    fn negation(&mut self) -> Result<Parsed> {
        if self.word("not") {
            let (inner, depth) = self.inside(Self::negation)?;
            return Ok((Expression::Not(Box::new(inner)), self.level(depth)?));
        }
        self.comparison()
    }

    /// The members of a bracketed list, closed by `close`, each a whole expression; the deepest.
    fn members(&mut self, close: &str) -> Result<(Vec<Expression>, usize)> {
        let (mut members, mut depth) = (Vec::new(), 0);
        while !self.symbol(close) {
            let (member, deep) = self.inside(Self::expression)?;
            members.push(member);
            depth = depth.max(deep);
            if !self.symbol(",") {
                self.expect(close)?;
                break;
            }
        }
        Ok((members, depth))
    }

    /// `sum [comparison sum]`, or `sum in [list]`.
    fn comparison(&mut self) -> Result<Parsed> {
        let (left, depth) = self.sum()?;
        if self.word("in") {
            self.expect("[")?;
            let (members, deep) = self.members("]")?;
            return Ok((Expression::In(Box::new(left), members), self.level(depth.max(deep))?));
        }
        let pairs = [
            ("<=", Operator::LessEqual),
            (">=", Operator::GreaterEqual),
            ("==", Operator::Equal),
            ("!=", Operator::NotEqual),
            ("<", Operator::Less),
            (">", Operator::Greater),
        ];
        for (symbol, operator) in pairs {
            if self.symbol(symbol) {
                let (right, deep) = self.sum()?;
                if pairs.iter().any(|(next, _)| matches!(self.peek(), Some(Token::Symbol(found)) if found == next)) {
                    return Err(problem(format!("Comparisons do not chain (the second at {}): write a < b and b < c, not a < b < c", self.place())));
                }
                let compared = Expression::Binary(operator, Box::new(left), Box::new(right));
                return Ok((compared, self.level(depth.max(deep))?));
            }
        }
        Ok((left, depth))
    }

    /// `term ((+|-) term)*`.
    fn sum(&mut self) -> Result<Parsed> {
        self.chain(Self::term, |parser| match parser.symbol("+") {
            true => Some(Operator::Add),
            false => parser.symbol("-").then_some(Operator::Subtract),
        })
    }

    /// `unary ((*|/|%) unary)*`.
    fn term(&mut self) -> Result<Parsed> {
        self.chain(Self::unary, |parser| {
            if parser.symbol("*") {
                Some(Operator::Multiply)
            } else if parser.symbol("/") {
                Some(Operator::Divide)
            } else {
                parser.symbol("%").then_some(Operator::Remainder)
            }
        })
    }

    /// `- unary`, or a power.
    fn unary(&mut self) -> Result<Parsed> {
        if self.symbol("-") {
            let (inner, depth) = self.inside(Self::unary)?;
            return Ok((Expression::Negate(Box::new(inner)), self.level(depth)?));
        }
        let (base, depth) = self.postfix()?;
        if self.symbol("**") {
            let (power, deep) = self.inside(Self::unary)?;
            let raised = Expression::Binary(Operator::Power, Box::new(base), Box::new(power));
            return Ok((raised, self.level(depth.max(deep))?));
        }
        Ok((base, depth))
    }

    /// A primary followed by any number of `[mask]`, or `sample.name`.
    fn postfix(&mut self) -> Result<Parsed> {
        let (mut value, mut depth) = self.primary()?;
        loop {
            let dot = self.place();
            if self.symbol(".") {
                let field = match (&value, self.tokens.get(self.at)) {
                    (Expression::Name(sample), Some((Token::Name(field), _))) if sample == "sample" => field.clone(),
                    (Expression::Name(sample), _) if sample == "sample" => {
                        return Err(problem(format!("Expected a name after sample. at {}", self.place())))
                    }
                    (Expression::Name(left), Some((Token::Name(_), _))) => return Err(problem(format!("Only sample has a field after a dot, as in sample.era (the dot at {dot}). Name a branch in full, such as Jet_pt, and keep some members of it with a mask, such as Jet_pt[good_jet]; a branch whose name holds a dot is written branch('{left}.{}')", self.dotted()))),
                    _ => return Err(problem(format!("Only sample has a field after a dot, as in sample.era (the dot at {dot}): name a branch in full, such as Muon_pt"))),
                };
                value = Expression::Sample(field);
                self.at += 1;
            } else if self.symbol("[") {
                let (mask, deep) = self.inside(Self::expression)?;
                self.expect("]")?;
                value = Expression::Pick(Box::new(value), Box::new(mask));
                depth = self.level(depth.max(deep))?;
            } else {
                return Ok((value, depth));
            }
        }
    }

    /// A number, text, name, call, or parenthesised expression.
    fn primary(&mut self) -> Result<Parsed> {
        let place = self.place();
        let token = self.tokens.get(self.at).map(|(token, _)| token.clone());
        self.at += 1;
        let value = match token {
            Some(Token::Number(number)) => Expression::Constant(number),
            Some(Token::Text(text)) => Expression::Constant(Constant::Text(text)),
            Some(Token::Symbol("(")) => {
                let (inner, depth) = self.inside(Self::expression)?;
                self.expect(")")?;
                return Ok((inner, self.level(depth)?));
            }
            Some(Token::Name(name)) => match name.as_str() {
                "true" => Expression::Constant(Constant::Truth(true)),
                "false" => Expression::Constant(Constant::Truth(false)),
                "pi" => Expression::Constant(Constant::Float(std::f64::consts::PI)),
                "inf" => Expression::Constant(Constant::Float(f64::INFINITY)),
                "not" => return Err(problem(format!("Unexpected not at {place}: not comes at the start of what it turns over, never straight after an operator: write a == (not b)"))),
                "and" | "or" | "in" => return Err(problem(format!("Unexpected {name} at {place}"))),
                _ if self.symbol("(") => {
                    let (arguments, depth) = self.members(")")?;
                    return Ok((Expression::Call(name, arguments), self.level(depth)?));
                }
                _ => {
                    let depth = (self.named)(&name);
                    return Ok((Expression::Name(name), self.level(depth.max(1) - 1)?));
                }
            },
            Some(Token::Symbol("+")) => return Err(problem(format!("Unexpected + at {place}: there is no unary +, so write the value alone, as x"))),
            _ => return Err(problem(format!("Expected a value at {place}"))),
        };
        Ok((value, 1))
    }
}

/// Read one expression, refusing anything left over after it.
pub fn parse(text: &str) -> Result<Expression> {
    parsed(text, &|_| 1).map(|(expression, _)| expression)
}

/// Read one expression, and how many levels deep it is with each name as deep as `named` says.
pub(crate) fn parsed(text: &str, named: &dyn Fn(&str) -> usize) -> Result<Parsed> {
    let mut parser = Parser { tokens: tokens(text)?, at: 0, length: text.chars().count(), nested: 0, named };
    let parsed = parser.expression()?;
    if parser.at < parser.tokens.len() {
        return Err(problem(format!("Unexpected text at {}", parser.place())));
    }
    Ok(parsed)
}
