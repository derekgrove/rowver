//! Read the text of an expression into a tree of operations.
//!
//! The grammar is this crate's own; its operators and their precedence follow Python's so that an
//! expression reads as the recipe line it replaces. From loosest to tightest: `where`, `or`,
//! `and`, `not`, comparisons and `in`, `+ -`, `* / %`, unary `-`, `**`, then `.name`,
//! `[mask]`, and calls.

use crate::array::{Constant, Operator};
use crate::{problem, Result};

/// One node of an expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    Constant(Constant),
    /// A branch, an object, or a defined name.
    Name(String),
    /// `left.name`: a field of the sample, or an attribute of an object.
    Field(Box<Expression>, String),
    Call(String, Vec<Expression>),
    Negate(Box<Expression>),
    Not(Box<Expression>),
    Binary(Operator, Box<Expression>, Box<Expression>),
    /// `values[mask]`: the members of each event's list that the mask keeps.
    Pick(Box<Expression>, Box<Expression>),
    /// `Collection where condition`: the members of a collection that pass.
    Where(Box<Expression>, Box<Expression>),
    /// `value in [a, b]`.
    In(Box<Expression>, Vec<Expression>),
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
            let number = if float {
                written.parse().map(Constant::Float).ok()
            } else {
                written.parse().map(Constant::Whole).ok()
            };
            let number = number.ok_or_else(|| {
                problem(format!("{written} is not a number (column {})", start + 1))
            })?;
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
            let symbol = SYMBOLS.iter().find(|symbol| rest.starts_with(**symbol));
            let symbol = symbol.ok_or_else(|| {
                problem(format!("Unexpected {character} at column {}", start + 1))
            })?;
            out.push((Token::Symbol(symbol), start));
            at += symbol.len();
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    at: usize,
    length: usize,
    /// How many brackets and calls are open.
    nested: usize,
}

/// How deeply brackets and calls may nest.
const DEEPEST: usize = 100;

impl Parser {
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
        if self.symbol(symbol) {
            Ok(())
        } else {
            Err(problem(format!("Expected {symbol} at {}", self.place())))
        }
    }

    /// `or-expression [where or-expression]`.
    fn expression(&mut self) -> Result<Expression> {
        let left = self.either()?;
        if self.word("where") {
            let condition = self.either()?;
            return Ok(Expression::Where(Box::new(left), Box::new(condition)));
        }
        Ok(left)
    }

    /// A chain of one binary operator level, built left to right.
    fn chain(
        &mut self,
        next: fn(&mut Self) -> Result<Expression>,
        operator: fn(&mut Self) -> Option<Operator>,
    ) -> Result<Expression> {
        let mut left = next(self)?;
        while let Some(found) = operator(self) {
            left = Expression::Binary(found, Box::new(left), Box::new(next(self)?));
        }
        Ok(left)
    }

    /// `and-expression (or and-expression)*`.
    fn either(&mut self) -> Result<Expression> {
        self.chain(Self::both, |parser| {
            parser.word("or").then_some(Operator::Or)
        })
    }

    /// `not-expression (and not-expression)*`.
    fn both(&mut self) -> Result<Expression> {
        self.chain(Self::negation, |parser| {
            parser.word("and").then_some(Operator::And)
        })
    }

    /// `not not-expression`, or a comparison.
    fn negation(&mut self) -> Result<Expression> {
        if self.word("not") {
            return Ok(Expression::Not(Box::new(self.negation()?)));
        }
        self.comparison()
    }

    /// `sum [comparison sum]`, or `sum in [list]`.
    fn comparison(&mut self) -> Result<Expression> {
        let left = self.sum()?;
        if self.word("in") {
            self.expect("[")?;
            let mut members = Vec::new();
            while !self.symbol("]") {
                members.push(self.either()?);
                if !self.symbol(",") {
                    self.expect("]")?;
                    break;
                }
            }
            return Ok(Expression::In(Box::new(left), members));
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
                return Ok(Expression::Binary(
                    operator,
                    Box::new(left),
                    Box::new(self.sum()?),
                ));
            }
        }
        Ok(left)
    }

    /// `term ((+|-) term)*`.
    fn sum(&mut self) -> Result<Expression> {
        self.chain(Self::term, |parser| {
            if parser.symbol("+") {
                Some(Operator::Add)
            } else {
                parser.symbol("-").then_some(Operator::Subtract)
            }
        })
    }

    /// `unary ((*|/|%) unary)*`.
    fn term(&mut self) -> Result<Expression> {
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
    fn unary(&mut self) -> Result<Expression> {
        if self.symbol("-") {
            return Ok(Expression::Negate(Box::new(self.unary()?)));
        }
        let base = self.postfix()?;
        if self.symbol("**") {
            let power = self.unary()?;
            return Ok(Expression::Binary(
                Operator::Power,
                Box::new(base),
                Box::new(power),
            ));
        }
        Ok(base)
    }

    /// A primary followed by any number of `.name` and `[mask]`.
    fn postfix(&mut self) -> Result<Expression> {
        let mut value = self.primary()?;
        loop {
            if self.symbol(".") {
                match self.tokens.get(self.at) {
                    Some((Token::Name(name), _)) => {
                        value = Expression::Field(Box::new(value), name.clone());
                        self.at += 1;
                    }
                    _ => {
                        return Err(problem(format!(
                            "Expected a name after . at {}",
                            self.place()
                        )))
                    }
                }
            } else if self.symbol("[") {
                let mask = self.expression()?;
                self.expect("]")?;
                value = Expression::Pick(Box::new(value), Box::new(mask));
            } else {
                return Ok(value);
            }
        }
    }

    /// A number, text, name, call, or parenthesised expression.
    fn primary(&mut self) -> Result<Expression> {
        let place = self.place();
        let token = self.tokens.get(self.at).map(|(token, _)| token.clone());
        self.at += 1;
        self.nested += 1;
        if self.nested > DEEPEST {
            return Err(problem(format!("Too many nested brackets at {place}")));
        }
        let value = self.inner(token, place)?;
        self.nested -= 1;
        Ok(value)
    }

    /// The value a primary's first token begins.
    fn inner(&mut self, token: Option<Token>, place: String) -> Result<Expression> {
        Ok(match token {
            Some(Token::Number(number)) => Expression::Constant(number),
            Some(Token::Text(text)) => Expression::Constant(Constant::Text(text)),
            Some(Token::Symbol("(")) => {
                let inner = self.expression()?;
                self.expect(")")?;
                inner
            }
            Some(Token::Name(name)) => match name.as_str() {
                "true" => Expression::Constant(Constant::Truth(true)),
                "false" => Expression::Constant(Constant::Truth(false)),
                "pi" => Expression::Constant(Constant::Float(std::f64::consts::PI)),
                "inf" => Expression::Constant(Constant::Float(f64::INFINITY)),
                "and" | "or" | "not" | "where" | "in" => {
                    return Err(problem(format!("Unexpected {name} at {place}")))
                }
                _ if self.symbol("(") => {
                    let mut arguments = Vec::new();
                    while !self.symbol(")") {
                        arguments.push(self.expression()?);
                        if !self.symbol(",") {
                            self.expect(")")?;
                            break;
                        }
                    }
                    Expression::Call(name, arguments)
                }
                _ => Expression::Name(name),
            },
            _ => return Err(problem(format!("Expected a value at {place}"))),
        })
    }
}

/// Read one expression, refusing anything left over after it.
pub fn parse(text: &str) -> Result<Expression> {
    let mut parser = Parser {
        tokens: tokens(text)?,
        at: 0,
        length: text.chars().count(),
        nested: 0,
    };
    let expression = parser.expression()?;
    if parser.at < parser.tokens.len() {
        return Err(problem(format!("Unexpected text at {}", parser.place())));
    }
    Ok(expression)
}
