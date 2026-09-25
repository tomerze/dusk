use crate::bytecode;
use crate::entry::{ShEntriesBuilder, ShEntry};
use dusk_capnp::dusk_capnp::dusk;
use dusk_program::anyhow::{self, Context};
use dusk_program::program_args::ProgramArgs;
use dusk_program_sh_compiler::{ast, parser, tokenize};
use std::collections::HashSet;
use std::rc::Rc;
use std::string::String;
use std::vec::Vec;

pub async fn compile<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    source: &str,
    defined_functions: &[String],
) -> anyhow::Result<Vec<u8>> {
    let mut message = capnp::message::Builder::new(capnp::message::HeapAllocator::new());
    write(
        client,
        sh_entries_builder,
        source,
        defined_functions,
        message.init_root::<bytecode::Builder<'_>>(),
        Capabilities::Disconnected,
    )
    .await?;
    Ok(capnp::serialize::write_message_to_words(&message))
}

#[derive(Clone, Copy, PartialEq)]
enum Capabilities {
    Carried,
    Disconnected,
}

pub async fn compile_into<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    source: &str,
    defined_functions: &[String],
    builder: bytecode::Builder<'_>,
) -> anyhow::Result<()> {
    write(
        client,
        sh_entries_builder,
        source,
        defined_functions,
        builder,
        Capabilities::Carried,
    )
    .await
}

async fn write<S: ShEntriesBuilder>(
    client: dusk::Client,
    sh_entries_builder: S,
    source: &str,
    defined_functions: &[String],
    builder: bytecode::Builder<'_>,
    capabilities: Capabilities,
) -> anyhow::Result<()> {
    let stripped = tokenize::strip_comments(source);
    let parsed = parser::parse(&stripped)?;
    let entries = sh_entries_builder.get_entries();
    let mut resolved = Vec::new();
    let enclosing: HashSet<String> = defined_functions.iter().cloned().collect();
    resolve_ast(&parsed, &client, &entries, &enclosing, &mut resolved).await?;
    write_ast(&parsed, &mut resolved.into_iter(), builder, capabilities)?;
    Ok(())
}

enum Resolved {
    Command(Rc<ProgramArgs>),
    Call(String),
}

fn resolve_ast<'a>(
    parsed: &'a ast::Ast<'a>,
    client: &'a dusk::Client,
    entries: &'a [ShEntry],
    enclosing_functions: &'a HashSet<String>,
    resolved: &'a mut Vec<Resolved>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>> {
    Box::pin(async move {
        let mut functions = enclosing_functions.clone();
        for statement in &parsed.statements {
            if let ast::Statement::FunctionDefinition { symbol, .. } = statement {
                functions.insert(String::from(*symbol));
            }
        }
        for statement in &parsed.statements {
            match statement {
                ast::Statement::Expr(expr) => {
                    resolve_expr(expr, client, entries, &functions, resolved).await?;
                }
                ast::Statement::FunctionDefinition { body, .. } => {
                    resolve_ast(body, client, entries, &functions, resolved).await?;
                }
            }
        }
        Ok(())
    })
}

fn resolve_expr<'a>(
    expr: &'a ast::Expr<'a>,
    client: &'a dusk::Client,
    entries: &'a [ShEntry],
    functions: &'a HashSet<String>,
    resolved: &'a mut Vec<Resolved>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>> {
    Box::pin(async move {
        match expr {
            ast::Expr::Command(command) => {
                let words: Vec<&str> = command.split_whitespace().collect();
                let program = *words.first().context("empty command")?;
                let takes_arguments = words.len() > 1;
                if functions.contains(program) {
                    if takes_arguments {
                        anyhow::bail!("function `{program}` cannot take arguments");
                    }
                    resolved.push(Resolved::Call(String::from(program)));
                    return Ok(());
                }
                if !takes_arguments && !entries.iter().any(|entry| entry.info.name == program) {
                    anyhow::bail!("call to unknown symbol: {program}");
                }
                resolved.push(Resolved::Command(
                    program_args_for_command(client.clone(), entries, command).await?,
                ));
                Ok(())
            }
            ast::Expr::And(first, second) | ast::Expr::Or(first, second) => {
                resolve_expr(first, client, entries, functions, resolved).await?;
                resolve_expr(second, client, entries, functions, resolved).await
            }
        }
    })
}

fn write_ast(
    parsed: &ast::Ast<'_>,
    resolved: &mut impl Iterator<Item = Resolved>,
    builder: bytecode::Builder<'_>,
    capabilities: Capabilities,
) -> anyhow::Result<()> {
    let mut statements = builder.init_statements(parsed.statements.len() as u32);
    for (index, statement) in parsed.statements.iter().enumerate() {
        let statement_builder = statements.reborrow().get(index as u32);
        match statement {
            ast::Statement::Expr(expr) => {
                write_expr(expr, resolved, statement_builder.init_expr(), capabilities)?;
            }
            ast::Statement::FunctionDefinition { symbol, body } => {
                let mut definition = statement_builder.init_function_definition();
                definition.set_symbol(symbol);
                write_ast(body, resolved, definition.init_body(), capabilities)?;
            }
        }
    }
    Ok(())
}

fn write_expr(
    expr: &ast::Expr<'_>,
    resolved: &mut impl Iterator<Item = Resolved>,
    mut builder: bytecode::statement::expr::Builder<'_>,
    capabilities: Capabilities,
) -> anyhow::Result<()> {
    match expr {
        ast::Expr::Command(_) => match resolved.next().context("a command went unresolved")? {
            Resolved::Command(program_args) => {
                program_args.with_reader(|reader| match capabilities {
                    Capabilities::Carried => builder.set_program_args(reader),
                    Capabilities::Disconnected => {
                        let mut program_args = builder.reborrow().init_program_args();
                        program_args.set_program_id(reader.get_program_id());
                        program_args
                            .reborrow()
                            .init_args()
                            .init_data()
                            .set_as(reader.get_args().get_data()?)
                    }
                })?;
                Ok(())
            }
            Resolved::Call(symbol) => {
                builder.set_call(&symbol);
                Ok(())
            }
        },
        ast::Expr::And(first, second) => {
            let mut pair = builder.init_and();
            write_expr(first, resolved, pair.reborrow().init_first(), capabilities)?;
            write_expr(second, resolved, pair.init_second(), capabilities)
        }
        ast::Expr::Or(first, second) => {
            let mut pair = builder.init_or();
            write_expr(first, resolved, pair.reborrow().init_first(), capabilities)?;
            write_expr(second, resolved, pair.init_second(), capabilities)
        }
    }
}

async fn program_args_for_command(
    client: dusk::Client,
    entries: &[ShEntry],
    command: &str,
) -> anyhow::Result<Rc<ProgramArgs>> {
    let (remaining, words) = crate::parser::command_words(command)
        .map_err(|_| anyhow::anyhow!("invalid command `{}`", command))?;
    if !remaining.trim().is_empty() {
        anyhow::bail!("invalid command `{command}`");
    }
    let program = words
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let builder = entries
        .iter()
        .find(|entry| entry.info.name == *program)
        .map(|entry| entry.program_args_builder.clone())
        .ok_or_else(|| anyhow::anyhow!("no sh entry found for `{program}`"))?;
    let arg_refs: Vec<&str> = words[1..].to_vec();
    builder
        .build(client, &arg_refs)
        .await
        .context("program args builder failed")
}
