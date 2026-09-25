use crate::BytecodeMessage;
use crate::bytecode;
use crate::entry::{ShEntry, sh_entries};
use crate::sh_capnp::sh_args;
use dusk_capnp::capnp::any_pointer;
use dusk_capnp::capnp::message::HeapAllocator;
use dusk_capnp::dusk_capnp::{dusk, program_args};
use dusk_program::anyhow::{self, Context};
use dusk_program_sh_compiler::{ast, parser, tokenize};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::string::String;
use std::vec::Vec;

pub async fn compile(client: dusk::Client, source: &str) -> anyhow::Result<BytecodeMessage> {
    let mut script = BytecodeMessage::new(HeapAllocator::new());
    write(client, source, &[], script.get_root()?).await?;
    Ok(script)
}

pub async fn compile_to_words(client: dusk::Client, source: &str) -> anyhow::Result<Vec<u8>> {
    let mut script = compile(client, source).await?;
    let mut message = capnp::message::Builder::new_default();
    copy_bytecode(
        script.get_root::<bytecode::Builder>()?.into_reader(),
        message.init_root(),
        &sh_entries(),
    )?;
    Ok(capnp::serialize::write_message_to_words(&message))
}

pub async fn compile_into(
    client: dusk::Client,
    source: &str,
    defined_functions: &[String],
    builder: bytecode::Builder<'_>,
) -> anyhow::Result<()> {
    write(client, source, defined_functions, builder).await
}

#[derive(Clone, Copy)]
enum Scope {
    TopLevel,
    FunctionBody,
}

async fn write(
    client: dusk::Client,
    source: &str,
    defined_functions: &[String],
    builder: bytecode::Builder<'_>,
) -> anyhow::Result<()> {
    let stripped = tokenize::strip_comments(source);
    let parsed = parser::parse(&stripped)?;
    let entries = sh_entries();
    let functions: HashSet<String> = defined_functions.iter().cloned().collect();
    write_ast(
        &parsed,
        builder,
        &functions,
        Scope::TopLevel,
        &client,
        &entries,
    )
    .await
}

fn write_ast<'a, 'b: 'a>(
    parsed: &'a ast::Ast<'a>,
    builder: bytecode::Builder<'b>,
    enclosing_functions: &'a HashSet<String>,
    scope: Scope,
    client: &'a dusk::Client,
    entries: &'a [ShEntry],
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>> {
    Box::pin(async move {
        let mut functions = enclosing_functions.clone();
        let mut statements = builder.init_statements(parsed.statements.len() as u32);
        for (index, statement) in parsed.statements.iter().enumerate() {
            let statement_builder = statements.reborrow().get(index as u32);
            match statement {
                ast::Statement::Expr(expr) => {
                    write_expr(
                        expr,
                        statement_builder.init_expr(),
                        &functions,
                        scope,
                        client,
                        entries,
                    )
                    .await?;
                }
                ast::Statement::FunctionDefinition { symbol, body } => {
                    functions.insert(String::from(*symbol));
                    let mut definition = statement_builder.init_function_definition();
                    definition.set_symbol(symbol);
                    write_ast(
                        body,
                        definition.init_body(),
                        &functions,
                        Scope::FunctionBody,
                        client,
                        entries,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    })
}

fn write_expr<'a, 'b: 'a>(
    expr: &'a ast::Expr<'a>,
    builder: bytecode::statement::expr::Builder<'b>,
    functions: &'a HashSet<String>,
    scope: Scope,
    client: &'a dusk::Client,
    entries: &'a [ShEntry],
) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>> {
    Box::pin(async move {
        let (first, second, mut pair) = match expr {
            ast::Expr::Command(command) => {
                return write_command(command, builder, functions, scope, client, entries).await;
            }
            ast::Expr::And(first, second) => (first, second, builder.init_and()),
            ast::Expr::Or(first, second) => (first, second, builder.init_or()),
        };
        write_expr(
            first,
            pair.reborrow().init_first(),
            functions,
            scope,
            client,
            entries,
        )
        .await?;
        write_expr(
            second,
            pair.init_second(),
            functions,
            scope,
            client,
            entries,
        )
        .await
    })
}

async fn write_command(
    command: &str,
    mut builder: bytecode::statement::expr::Builder<'_>,
    functions: &HashSet<String>,
    scope: Scope,
    client: &dusk::Client,
    entries: &[ShEntry],
) -> anyhow::Result<()> {
    let (remaining, words) = crate::parser::command_words(command)
        .map_err(|_| anyhow::anyhow!("invalid command `{command}`"))?;
    if !remaining.trim().is_empty() {
        anyhow::bail!("invalid command `{command}`");
    }
    let program = *words.first().context("empty command")?;
    let entry = entries.iter().find(|entry| entry.info.name == program);
    if functions.contains(program) {
        if words.len() > 1 {
            anyhow::bail!("function `{program}` cannot take arguments");
        }
        builder.set_call(program);
        return Ok(());
    }
    if let (Scope::FunctionBody, None, 1) = (scope, entry, words.len()) {
        builder.set_call(program);
        return Ok(());
    }
    let entry = entry.with_context(|| std::format!("no sh entry found for `{program}`"))?;
    let program_args = entry
        .program_args_builder
        .build(client.clone(), &words[1..])
        .await
        .context("program args builder failed")?;
    program_args.with_reader(|reader| builder.set_program_args(reader))?;
    Ok(())
}

fn copy_bytecode(
    source: bytecode::Reader<'_>,
    destination: bytecode::Builder<'_>,
    entries: &[ShEntry],
) -> anyhow::Result<()> {
    let statements = source.get_statements()?;
    let mut destination_statements = destination.init_statements(statements.len());
    for (index, statement) in statements.iter().enumerate() {
        let destination_statement = destination_statements.reborrow().get(index as u32);
        match statement.which()? {
            bytecode::statement::Which::Expr(expr) => {
                copy_expr(expr?, destination_statement.init_expr(), entries)?;
            }
            bytecode::statement::Which::FunctionDefinition(definition) => {
                let definition = definition?;
                let mut destination_definition = destination_statement.init_function_definition();
                destination_definition.set_symbol(definition.get_symbol()?);
                copy_bytecode(
                    definition.get_body()?,
                    destination_definition.init_body(),
                    entries,
                )?;
            }
        }
    }
    Ok(())
}

fn copy_expr(
    source: bytecode::statement::expr::Reader<'_>,
    mut destination: bytecode::statement::expr::Builder<'_>,
    entries: &[ShEntry],
) -> anyhow::Result<()> {
    use bytecode::statement::expr::Which;
    let (pair, mut destination_pair) = match source.which()? {
        Which::ProgramArgs(program_args) => {
            return copy_program_args(program_args?, destination.init_program_args(), entries);
        }
        Which::Call(symbol) => {
            destination.set_call(symbol?);
            return Ok(());
        }
        Which::And(pair) => (pair?, destination.init_and()),
        Which::Or(pair) => (pair?, destination.init_or()),
    };
    copy_expr(
        pair.get_first()?,
        destination_pair.reborrow().init_first(),
        entries,
    )?;
    copy_expr(pair.get_second()?, destination_pair.init_second(), entries)
}

fn copy_program_args(
    source: program_args::Reader<'_, any_pointer::Owned, any_pointer::Owned>,
    mut destination: program_args::Builder<'_, any_pointer::Owned, any_pointer::Owned>,
    entries: &[ShEntry],
) -> anyhow::Result<()> {
    let program_id = source.get_program_id();
    destination.set_program_id(program_id);
    match source.get_pid().which()? {
        program_args::pid::Which::Auto(()) => destination.reborrow().get_pid().set_auto(()),
        program_args::pid::Which::Fixed(pid) => destination.reborrow().get_pid().set_fixed(pid),
    }
    let data = source.get_args().get_data()?;
    let mut destination_data = destination.init_args().init_data();
    if program_id == crate::sh_capnp::PROGRAM_ID {
        let mut destination_data: sh_args::data::Builder = destination_data.init_as();
        match data.get_as::<sh_args::data::Reader>()?.which()? {
            sh_args::data::Which::Script(script) => {
                copy_bytecode(script?, destination_data.init_script(), entries)?
            }
            sh_args::data::Which::DetachedScript(script) => {
                copy_bytecode(script?, destination_data.init_detached_script(), entries)?
            }
            sh_args::data::Which::Server(()) => destination_data.set_server(()),
            sh_args::data::Which::Prompt(client_hostname) => {
                destination_data.set_prompt(client_hostname?)
            }
        }
        return Ok(());
    }
    if data.target_size()?.cap_count > 0 {
        let program = entries
            .iter()
            .find(|entry| entry.info.program_id == Some(program_id))
            .map_or(std::format!("{program_id:#x}"), |entry| {
                String::from(entry.info.name)
            });
        anyhow::bail!(
            "`{program}` cannot be used in compile_sh!: its args hold a capability, which a script compiled at build time cannot carry"
        );
    }
    destination_data.set_as(data)?;
    Ok(())
}
