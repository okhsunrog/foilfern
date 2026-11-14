//! Parser and evaluator for gettext `Plural-Forms` headers.
//!
//! Self-contained (also compiled into `build.rs` to validate catalogs).

use std::fmt;

/// Error returned for an invalid `Plural-Forms` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluralFormsError(String);

impl fmt::Display for PluralFormsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid Plural-Forms: {}", self.0)
    }
}

impl std::error::Error for PluralFormsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    N,
    Num(u64),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
}

impl Expr {
    fn eval(&self, n: u64) -> u64 {
        match self {
            Expr::N => n,
            Expr::Num(v) => *v,
            Expr::Not(e) => u64::from(e.eval(n) == 0),
            Expr::Cond(c, a, b) => {
                if c.eval(n) != 0 {
                    a.eval(n)
                } else {
                    b.eval(n)
                }
            }
            Expr::Binary(op, a, b) => {
                let a = a.eval(n);
                // Short-circuit like C.
                match op {
                    Op::Or if a != 0 => return 1,
                    Op::And if a == 0 => return 0,
                    _ => {}
                }
                let b = b.eval(n);
                match op {
                    Op::Or | Op::And => u64::from(b != 0),
                    Op::Eq => u64::from(a == b),
                    Op::Ne => u64::from(a != b),
                    Op::Lt => u64::from(a < b),
                    Op::Le => u64::from(a <= b),
                    Op::Gt => u64::from(a > b),
                    Op::Ge => u64::from(a >= b),
                    Op::Add => a.wrapping_add(b),
                    Op::Sub => a.wrapping_sub(b),
                    Op::Mul => a.wrapping_mul(b),
                    Op::Div => a.checked_div(b).unwrap_or(0),
                    Op::Rem => a.checked_rem(b).unwrap_or(0),
                }
            }
        }
    }
}

struct Parser<'a> {
    rest: &'a str,
}

impl Parser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, PluralFormsError> {
        Err(PluralFormsError(format!("{what} at {:?}", self.rest)))
    }

    fn eat(&mut self, token: &str) -> bool {
        self.rest = self.rest.trim_start();
        match self.rest.strip_prefix(token) {
            Some(r) => {
                self.rest = r;
                true
            }
            None => false,
        }
    }

    /// `or ('?' expr ':' expr)?`
    fn expr(&mut self) -> Result<Expr, PluralFormsError> {
        let cond = self.binary(0)?;
        if self.eat("?") {
            let a = self.expr()?;
            if !self.eat(":") {
                return self.err("expected ':'");
            }
            let b = self.expr()?;
            Ok(Expr::Cond(Box::new(cond), Box::new(a), Box::new(b)))
        } else {
            Ok(cond)
        }
    }

    /// Binary operators by precedence level (lowest first). Longer tokens
    /// come first so `<=` is not parsed as `<`.
    const LEVELS: &'static [&'static [(&'static str, Op)]] = &[
        &[("||", Op::Or)],
        &[("&&", Op::And)],
        &[("==", Op::Eq), ("!=", Op::Ne)],
        &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
        &[("+", Op::Add), ("-", Op::Sub)],
        &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
    ];

    fn binary(&mut self, level: usize) -> Result<Expr, PluralFormsError> {
        let Some(ops) = Self::LEVELS.get(level) else {
            return self.unary();
        };
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for &(token, op) in *ops {
                if self.eat(token) {
                    let rhs = self.binary(level + 1)?;
                    lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn unary(&mut self) -> Result<Expr, PluralFormsError> {
        if self.rest.trim_start().starts_with("!=") {
            return self.err("unexpected '!='");
        }
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("(") {
            let e = self.expr()?;
            if !self.eat(")") {
                return self.err("expected ')'");
            }
            return Ok(e);
        }
        if self.eat("n") {
            return Ok(Expr::N);
        }
        self.rest = self.rest.trim_start();
        let end = self
            .rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(self.rest.len());
        if end == 0 {
            return self.err("expected operand");
        }
        let (digits, rest) = self.rest.split_at(end);
        let value = digits
            .parse()
            .map_err(|_| PluralFormsError(format!("bad number {digits}")))?;
        self.rest = rest;
        Ok(Expr::Num(value))
    }
}

/// A parsed `Plural-Forms` header, e.g.
/// `nplurals=2; plural=(n != 1);`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluralForms {
    nplurals: usize,
    expr: Expr,
}

impl PluralForms {
    /// Parses the value of a `Plural-Forms` header.
    pub fn parse(header: &str) -> Result<Self, PluralFormsError> {
        let mut nplurals = None;
        let mut expr = None;
        for part in header.split(';') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            match key.trim() {
                "nplurals" => {
                    nplurals = Some(value.trim().parse::<usize>().map_err(|_| {
                        PluralFormsError(format!("bad nplurals {:?}", value.trim()))
                    })?);
                }
                "plural" => {
                    let mut p = Parser { rest: value };
                    let e = p.expr()?;
                    if !p.rest.trim().is_empty() {
                        return p.err("trailing characters");
                    }
                    expr = Some(e);
                }
                _ => {}
            }
        }
        match (nplurals, expr) {
            (Some(nplurals), Some(expr)) if nplurals > 0 => Ok(Self { nplurals, expr }),
            _ => Err(PluralFormsError(format!(
                "missing nplurals/plural in {header:?}"
            ))),
        }
    }

    /// The English/source-language rule, `nplurals=2; plural=(n != 1);`.
    pub fn english() -> Self {
        Self {
            nplurals: 2,
            expr: Expr::Binary(Op::Ne, Box::new(Expr::N), Box::new(Expr::Num(1))),
        }
    }

    /// Number of plural forms.
    pub fn nplurals(&self) -> usize {
        self.nplurals
    }

    /// Index of the plural form to use for `n`. May be `>= nplurals()` for
    /// broken headers; callers should treat that as "no translation".
    pub fn index(&self, n: u64) -> usize {
        usize::try_from(self.expr.eval(n)).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(header: &str, n: u64) -> usize {
        PluralForms::parse(header).unwrap().index(n)
    }

    #[test]
    fn common_rules() {
        let en = "nplurals=2; plural=(n != 1);";
        assert_eq!([0, 1, 2].map(|n| idx(en, n)), [1, 0, 1]);
        assert_eq!(idx("nplurals=1; plural=0;", 5), 0);
        assert_eq!(idx("nplurals=2; plural=n>1;", 1), 0);
        let ru = "nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);";
        assert_eq!(
            [1, 2, 5, 11, 12, 21, 22, 25, 111, 112, 101].map(|n| idx(ru, n)),
            [0, 1, 2, 2, 2, 0, 1, 2, 2, 2, 0]
        );
        let ar = "nplurals=6; plural=(n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5);";
        assert_eq!(
            [0, 1, 2, 3, 11, 100, 102].map(|n| idx(ar, n)),
            [0, 1, 2, 3, 4, 5, 5]
        );
        let fr = "nplurals=3; plural=(n == 0 || n == 1 ? 0 : n % 1000000 == 0 ? 1 : 2);";
        assert_eq!([0, 1, 2, 1_000_000].map(|n| idx(fr, n)), [0, 0, 2, 1]);
    }

    #[test]
    fn arithmetic_and_not() {
        assert_eq!(idx("nplurals=9; plural=!(n - 1) + 2 * 3 / 2;", 1), 4);
        assert_eq!(idx("nplurals=9; plural=n / 0 + n % 0;", 7), 0);
        assert_eq!(idx("nplurals=9; plural=1 ? 2 : 3 ? 4 : 5;", 0), 2);
    }

    #[test]
    fn errors() {
        assert!(PluralForms::parse("nplurals=2;").is_err());
        assert!(PluralForms::parse("nplurals=2; plural=(n != 1;").is_err());
        assert!(PluralForms::parse("nplurals=2; plural=n ! 1;").is_err());
        assert!(PluralForms::parse("nplurals=x; plural=n;").is_err());
        assert!(PluralForms::parse("nplurals=0; plural=0;").is_err());
    }
}
