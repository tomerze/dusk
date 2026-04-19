use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::tag;
use nom::character::complete::{char, space0, space1};
use nom::multi::{fold_many0, separated_list1};
use nom::sequence::{pair, preceded};
use nom::{IResult, bytes::complete::is_not, sequence::delimited};

use super::ast;

fn quoted_single(input: &str) -> IResult<&str, &str> {
    delimited(char('\''), is_not("'"), char('\'')).parse(input)
}

fn quoted_double(input: &str) -> IResult<&str, &str> {
    delimited(char('"'), is_not("\""), char('"')).parse(input)
}

fn bare_word(input: &str) -> IResult<&str, &str> {
    is_not(" \t\r\n;&|").parse(input)
}

fn word(input: &str) -> IResult<&str, &str> {
    alt((quoted_single, quoted_double, bare_word)).parse(input)
}

fn command_expr(input: &str) -> IResult<&str, ast::Expr<'_>> {
    let (input, parts) = separated_list1(space1, word).parse(input)?;

    let program = parts[0];
    let args = parts[1..].to_vec();

    Ok((input, ast::Expr::Command(ast::Command { program, args })))
}

fn logical_expr(input: &str) -> IResult<&str, ast::Expr<'_>> {
    let (input, init) = command_expr(input)?;

    fold_many0(
        pair(
            preceded(space0, alt((tag("&&"), tag("||")))),
            preceded(space0, command_expr),
        ),
        move || init.clone(),
        |acc, (op, rhs)| {
            if op == "&&" {
                ast::Expr::And(Box::new(acc), Box::new(rhs))
            } else {
                ast::Expr::Or(Box::new(acc), Box::new(rhs))
            }
        },
    )
    .parse(input)
}

fn statement(input: &str) -> IResult<&str, ast::Statement<'_>> {
    let (input, expr) = logical_expr(input)?;
    Ok((input, ast::Statement { expr }))
}

pub fn ast(input: &str) -> IResult<&str, ast::Ast<'_>> {
    let (input, statements) =
        separated_list1(preceded(space0, tag(";")), preceded(space0, statement)).parse(input)?;

    Ok((input, ast::Ast { statements }))
}
