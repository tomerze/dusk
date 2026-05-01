use nom::IResult;
use nom::Parser;
use nom::branch::alt;
use nom::bytes::complete::{is_not, tag, take_while1};
use nom::character::complete::{char, multispace0, space0, space1};
use nom::combinator::recognize;
use nom::multi::{fold_many0, separated_list1};
use nom::sequence::{delimited, pair, preceded};

use super::ast;

fn quoted_single(input: &str) -> IResult<&str, &str> {
    delimited(char('\''), is_not("'"), char('\'')).parse(input)
}

fn quoted_double(input: &str) -> IResult<&str, &str> {
    delimited(char('"'), is_not("\""), char('"')).parse(input)
}

fn bare_word(input: &str) -> IResult<&str, &str> {
    is_not(" \t\r\n;&|(){}").parse(input)
}

fn word(input: &str) -> IResult<&str, &str> {
    alt((quoted_single, quoted_double, bare_word)).parse(input)
}

fn identifier(input: &str) -> IResult<&str, &str> {
    recognize(take_while1(|c: char| c.is_alphanumeric() || c == '_')).parse(input)
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

fn function_definition(input: &str) -> IResult<&str, ast::Statement<'_>> {
    let (input, name) = identifier(input)?;
    let (input, _) = preceded(space0, tag("()")).parse(input)?;
    let (input, _) = preceded(multispace0, char('{')).parse(input)?;
    let (input, body) = preceded(multispace0, ast).parse(input)?;
    let (input, _) = preceded(multispace0, char('}')).parse(input)?;
    Ok((input, ast::Statement::FunctionDefinition { name, body }))
}

fn statement(input: &str) -> IResult<&str, ast::Statement<'_>> {
    alt((function_definition, logical_expr.map(ast::Statement::Expr))).parse(input)
}

fn statement_separator(input: &str) -> IResult<&str, ()> {
    let (input, _) = preceded(
        space0,
        take_while1(|c: char| c == ';' || c == '\n' || c == '\r'),
    )
    .parse(input)?;
    Ok((input, ()))
}

pub fn ast(input: &str) -> IResult<&str, ast::Ast<'_>> {
    let (input, _) = multispace0(input)?;
    let (input, statements) =
        separated_list1(statement_separator, preceded(multispace0, statement)).parse(input)?;
    let (input, _) = multispace0(input)?;
    let (input, _) = nom::combinator::opt(statement_separator).parse(input)?;
    let (input, _) = multispace0(input)?;

    Ok((input, ast::Ast { statements }))
}
