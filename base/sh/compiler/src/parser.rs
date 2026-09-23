use crate::ast::Ast;
use crate::syntax_error::SyntaxError;
use crate::tokenize;

pub fn parse(stripped: &str) -> Result<Ast<'_>, SyntaxError> {
    let (remaining, parsed) = tokenize::ast(stripped).map_err(|_| SyntaxError::whole_script())?;
    let trailing = remaining.trim_start();
    if !trailing.is_empty() {
        return Err(SyntaxError::at(trailing));
    }
    Ok(parsed)
}
