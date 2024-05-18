use crate::program::*;
use alloy_primitives::U256;
use winnow::{ascii::{alphanumeric1, multispace0}, combinator::{empty, fail, peek, preceded, repeat, separated, terminated}, dispatch, error::{ErrMode, ParserError}, prelude::*, seq, stream::AsChar, token::any};
use eyre::eyre;

enum Token<S> {
    Let,
    Const,
    Eq,
    Semi,
    Comma,
    Identifier(S),
    Literal(S),
}

fn token<'a>(input: &mut &'a str) -> PResult<Token<&'a str>> {
    dispatch! {
        preceded(multispace0, peek(any));

        t if AsChar::is_alpha(t) => alphanumeric1.map(|id| {
            match id {
                "let" => Token::Let,
                "const" => Token::Const,
                _ => Token::Identifier(id),
            }
        }),

        t if AsChar::is_dec_digit(t) => alphanumeric1.map(|c: &str| Token::Literal(c)),

        '=' => any.map(|_| Token::Eq),
        ';' => any.map(|_| Token::Semi),
        ',' => any.map(|_| Token::Comma),

        _ => fail,
    }
    .parse_next(input)
}

macro_rules! token {
    ($pat:ident$(($($args:pat),*))?) => { token!($pat$(($($args),*))? => ()) };
    ($pat:ident$(($($args:pat),*))? => $expr:expr) => {
        token.verify_map(|t| {
            match t {
                Token::$pat$(($($args),*))? => Some($expr),
                _ => None,
            }
        })
    };
}

fn identifier(input: &mut &str) -> PResult<String> {
    token!(Identifier(id) => id.into()).parse_next(input)
}

fn constant(input: &mut &str) -> PResult<U256> {
    let c = token!(Literal(c) => c).parse_next(input)?;
    U256::from_str_radix(&c, 10).map_err(|_| ErrMode::assert(input, "bad literal"))
}

fn expression(input: &mut &str) -> PResult<Expression<String>> {
    use Expression::*;

    dispatch! {
        token;

        Token::Const => seq!(Const(constant)),
        Token::Identifier(op) => seq!(Op(empty.value(op.into()), repeat(0.., identifier))),
        _ => fail,
    }.parse_next(input)
}

fn statement(input: &mut &str) -> PResult<Statement<String>> {
    terminated(
        dispatch! {
            peek(token);

            Token::Let => seq!(Statement(
                _: token!(Let),
                separated(1.., identifier, token!(Comma)),
                _: token!(Eq),
                expression,
            )),
            _ => seq!(Statement(empty.value(vec![]), expression)),
        },
        token!(Semi),
    ).parse_next(input)
}

fn block(input: &mut &str) -> PResult<Block<String>> {
    seq!(Block(repeat(0.., statement))).parse_next(input)
}

fn file(input: &mut &str) -> PResult<Block<String>> {
    terminated(block, multispace0).parse_next(input)
}

pub fn parse(ref mut input: &str) -> eyre::Result<Block<String>> {
    file.parse(input).map_err(|e| eyre!("parser error: {e}"))
}
